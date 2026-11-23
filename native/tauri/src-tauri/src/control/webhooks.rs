//! MC-9: webhooks (Settings › 자동화). An agent pane moving to a chosen state posts to each Slack, Discord or HTTPS address the person
//! added: a title, the state and the workspace's and pane's names, nothing else — never a prompt, an answer, a file or a token.
//!
//! - The addresses are secrets (a Slack or Discord address is its own key): kept only in the OS keychain (`remote::secret_vault`,
//!   item `automation.webhooks`), read once as the app starts, shown masked.
//! - Each post goes through the app's HTTPS port with a 10 s limit and is tried once more after a transport error, a 429 or a 5xx
//!   (after `Retry-After`, at most 10 s, else 2 s). The same pane and state within 5 s posts once; each address posts at most 6 times a
//!   minute (a burst of 6, refilled evenly).
//! - The test button posts a test note to one address, under the same rate limit.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use mighty_core::control::events::{ControlEvent, StateEvent};
use mighty_core::control::limit::{Debouncer, RateLimiter};
use mighty_core::locale::Locale;
use mighty_platform::http::{HttpsRequest, HttpsSend};
use serde_json::{Value, json};

use crate::host::Host;

/// The keychain item holding the addresses (a JSON list).
pub const SECRET_ID: &str = "automation.webhooks";
pub const MAX_WEBHOOKS: usize = 5;
const MAX_URL_CHARS: usize = 2_048;
const TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_AFTER: Duration = Duration::from_secs(2);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);
const DEBOUNCE: Duration = Duration::from_secs(5);
const RATE: u32 = 6;
const RATE_PER: Duration = Duration::from_secs(60);
/// The most of a name a post carries.
const MAX_NAME_CHARS: usize = 80;

/// Which service an address is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookKind {
    Slack,
    Discord,
    Generic,
}

impl WebhookKind {
    pub fn label(self) -> &'static str {
        match self {
            WebhookKind::Slack => "Slack",
            WebhookKind::Discord => "Discord",
            WebhookKind::Generic => "HTTPS",
        }
    }
}

/// Why an address was refused: the locale key that says so.
pub fn validate(text: &str) -> Result<String, &'static str> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_URL_CHARS || text.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("automation.webhooks.invalid");
    }
    let mut parsed = url::Url::parse(text).map_err(|_| "automation.webhooks.invalid")?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.host_str().is_none_or(str::is_empty)
    {
        return Err("automation.webhooks.invalid");
    }
    parsed.set_fragment(None);
    Ok(parsed.to_string())
}

pub fn kind_of(url: &str) -> WebhookKind {
    let Ok(parsed) = url::Url::parse(url) else { return WebhookKind::Generic };
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if host == "hooks.slack.com" {
        WebhookKind::Slack
    } else if matches!(host.as_str(), "discord.com" | "discordapp.com" | "ptb.discord.com" | "canary.discord.com")
        && parsed.path().starts_with("/api/webhooks/")
    {
        WebhookKind::Discord
    } else {
        WebhookKind::Generic
    }
}

/// An address as settings show it: its host and the last four characters of its path, the rest hidden.
pub fn masked(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else { return "…".into() };
    let host = parsed.host_str().unwrap_or("");
    let rest: String = format!("{}{}", parsed.path(), parsed.query().map(|q| format!("?{q}")).unwrap_or_default());
    let tail: String = rest.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    if rest.chars().count() <= 4 { format!("{host}{rest}") } else { format!("{host}/…{tail}") }
}

/// A name fit for a post: no control or formatting character, white space folded, at most 80 characters.
fn clean_name(text: &str) -> String {
    let cleaned = mighty_core::control::hooks::clean(text);
    if cleaned.chars().count() > MAX_NAME_CHARS {
        format!("{}…", cleaned.chars().take(MAX_NAME_CHARS - 1).collect::<String>())
    } else {
        cleaned
    }
}

/// Slack's control characters in text (`<!channel>`, `<@U…>`, links) as entities, so a name never mentions or links.
fn slack_text(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// What a post says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Note {
    pub title: String,
    /// The state's id (`done`, …; `test` for a test post) and its word.
    pub state: String,
    pub state_text: String,
    pub workspace: String,
    pub pane: String,
    pub agent: String,
    pub at: String,
}

/// The note of `event` in `locale`'s words.
pub fn note_of(locale: &Locale, event: &ControlEvent, state: StateEvent) -> Note {
    let state_text = event_word(locale, state);
    Note {
        title: locale.get_with("automation.webhook.title", &[("state", &state_text)]),
        state: state.name().to_string(),
        state_text,
        workspace: clean_name(&event.pane.workspace_name),
        pane: clean_name(&event.pane.title),
        agent: event.pane.agent.clone(),
        at: event.at.clone(),
    }
}

