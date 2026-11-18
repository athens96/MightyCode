//! What to push (C# `MobilePushPane`, `MobilePushPlanner`, R\MobilePush.cs:110-283; plan §1.3, `push.json`'s planner vectors): the
//! first observation is the baseline; a new request is pushed once; `running → completed|error` with nothing pending pushes
//! `completed`/`failed`; a newly lost sign-in is pushed; names clipped to 80 scalars, the summary to 200 on one line; 512 sent ids and
//! counters for 512 sessions remembered (`Reset` keeps both); ids `request:<runId>:<id>`, `run:<session>:<n>:completed|failed`,
//! `signin:<session>:<n>`. Owner: W8-A.

use std::collections::{HashMap, HashSet, VecDeque};

use mighty_core::model::{RunSession, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_run::antigravity::permissions::PermissionRequest as AntigravityCard;

use crate::push::message::{COMPLETED, FAILED, PERMISSION, PushMessage, PushPermission, PushRequest, SIGNIN};

/// `MobilePushPane.PreviewLimit`: the longest preview handed to the planner, in Unicode scalars (it clips again for the message).
pub const PREVIEW_LIMIT: usize = 400;
/// `MobilePushPlanner.SentMemory`.
pub const SENT_MEMORY: usize = 512;
/// `MobilePushPlanner.CountMemory`: how many sessions' run and sign-in numbers are kept, absent panes oldest first out.
pub const COUNT_MEMORY: usize = 512;

/// C# `MobilePushPane(SessionId, Workspace, Title, Status, Preview, Requests, SignInLost)`: one pane as the planner sees it; `status`
/// is the pane's own run status (running, completed, error, stopped, …), not the phone's display status. Equal panes (the Mac's
/// Equatable, requests in order) decide nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushPane {
    pub session_id: String,
    pub workspace: String,
    pub title: String,
    pub status: String,
    pub preview: Option<String>,
    pub requests: Vec<PushRequest>,
    pub sign_in_lost: bool,
}

impl PushPane {
    pub fn new(
        session_id: &str,
        workspace: &str,
        title: &str,
        status: &str,
        preview: Option<&str>,
        requests: Vec<PushRequest>,
    ) -> PushPane {
        PushPane {
            session_id: session_id.into(),
            workspace: workspace.into(),
            title: title.into(),
            status: status.into(),
            preview: preview.map(str::to_string),
            requests,
            sign_in_lost: false,
        }
    }
}

/// `MobilePushPane.Project`: the panes as the planner sees them — only the panes the phone can see (no files or work map panes, no other
/// computer's workspaces); the pending requests minus the ones a style is answering on its own, then the pane's Antigravity card; a
/// confirmed lost sign-in, never a raw auth failure; and the raw run status, so a pane waiting on a permission is still "running".
pub fn project(
    sessions: &[&RunSession],
    workspaces: &[Workspace],
    permissions: &dyn Fn(&str) -> Vec<ToolPermissionRequest>,
    auto_allowed: &dyn Fn(&str, &ToolPermissionRequest) -> bool,
    antigravity: &dyn Fn(&str) -> Option<AntigravityCard>,
    sign_in_lost: &dyn Fn(&str) -> bool,
) -> Vec<PushPane> {
    let mut names: HashMap<&str, &str> = HashMap::new();
    for workspace in workspaces {
        names.entry(workspace.id.as_deref().unwrap_or("")).or_insert(workspace.name.as_deref().unwrap_or("Workspace"));
    }
    // `RemoteWorkspacePhone.Sessions`: the panes of this computer's workspaces only.
    let local: HashSet<&str> = workspaces.iter().filter(|w| w.remote.is_none()).map(|w| w.id.as_deref().unwrap_or("")).collect();
    sessions
        .iter()
        .filter(|s| local.contains(s.workspace_id.as_deref().unwrap_or("")))
        .filter(|s| !matches!(s.kind.as_deref().unwrap_or("claude"), "files" | "workmap"))
        .map(|session| {
            let id = session.id.as_deref().unwrap_or("");
            let mut requests: Vec<PushRequest> = permissions(id)
                .iter()
                .filter(|p| p.state == "pending" && !auto_allowed(id, p))
                .map(PushRequest::from_tool_request)
                .collect();
            if let Some(card) = antigravity(id) {
                requests.push(PushRequest::from_antigravity(&card));
            }
            // `MobileRemoteSupport.PreviewLine`: the newest line that is not a tool activity and has text.
            let preview = session
                .logs
                .as_deref()
                .unwrap_or_default()
                .iter()
                .rev()
                .find(|entry| entry.activity.is_none() && !entry.text.as_deref().unwrap_or("").is_empty())
                .map(|entry| clip(entry.text.as_deref().unwrap_or(""), PREVIEW_LIMIT));
            PushPane {
                session_id: id.to_string(),
                workspace: names.get(session.workspace_id.as_deref().unwrap_or("")).copied().unwrap_or("").to_string(),
                title: session.title.clone().unwrap_or_else(|| "Claude".into()),
                status: session.status.clone().unwrap_or_else(|| "idle".into()),
                preview,
                requests,
                sign_in_lost: sign_in_lost(id),
            }
        })
        .collect()
}

struct Memory {
    status: String,
    requests: HashSet<String>,
    sign_in_lost: bool,
}

