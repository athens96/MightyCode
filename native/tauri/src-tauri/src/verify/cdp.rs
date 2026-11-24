//! MC-48: a small Chrome DevTools Protocol client over the browser's WebSocket (tokio-tungstenite over a TCP connection to
//! `127.0.0.1` only): calls answered by id, events kept in order until read, every wait bounded. [`check_page`] visits one address
//! in a new tab and reads what the browser saw there (console errors, exceptions, failed requests, the accessibility tree and a
//! screenshot) into mighty-delegation's [`PageFindings`]. MC-7's live preview keeps one open for a while ([`Cdp::send`] for a call whose
//! answer is not waited for, [`Cdp::closed`] once the browser went).

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use mighty_delegation::verify::PageFindings;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

/// An event: its tab's session (None for the browser's own), its method and its parameters.
pub type Event = (Option<String>, String, Value);

pub struct Cdp {
    socket: WebSocketStream<TcpStream>,
    next_id: u64,
    events: VecDeque<Event>,
    closed: bool,
}

/// The most events kept unread (a page that logs without end must not grow the memory).
const MAXIMUM_EVENTS: usize = 5_000;

impl Cdp {
    /// Connects to `ws_url`, which must be a `ws://127.0.0.1:<port>/…` or `ws://localhost:<port>/…` address.
    pub async fn connect(ws_url: &str, limit: Duration) -> Result<Cdp, String> {
        let parsed = url::Url::parse(ws_url).map_err(|_| "the browser's DevTools address cannot be read".to_string())?;
        let host = parsed.host_str().unwrap_or_default();
        if parsed.scheme() != "ws" || !matches!(host, "127.0.0.1" | "localhost") {
            return Err("the browser's DevTools address is not on this computer".into());
        }
        let port = parsed.port().ok_or_else(|| "the browser's DevTools address names no port".to_string())?;
        let connecting = async {
            let stream = TcpStream::connect(("127.0.0.1", port)).await.map_err(|e| e.to_string())?;
            let _ = stream.set_nodelay(true);
            let (socket, _) = tokio_tungstenite::client_async(ws_url, stream).await.map_err(|e| e.to_string())?;
            Ok::<_, String>(socket)
        };
        let socket = tokio::time::timeout(limit, connecting).await.map_err(|_| "the browser did not answer in time".to_string())??;
        Ok(Cdp { socket, next_id: 1, events: VecDeque::new(), closed: false })
    }

    /// Sends `method` (to the tab `session`, or the browser) and waits for its answer until `deadline`; events read meanwhile are
    /// kept. Err with the protocol's error message.
    pub async fn call(&mut self, session: Option<&str>, method: &str, params: Value, deadline: Instant) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = Value::String(session.to_string());
        }
        tokio::time::timeout_at(deadline, self.socket.send(Message::text(message.to_string())))
            .await
            .map_err(|_| format!("{method}: no answer in time"))?
            .map_err(|e| e.to_string())?;
        loop {
            let value = self.read(deadline).await.ok_or_else(|| format!("{method}: no answer in time"))??;
            if value["id"].as_u64() == Some(id) {
                if let Some(error) = value.get("error") {
                    return Err(format!("{method}: {}", error["message"].as_str().unwrap_or("failed")));
                }
                return Ok(value["result"].clone());
            }
            self.keep(value);
        }
    }

    /// MC-7: sends `method` without waiting for its answer (dropped when it comes): a navigation whose requests this client pauses
    /// would otherwise wait on itself.
    pub async fn send(&mut self, session: Option<&str>, method: &str, params: Value) -> Result<(), String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = Value::String(session.to_string());
        }
        let sending = self.socket.send(Message::text(message.to_string()));
        match tokio::time::timeout(Duration::from_secs(5), sending).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.closed = true;
                Err(error.to_string())
            }
            Err(_) => Err(format!("{method}: no answer in time")),
        }
    }

    /// Whether the browser closed the connection (or it broke).
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// The next event (kept or new) until `deadline`; None past it or when the connection closed.
    pub async fn next_event(&mut self, deadline: Instant) -> Option<Event> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Some(event);
            }
            match self.read(deadline).await {
                Some(Ok(value)) => self.keep(value),
                _ => return None,
            }
        }
    }

    fn keep(&mut self, value: Value) {
        let Some(method) = value["method"].as_str() else { return };
        if self.events.len() >= MAXIMUM_EVENTS {
            self.events.pop_front();
        }
        let session = value["sessionId"].as_str().map(str::to_string);
        self.events.push_back((session, method.to_string(), value["params"].clone()));
    }

    /// One JSON message until `deadline`: None past it, Err when the connection broke.
    async fn read(&mut self, deadline: Instant) -> Option<Result<Value, String>> {
        loop {
            if self.closed {
                return Some(Err("the browser closed the connection".into()));
            }
            let next = tokio::time::timeout_at(deadline, self.socket.next()).await.ok()?;
            match next {
                None => {
                    self.closed = true;
                    return Some(Err("the browser closed the connection".into()));
                }
                Some(Err(error)) => {
                    self.closed = true;
                    return Some(Err(error.to_string()));
                }
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<Value>(text.as_str()) {
                    Ok(value) => return Some(Ok(value)),
                    Err(_) => continue,
                },
                Some(Ok(Message::Close(_))) => {
                    self.closed = true;
                    return Some(Err("the browser closed the connection".into()));
                }
                Some(Ok(_)) => continue,
            }
        }
    }

    pub async fn close(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(1), self.socket.close(None)).await;
    }
}