/// A state's word ("완료", "승인·질문 대기", …).
pub fn event_word(locale: &Locale, state: StateEvent) -> String {
    locale.get(match state {
        StateEvent::Started => "automation.event.started",
        StateEvent::Blocked => "automation.event.blocked",
        StateEvent::Done => "automation.event.done",
        StateEvent::Failed => "automation.event.failed",
        StateEvent::Stopped => "automation.event.stopped",
    })
}

/// The body posted to an address of `kind`: Slack's `text`, Discord's `content` (mentions off), or a plain JSON object.
pub fn payload(kind: WebhookKind, note: &Note) -> Value {
    let names = [note.workspace.as_str(), note.pane.as_str()].into_iter().filter(|n| !n.is_empty()).collect::<Vec<_>>().join(" · ");
    let text = if names.is_empty() { note.title.clone() } else { format!("{}\n{names}", note.title) };
    match kind {
        WebhookKind::Slack => json!({ "text": slack_text(&text) }),
        WebhookKind::Discord => json!({ "content": text, "allowed_mentions": { "parse": [] } }),
        WebhookKind::Generic => json!({
            "source": "mighty-code",
            "title": note.title,
            "state": note.state,
            "stateText": note.state_text,
            "workspace": note.workspace,
            "pane": note.pane,
            "agent": note.agent,
            "at": note.at,
        }),
    }
}

/// Posts `body` to `url`, once more after a transport error, a 429 or a 5xx; the status of a 2xx.
pub async fn post(https: &dyn HttpsSend, url: &str, body: &Value) -> Result<u16, String> {
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let request = || HttpsRequest {
        url: url.to_string(),
        headers: vec![("Content-Type".into(), "application/json".into()), ("User-Agent".into(), "MightyCode".into())],
        timeout: TIMEOUT,
    };
    let mut last = String::new();
    for attempt in 0..2 {
        let wait = match https.post(request(), bytes.clone(), 64 * 1024).await {
            Ok(answer) if (200..300).contains(&answer.status) => return Ok(answer.status),
            Ok(answer) if answer.status == 429 || answer.status >= 500 => {
                last = format!("HTTP {}", answer.status);
                answer
                    .retry_after
                    .as_deref()
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .map_or(RETRY_AFTER, |s| Duration::from_secs(s).min(MAX_RETRY_AFTER))
            }
            Ok(answer) => return Err(format!("HTTP {}", answer.status)),
            Err(error) => {
                last = error.to_string();
                RETRY_AFTER
            }
        };
        if attempt == 0 {
            tokio::time::sleep(wait).await;
        }
    }
    Err(last)
}

// ── the addresses, the limits and the settings' last message ───────────────────────────────────────────────────────────

#[derive(Default)]
struct State {
    /// None until read from the keychain.
    urls: Mutex<Option<Vec<String>>>,
    limiter: Mutex<Option<RateLimiter>>,
    debounce: Mutex<Option<Debouncer>>,
    /// What the last add, removal or test said (Settings shows it).
    message: Mutex<Option<String>>,
}

fn state() -> &'static State {
    static STATE: OnceLock<State> = OnceLock::new();
    STATE.get_or_init(State::default)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// The addresses as last read (empty until the keychain answered).
pub fn urls() -> Vec<String> {
    lock(&state().urls).clone().unwrap_or_default()
}

pub fn message() -> Option<String> {
    lock(&state().message).clone()
}

fn say(host: &Host, message: Option<String>) {
    *lock(&state().message) = message;
    crate::native::touch(host, crate::settings::SURFACE);
}

/// Reads the addresses from the keychain (off the async threads: a Keychain read may ask).
pub async fn load(host: &Arc<Host>) {
    let vault = crate::remote::secret_vault(host);
    let read = tauri::async_runtime::spawn_blocking(move || vault.load(SECRET_ID)).await.ok().flatten();
    let list: Vec<String> = read
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|url| validate(&url).ok())
        .take(MAX_WEBHOOKS)
        .collect();
    *lock(&state().urls) = Some(list);
    crate::native::touch(host, crate::settings::SURFACE);
}

async fn save(host: &Arc<Host>, list: Vec<String>) -> Result<(), String> {
    let vault = crate::remote::secret_vault(host);
    let text = serde_json::to_string(&list).map_err(|e| e.to_string())?;
    let empty = list.is_empty();
    tauri::async_runtime::spawn_blocking(move || if empty { vault.delete(SECRET_ID) } else { vault.save(SECRET_ID, &text) })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    *lock(&state().urls) = Some(list);
    Ok(())
}