/// Kept across observations a pane misses, so its ids keep counting up instead of repeating sent ones.
#[derive(Default)]
struct Count {
    runs: u32,
    signins: u32,
    seen: u64,
}

/// C# `MobilePushPlanner`: decides what a host pushes from what its panes show now and what they showed last time. One caller observes
/// at a time (the hub holds it under its planning lock).
#[derive(Default)]
pub struct Planner {
    baseline: bool,
    memory: HashMap<String, Memory>,
    counts: HashMap<String, Count>,
    observation: u64,
    sent: VecDeque<String>,
    sent_set: HashSet<String>,
}

impl Planner {
    pub fn new() -> Planner {
        Planner::default()
    }

    /// `Reset`: starts over from the next observation, which only sets the baseline: what changed while nothing was watched is not
    /// pushed. Run and sign-in numbers and the sent ids are kept, so an id already pushed is never reused.
    pub fn reset(&mut self) {
        self.baseline = false;
        self.memory.clear();
    }

    /// `Observe`: what to push for these panes.
    pub fn observe(&mut self, panes: &[PushPane], host_id: &str, host_name: &str, at: &str) -> Vec<PushMessage> {
        let mut messages = vec![];
        let mut next = HashMap::new();
        self.observation += 1;
        for pane in panes {
            let previous = self.memory.get(&pane.session_id);
            let count = self.counts.entry(pane.session_id.clone()).or_default();
            count.seen = self.observation;
            let mut emits: Vec<(String, String, String, Option<PushPermission>)> = vec![];
            if self.baseline {
                for request in &pane.requests {
                    let id = request_id(request);
                    if previous.is_some_and(|p| p.requests.contains(&id)) {
                        continue;
                    }
                    let permission = (request.kind == PERMISSION).then(|| PushPermission {
                        request_id: request.id.clone(),
                        run_id: request.run_id.clone(),
                        buttons: request.buttons.clone(),
                    });
                    emits.push((request.kind.clone(), id, request.summary.clone(), permission));
                }
                if previous.is_some_and(|p| p.status == "running")
                    && pane.requests.is_empty()
                    && (pane.status == "completed" || pane.status == "error")
                {
                    count.runs += 1;
                    let kind = if pane.status == "completed" { COMPLETED } else { FAILED };
                    emits.push((
                        kind.into(),
                        format!("run:{}:{}:{kind}", pane.session_id, count.runs),
                        pane.preview.clone().unwrap_or_default(),
                        None,
                    ));
                }
                if pane.sign_in_lost && !previous.is_some_and(|p| p.sign_in_lost) {
                    count.signins += 1;
                    emits.push((SIGNIN.into(), format!("signin:{}:{}", pane.session_id, count.signins), String::new(), None));
                }
            }
            for (kind, id, summary, permission) in emits {
                self.emit(&mut messages, pane, &kind, id, &summary, permission, host_id, host_name, at);
            }
            next.insert(
                pane.session_id.clone(),
                Memory {
                    status: pane.status.clone(),
                    requests: pane.requests.iter().map(request_id).collect(),
                    sign_in_lost: pane.sign_in_lost,
                },
            );
        }
        if self.counts.len() > COUNT_MEMORY {
            let mut absent: Vec<(String, u64)> =
                self.counts.iter().filter(|(_, c)| c.seen != self.observation).map(|(k, c)| (k.clone(), c.seen)).collect();
            absent.sort_by_key(|(_, seen)| *seen);
            let excess = self.counts.len() - COUNT_MEMORY;
            for (session, _) in absent.into_iter().take(excess) {
                self.counts.remove(&session);
            }
        }
        self.memory = next;
        self.baseline = true;
        messages
    }

    #[allow(clippy::too_many_arguments, reason = "C#'s Emit, argument for argument")]
    fn emit(
        &mut self,
        messages: &mut Vec<PushMessage>,
        pane: &PushPane,
        kind: &str,
        id: String,
        summary: &str,
        permission: Option<PushPermission>,
        host_id: &str,
        host_name: &str,
        at: &str,
    ) {
        if !self.sent_set.insert(id.clone()) {
            return;
        }
        self.sent.push_back(id.clone());
        if self.sent.len() > SENT_MEMORY
            && let Some(old) = self.sent.pop_front()
        {
            self.sent_set.remove(&old);
        }
        messages.push(PushMessage {
            v: 1,
            id,
            kind: kind.into(),
            host_id: host_id.into(),
            host_name: clip(host_name, 80),
            workspace: clip(&pane.workspace, 80),
            title: clip(&pane.title, 80),
            summary: clip(&one_line(summary), 200),
            session_id: pane.session_id.clone(),
            permission,
            at: at.into(),
        });
    }
}

fn request_id(request: &PushRequest) -> String {
    format!("request:{}:{}", request.run_id, request.id)
}

/// `MobilePushPlanner.Clip`: the first `limit` Unicode scalars.
pub fn clip(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// `MobilePushPlanner.OneLine`: line breaks become spaces, then spaces and tabs (Swift's `.whitespaces`: Zs and U+0009) are trimmed
/// from both ends.
pub fn one_line(text: &str) -> String {
    let line = text.replace("\r\n", " ").replace(['\n', '\r'], " ");
    let blank = |c: char| c == '\t' || is_space_separator(c);
    line.trim_matches(blank).to_string()
}

/// Unicode's Zs (space separators).
fn is_space_separator(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}