/// How long one page may take: loading, then a moment for late errors.
#[derive(Debug, Clone, Copy)]
pub struct PageLimits {
    pub load: Duration,
    pub settle: Duration,
}

/// Visits `url` in a new tab of the browser behind `cdp` and reads what it saw. The tab is closed afterwards. A page that cannot be
/// visited says why in its `error`; the screenshot's PNG bytes come back beside the findings.
pub async fn check_page(cdp: &mut Cdp, url: &str, limits: PageLimits) -> (PageFindings, Option<Vec<u8>>) {
    let mut page = PageFindings { url: url.to_string(), ..PageFindings::default() };
    let deadline = Instant::now() + limits.load + limits.settle + Duration::from_secs(5);
    let short = || Instant::now() + Duration::from_secs(5);
    let target = match cdp.call(None, "Target.createTarget", json!({ "url": "about:blank" }), short()).await {
        Ok(created) => created["targetId"].as_str().unwrap_or_default().to_string(),
        Err(error) => {
            page.error = Some(error);
            return (page, None);
        }
    };
    let session = match cdp.call(None, "Target.attachToTarget", json!({ "targetId": target, "flatten": true }), short()).await {
        Ok(attached) => attached["sessionId"].as_str().unwrap_or_default().to_string(),
        Err(error) => {
            page.error = Some(error);
            let _ = cdp.call(None, "Target.closeTarget", json!({ "targetId": target }), short()).await;
            return (page, None);
        }
    };
    let screenshot = visit(cdp, &session, &mut page, url, limits, deadline).await;
    let _ = cdp.call(None, "Target.closeTarget", json!({ "targetId": target }), short()).await;
    (page, screenshot)
}