/// Settings' 추가: the address checked, kept in the keychain.
pub async fn add(host: &Arc<Host>, typed: &str) -> Result<(), String> {
    let l = host.locale;
    let url = validate(typed).map_err(|key| l.get(key))?;
    let mut list = urls();
    if list.contains(&url) {
        return Ok(());
    }
    if list.len() >= MAX_WEBHOOKS {
        return Err(l.get_with("automation.webhooks.full", &[("count", &MAX_WEBHOOKS.to_string())]));
    }
    list.push(url);
    save(host, list).await
}

/// Settings' 삭제.
pub async fn remove(host: &Arc<Host>, index: usize) -> Result<(), String> {
    let mut list = urls();
    if index < list.len() {
        list.remove(index);
        save(host, list).await?;
    }
    Ok(())
}

/// Settings' 테스트: a test note to one address.
pub async fn test(host: &Arc<Host>, index: usize) {
    let l = host.locale;
    let Some(url) = urls().get(index).cloned() else { return };
    let note = Note {
        title: l.get("automation.webhook.testTitle"),
        state: "test".into(),
        state_text: l.get("automation.webhook.testTitle"),
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ..Note::default()
    };
    let said = match send(host, &url, &note).await {
        Ok(status) => l.get_with("automation.webhooks.sent", &[("status", &status.to_string())]),
        Err(reason) => l.get_with("automation.webhooks.failed", &[("reason", &reason)]),
    };
    say(host, Some(said));
}

/// Shows an add's or removal's failure in Settings (or clears the last message).
pub fn report(host: &Host, result: Result<(), String>) {
    say(host, result.err().map(|reason| host.locale.get_with("automation.webhooks.failed", &[("reason", &reason)])));
}

/// One post through the app's HTTPS port, under the address's rate limit.
async fn send(host: &Arc<Host>, url: &str, note: &Note) -> Result<u16, String> {
    let allowed = lock(&state().limiter).get_or_insert_with(|| RateLimiter::new(RATE, RATE_PER)).allow(url, Instant::now());
    if !allowed {
        return Err(host.locale.get("automation.webhooks.limited"));
    }
    let Some(ports) = tauri::Manager::try_state::<crate::platform::Ports>(&host.app) else { return Err("no HTTPS".into()) };
    let https = ports.https.clone();
    post(https.as_ref(), url, &payload(kind_of(url), note)).await
}

