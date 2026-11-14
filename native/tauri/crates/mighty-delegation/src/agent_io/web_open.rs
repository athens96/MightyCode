//! The open_url prompts (C# Core/AgentIO/AgentWebOpenPrompts.cs): a pending choice per request, the 30 s fallback to inApp (unsaved),
//! at most 16 pending, `webOpenChoose`'s answer and `remember`. Owner: W7-C.
//!
//! Each request is independent and lives only in memory. A request ends with the user's choice, the fallback, or its cancellation:
//! its binding revoked, the app quitting ([`AgentWebOpenPrompts::dispose`]), or its future dropped (the server's deadline). A
//! cancelled request's card goes at once, so a late click can neither open it nor remember its choice.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use indexmap::IndexMap;
use tokio::sync::oneshot;

use super::binding::{AgentIoBinding, Revocation};
use super::tools::{absolute_uri, validate};

/// `AgentWebOpenPrompts.DefaultTimeout`: as on the Mac.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// How many requests may wait at once.
pub const MAXIMUM_PENDING: usize = 16;

/// `AgentWebOpenAnswer(Destination, Remember)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebOpenAnswer {
    /// `inApp` or `external`.
    pub destination: String,
    pub remember: bool,
}

/// `AgentWebOpenPrompt(Id, PaneId, WorkspaceId, Url)`: `url` is the address as the agent gave it, `absolute` its `AbsoluteUri`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebOpenPrompt {
    pub id: String,
    pub pane_id: String,
    pub workspace_id: String,
    pub url: String,
    pub absolute: String,
}

/// Why a request ended without an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebOpenError {
    /// The address is not one `open_url` takes.
    Invalid,
    /// Its binding was revoked, or the prompts were disposed.
    Cancelled,
    /// 16 requests already wait.
    TooMany,
}

struct Waiting {
    prompt: WebOpenPrompt,
    binding: Arc<AgentIoBinding>,
    completion: oneshot::Sender<WebOpenAnswer>,
}

#[derive(Default)]
struct Inner {
    pending: IndexMap<String, Waiting>,
    disposed: bool,
}

/// `AgentWebOpenPrompts`.
pub struct AgentWebOpenPrompts {
    inner: Mutex<Inner>,
    lifetime: Revocation,
    /// `Changed`: a card came or went.
    changed: Box<dyn Fn() + Send + Sync>,
}

impl Default for AgentWebOpenPrompts {
    fn default() -> Self {
        AgentWebOpenPrompts::new(Box::new(|| {}))
    }
}

impl AgentWebOpenPrompts {
    pub fn new(changed: Box<dyn Fn() + Send + Sync>) -> AgentWebOpenPrompts {
        AgentWebOpenPrompts { inner: Mutex::new(Inner::default()), lifetime: Revocation::default(), changed }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn live(&self, waiting: &Waiting) -> bool {
        !waiting.binding.is_revoked() && !self.lifetime.is_cancelled()
    }

    /// `ForPane`: the pane's waiting prompts, in the order they came (a cancelled one is never listed).
    pub fn for_pane(&self, pane_id: &str) -> Vec<WebOpenPrompt> {
        self.lock().pending.values().filter(|w| w.prompt.pane_id == pane_id && self.live(w)).map(|w| w.prompt.clone()).collect()
    }

    /// `RequestAsync`: a card in the binding's pane until the user chooses, or `timeout` (30 s) passes and it opens in the app without
    /// remembering. Dropping the future cancels it.
    pub async fn request(
        &self,
        binding: &Arc<AgentIoBinding>,
        url: &str,
        timeout: Option<Duration>,
    ) -> Result<WebOpenAnswer, WebOpenError> {
        // The transport's validator again, so a direct caller cannot show an unsafe link.
        validate("open_url", &serde_json::json!({ "url": url })).map_err(|_| WebOpenError::Invalid)?;
        if binding.is_revoked() || self.lifetime.is_cancelled() {
            return Err(WebOpenError::Cancelled);
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let (sender, mut receiver) = oneshot::channel();
        {
            let mut inner = self.lock();
            if inner.disposed {
                return Err(WebOpenError::Cancelled);
            }
            if inner.pending.len() >= MAXIMUM_PENDING {
                return Err(WebOpenError::TooMany);
            }
            let prompt = WebOpenPrompt {
                id: id.clone(),
                pane_id: binding.pane_id.clone(),
                workspace_id: binding.workspace_id.clone(),
                url: url.to_string(),
                absolute: absolute_uri(url),
            };
            inner.pending.insert(id.clone(), Waiting { prompt, binding: binding.clone(), completion: sender });
        }
        (self.changed)();
        // However this ends (answered, timed out, cancelled, dropped), the card goes.
        let _remove = Remove { prompts: self, id: id.clone() };
        let timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);
        tokio::select! {
            answer = &mut receiver => answer.map_err(|_| WebOpenError::Cancelled),
            _ = binding.revoked().cancelled() => Err(WebOpenError::Cancelled),
            _ = self.lifetime.cancelled() => Err(WebOpenError::Cancelled),
            _ = tokio::time::sleep(timeout) => {
                self.choose(&id, "inApp", false);
                if binding.is_revoked() || self.lifetime.is_cancelled() {
                    return Err(WebOpenError::Cancelled);
                }
                receiver.await.map_err(|_| WebOpenError::Cancelled)
            }
        }
    }

    /// `Choose`: the user's answer to one waiting request; false for a destination other than inApp or external, a request no
    /// longer waiting (answered, timed out, cancelled) or unknown.
    pub fn choose(&self, request_id: &str, destination: &str, remember: bool) -> bool {
        if !matches!(destination, "inApp" | "external") {
            return false;
        }
        {
            let mut inner = self.lock();
            match inner.pending.get(request_id) {
                Some(waiting) if self.live(waiting) => {}
                _ => return false,
            }
            let Some(waiting) = inner.pending.shift_remove(request_id) else { return false };
            let _ = waiting.completion.send(WebOpenAnswer { destination: destination.to_string(), remember });
        }
        (self.changed)();
        true
    }

    fn remove(&self, id: &str) {
        let removed = self.lock().pending.shift_remove(id).is_some();
        if removed {
            (self.changed)();
        }
    }

    /// `Dispose`: every waiting request cancelled (the app quits); no new one is taken.
    pub fn dispose(&self) {
        {
            let mut inner = self.lock();
            if inner.disposed {
                return;
            }
            inner.disposed = true;
        }
        self.lifetime.cancel();
        (self.changed)();
    }
}

/// The card's removal however its request ends.
struct Remove<'a> {
    prompts: &'a AgentWebOpenPrompts,
    id: String,
}

impl Drop for Remove<'_> {
    fn drop(&mut self) {
        self.prompts.remove(&self.id);
    }
}