async fn visit(cdp: &mut Cdp, session: &str, page: &mut PageFindings, url: &str, limits: PageLimits, deadline: Instant) -> Option<Vec<u8>> {
    let s = Some(session);
    for method in ["Runtime.enable", "Log.enable", "Network.enable", "Page.enable"] {
        if let Err(error) = cdp.call(s, method, json!({}), deadline).await {
            page.error = Some(error);
            return None;
        }
    }
    let mut requests: HashMap<String, (String, String)> = HashMap::new();
    match cdp.call(s, "Page.navigate", json!({ "url": url }), deadline).await {
        Ok(answer) => {
            if let Some(error) = answer["errorText"].as_str().filter(|e| !e.is_empty()) {
                page.error = Some(format!("{error} ({url})"));
            }
        }
        Err(error) => {
            page.error = Some(error);
            return None;
        }
    }
    // The load, then a moment for what the page does after it.
    let load_by = (Instant::now() + limits.load).min(deadline);
    let mut settle_by: Option<Instant> = None;
    loop {
        let until = settle_by.unwrap_or(load_by);
        let Some((from, method, params)) = cdp.next_event(until).await else {
            if settle_by.is_none() && !page.loaded && page.error.is_none() {
                page.error = Some(format!("the page did not finish loading within {} s", limits.load.as_secs()));
            }
            break;
        };
        if from.as_deref() != Some(session) {
            continue;
        }
        if method == "Page.loadEventFired" {
            page.loaded = true;
            settle_by = Some((Instant::now() + limits.settle).min(deadline));
            continue;
        }
        page.observe(&method, &params, &mut requests);
    }
    if let Ok(tree) = cdp.call(s, "Accessibility.getFullAXTree", json!({}), deadline).await {
        page.read_accessibility(tree["nodes"].as_array().map(Vec::as_slice).unwrap_or_default());
    }
    let shot = cdp.call(s, "Page.captureScreenshot", json!({ "format": "png", "captureBeyondViewport": false }), deadline).await.ok()?;
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(shot["data"].as_str()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A fake browser: answers each call it knows, and on `Page.navigate` sends the page's events before the load.
    async fn fake_browser(listener: TcpListener, hang_on_navigate: bool) {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = socket.next().await {
            let Message::Text(text) = message else { continue };
            let call: Value = serde_json::from_str(text.as_str()).unwrap();
            let id = call["id"].clone();
            let session = call["sessionId"].clone();
            let method = call["method"].as_str().unwrap_or_default().to_string();
            let event = |method: &str, params: Value| json!({ "method": method, "params": params, "sessionId": "S1" }).to_string();
            let result = match method.as_str() {
                "Target.createTarget" => json!({ "targetId": "T1" }),
                "Target.attachToTarget" => json!({ "sessionId": "S1" }),
                "Page.navigate" => {
                    assert_eq!(session, "S1");
                    let url = call["params"]["url"].as_str().unwrap().to_string();
                    socket.send(Message::text(json!({"id": id, "result": {"frameId": "F"}}).to_string())).await.unwrap();
                    if hang_on_navigate {
                        continue;
                    }
                    // Another tab's event is not this page's.
                    let other = json!({"method": "Runtime.exceptionThrown", "params": {"exceptionDetails": {"text": "elsewhere"}}, "sessionId": "S2"});
                    socket.send(Message::text(other.to_string())).await.unwrap();
                    for (m, p) in [
                        ("Network.requestWillBeSent", json!({"requestId": "1", "request": {"method": "GET", "url": url}})),
                        (
                            "Network.responseReceived",
                            json!({"requestId": "1", "type": "Document", "response": {"status": 200, "url": url}}),
                        ),
                        ("Runtime.consoleAPICalled", json!({"type": "error", "args": [{"type": "string", "value": "boom"}]})),
                        (
                            "Network.requestWillBeSent",
                            json!({"requestId": "2", "request": {"method": "GET", "url": "http://127.0.0.1:1/api"}}),
                        ),
                        (
                            "Network.responseReceived",
                            json!({"requestId": "2", "type": "Fetch", "response": {"status": 404, "url": "http://127.0.0.1:1/api"}}),
                        ),
                        ("Page.loadEventFired", json!({"timestamp": 1})),
                        (
                            "Runtime.exceptionThrown",
                            json!({"exceptionDetails": {"text": "Uncaught", "exception": {"description": "Error: late"}}}),
                        ),
                    ] {
                        socket.send(Message::text(event(m, p))).await.unwrap();
                    }
                    continue;
                }
                "Accessibility.getFullAXTree" => json!({ "nodes": [
                    {"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "Fake"}},
                    {"nodeId": "2", "parentId": "1", "role": {"value": "button"}, "name": {"value": ""}},
                ]}),
                "Page.captureScreenshot" => json!({ "data": "iVBORw0KGgo=" }),
                "Unknown.method" => {
                    let reply = json!({"id": id, "error": {"code": -32601, "message": "'Unknown.method' wasn't found"}});
                    socket.send(Message::text(reply.to_string())).await.unwrap();
                    continue;
                }
                _ => json!({}),
            };
            if socket.send(Message::text(json!({"id": id, "result": result}).to_string())).await.is_err() {
                return;
            }
        }
    }

    async fn start(hang: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(fake_browser(listener, hang));
        format!("ws://127.0.0.1:{port}/devtools/browser/fake")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_page_is_visited_and_its_findings_and_screenshot_read() {
        let url = start(false).await;
        let mut cdp = Cdp::connect(&url, Duration::from_secs(5)).await.unwrap();
        let limits = PageLimits { load: Duration::from_secs(5), settle: Duration::from_millis(300) };
        let (page, shot) = check_page(&mut cdp, "http://127.0.0.1:4100/", limits).await;
        assert!(page.loaded && page.error.is_none(), "{page:?}");
        assert_eq!(page.status, Some(200));
        assert_eq!(page.console_errors, ["boom"]);
        assert_eq!(page.exceptions, ["Error: late"], "an error just after the load is caught, another tab's is not");
        assert_eq!(page.failed_requests, ["404 GET http://127.0.0.1:1/api"]);
        assert_eq!(page.accessibility, ["RootWebArea \"Fake\"", "  button"]);
        assert_eq!(page.unnamed_controls, ["button"]);
        assert_eq!(shot.as_deref(), Some(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a][..]));

        let error = cdp.call(None, "Unknown.method", json!({}), Instant::now() + Duration::from_secs(2)).await.unwrap_err();
        assert!(error.contains("wasn't found"), "{error}");
        cdp.close().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_page_that_never_loads_ends_at_its_limit() {
        let url = start(true).await;
        let mut cdp = Cdp::connect(&url, Duration::from_secs(5)).await.unwrap();
        let began = std::time::Instant::now();
        let limits = PageLimits { load: Duration::from_millis(400), settle: Duration::from_millis(100) };
        let (page, _) = check_page(&mut cdp, "http://127.0.0.1:4100/", limits).await;
        assert!(!page.loaded);
        assert!(page.error.as_deref().is_some_and(|e| e.contains("did not finish loading")), "{page:?}");
        assert!(page.problems() > 0, "a page that does not load fails the check");
        assert!(began.elapsed() < Duration::from_secs(10));
    }

    /// MC-7: a call sent without waiting leaves its answer unread as no event, and a browser that goes is noticed.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_sent_call_is_not_waited_for_and_a_closed_browser_is_noticed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            if let Some(Ok(Message::Text(text))) = socket.next().await {
                let call: Value = serde_json::from_str(text.as_str()).unwrap();
                let answer = json!({ "id": call["id"], "result": {} });
                socket.send(Message::text(answer.to_string())).await.unwrap();
                let event = json!({ "method": "Page.loadEventFired", "params": {}, "sessionId": "S1" });
                socket.send(Message::text(event.to_string())).await.unwrap();
            }
            let _ = socket.close(None).await;
        });
        let mut cdp = Cdp::connect(&format!("ws://127.0.0.1:{port}/devtools/browser/x"), Duration::from_secs(5)).await.unwrap();
        cdp.send(Some("S1"), "Page.navigate", json!({ "url": "http://localhost:41000/" })).await.unwrap();
        let (session, method, _) = cdp.next_event(Instant::now() + Duration::from_secs(5)).await.unwrap();
        assert_eq!((session.as_deref(), method.as_str()), (Some("S1"), "Page.loadEventFired"), "the answer is no event");
        assert!(!cdp.closed());
        assert!(cdp.next_event(Instant::now() + Duration::from_secs(5)).await.is_none());
        assert!(cdp.closed(), "the browser went");
    }

    #[tokio::test]
    async fn only_a_devtools_address_on_this_computer_is_connected_to() {
        for refused in ["ws://example.com:9222/devtools/browser/x", "wss://127.0.0.1:9222/x", "http://127.0.0.1:9222/", "ws://127.0.0.1/x"]
        {
            assert!(Cdp::connect(refused, Duration::from_secs(1)).await.is_err(), "{refused}");
        }
    }
}