/// A bus event: an agent pane reaching a chosen state posts to every address (once per pane and state in 5 s).
pub fn on_event(host: &Arc<Host>, event: &ControlEvent, chosen: &[StateEvent]) {
    let Some(state_event) = event.state_event().filter(|e| chosen.contains(e)) else { return };
    if !event.pane.is_agent() || host.smoke.is_some() {
        return;
    }
    let list = urls();
    if list.is_empty() {
        return;
    }
    let key = format!("{}:{}", event.pane.pane, state_event.name());
    if !lock(&state().debounce).get_or_insert_with(|| Debouncer::new(DEBOUNCE)).allow(&key, Instant::now()) {
        return;
    }
    let note = note_of(&host.locale, event, state_event);
    for url in list {
        let (host, note) = (host.clone(), note.clone());
        tauri::async_runtime::spawn(async move {
            if let Err(reason) = send(&host, &url, &note).await {
                // The address is a secret: the log names its host only.
                host.log.line(&format!("webhook to {} not sent: {reason}", masked(&url)));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use mighty_core::control::events::{EventBody, PaneRef, PaneState};
    use mighty_platform::http::HttpsResponse;

    use super::*;

    #[test]
    fn addresses_are_https_only_and_shown_masked() {
        assert_eq!(
            validate(" https://hooks.slack.com/services/T0/B0/xyz123#frag ").unwrap(),
            "https://hooks.slack.com/services/T0/B0/xyz123"
        );
        for bad in [
            "http://hooks.slack.com/services/x",
            "https://user:pw@example.com/hook",
            "ftp://example.com",
            "https:// example.com",
            "not a url",
            "",
            "javascript:alert(1)",
        ] {
            assert_eq!(validate(bad), Err("automation.webhooks.invalid"), "{bad}");
        }
        assert_eq!(kind_of("https://hooks.slack.com/services/T0/B0/x"), WebhookKind::Slack);
        assert_eq!(kind_of("https://discord.com/api/webhooks/1/abc"), WebhookKind::Discord);
        assert_eq!(kind_of("https://discord.com/channels/1"), WebhookKind::Generic);
        assert_eq!(kind_of("https://example.com/hook"), WebhookKind::Generic);
        let shown = masked("https://hooks.slack.com/services/T0/B0/secretTOKEN9z");
        assert_eq!(shown, "hooks.slack.com/…EN9z");
        assert!(!shown.contains("secret"));
    }

    fn event(title: &str) -> ControlEvent {
        ControlEvent {
            seq: 1,
            at: "2026-11-23T10:00:00.000Z".into(),
            pane: PaneRef {
                pane: "p1".into(),
                workspace: "w1".into(),
                title: title.into(),
                workspace_name: "my-app".into(),
                agent: "claude".into(),
                kind: "agent".into(),
            },
            body: EventBody::State { from: PaneState::Running, to: PaneState::Done, event: Some(StateEvent::Done), reason: None },
        }
    }

    #[test]
    fn a_post_carries_the_title_state_and_names_only() {
        let locale = Locale::new("en");
        let note = note_of(&locale, &event("Fix <!channel> login\nnow"), StateEvent::Done);
        assert_eq!(note.pane, "Fix <!channel> login now");
        let slack = payload(WebhookKind::Slack, &note);
        assert_eq!(slack.as_object().unwrap().len(), 1);
        let text = slack["text"].as_str().unwrap();
        assert!(text.contains("&lt;!channel&gt;") && !text.contains("<!channel>"), "{text}");
        assert!(text.contains("my-app · Fix"));
        let discord = payload(WebhookKind::Discord, &note);
        assert_eq!(discord["allowed_mentions"]["parse"], json!([]));
        let generic = payload(WebhookKind::Generic, &note);
        let keys: Vec<&String> = generic.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["source", "title", "state", "stateText", "workspace", "pane", "agent", "at"]);
        assert_eq!(generic["state"], "done");
        // Nothing but the note goes: the event's ids are not in it, and a long name is cut.
        let long = note_of(&locale, &event(&"x".repeat(300)), StateEvent::Done);
        assert_eq!(long.pane.chars().count(), MAX_NAME_CHARS);
        for body in [slack, discord, generic] {
            let text = body.to_string();
            assert!(!text.contains("\"p1\"") && !text.contains("\"w1\""), "{text}");
        }
    }

    /// A scripted HTTPS port: its answers in order, each request's body kept.
    #[derive(Default)]
    struct Scripted {
        answers: Mutex<Vec<Result<HttpsResponse, String>>>,
        bodies: Mutex<Vec<Vec<u8>>>,
    }

    impl HttpsSend for Scripted {
        fn get(&self, _: HttpsRequest, _: usize) -> mighty_platform::BoxFuture<'_, mighty_platform::Result<HttpsResponse>> {
            Box::pin(async { Err(mighty_platform::PlatformError::Refused("no GET".into())) })
        }
        fn post(
            &self,
            request: HttpsRequest,
            body: Vec<u8>,
            _: usize,
        ) -> mighty_platform::BoxFuture<'_, mighty_platform::Result<HttpsResponse>> {
            assert!(request.headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"));
            assert_eq!(request.timeout, TIMEOUT);
            lock(&self.bodies).push(body);
            let answer = lock(&self.answers).remove(0);
            Box::pin(async move { answer.map_err(mighty_platform::PlatformError::Refused) })
        }
    }

    fn answer(status: u16) -> Result<HttpsResponse, String> {
        Ok(HttpsResponse { status, ..HttpsResponse::default() })
    }

    #[tokio::test(start_paused = true)]
    async fn a_post_is_tried_once_more_after_a_server_error_only() {
        let body = json!({ "text": "hi" });
        let https = Scripted { answers: Mutex::new(vec![answer(503), answer(200)]), ..Scripted::default() };
        assert_eq!(post(&https, "https://example.com/h", &body).await, Ok(200));
        assert_eq!(lock(&https.bodies).len(), 2);

        let https = Scripted { answers: Mutex::new(vec![Err("reset".into()), answer(500)]), ..Scripted::default() };
        assert_eq!(post(&https, "https://example.com/h", &body).await, Err("HTTP 500".into()), "tried twice at most");

        let https = Scripted { answers: Mutex::new(vec![answer(404)]), ..Scripted::default() };
        assert_eq!(post(&https, "https://example.com/h", &body).await, Err("HTTP 404".into()));
        assert_eq!(lock(&https.bodies).len(), 1, "a 4xx is not tried again");

        let https = Scripted {
            answers: Mutex::new(vec![
                Ok(HttpsResponse { status: 429, retry_after: Some("3".into()), ..HttpsResponse::default() }),
                answer(204),
            ]),
            ..Scripted::default()
        };
        let started = tokio::time::Instant::now();
        assert_eq!(post(&https, "https://example.com/h", &body).await, Ok(204));
        assert_eq!(started.elapsed(), Duration::from_secs(3), "after Retry-After");
    }
}
