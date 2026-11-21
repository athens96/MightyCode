//! A pairing relay on loopback port 0 (relay/src/protocol.ts's pairing, docs/relay.md "릴레이 와이어"): a host's control socket (its
//! `hostToken` hashing to its `serverId`, else 4401) hears `connected` for each client socket, the host's data socket of that connection
//! id is paired with the client's, and every frame then goes across as it came; a side that closes closes the other with its code. No
//! control socket: a client gets 4404. Unlike `fake_relay` (a scripted phone on the host's data socket), both ends here are real: the
//! host's `RelayHost` and the client's `RelayClient` or a test's own phone.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

pub type Socket = WebSocketStream<TcpStream>;

#[derive(Default)]
struct Hub {
    control: Option<mpsc::UnboundedSender<Message>>,
    waiting: HashMap<String, oneshot::Sender<Socket>>,
    /// Every client socket's close code as the relay sent it (the checks read what a phone was told).
    client_closes: Vec<u16>,
}

pub struct PairingRelay {
    pub url: String,
    hub: Arc<Mutex<Hub>>,
    task: JoinHandle<()>,
}

impl Drop for PairingRelay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn query_of(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.entry(key.to_string()).or_insert_with(|| value.to_string());
    }
    params
}

async fn close(mut socket: Socket, code: u16) {
    let _ = socket.send(Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: "".into() }))).await;
    let _ = tokio::time::timeout(Duration::from_secs(1), async { while socket.next().await.is_some() {} }).await;
}

impl PairingRelay {
    pub async fn start() -> PairingRelay {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let hub: Arc<Mutex<Hub>> = Arc::default();
        let serving = hub.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { return };
                tokio::spawn(accept(tcp, serving.clone()));
            }
        });
        PairingRelay { url, hub, task }
    }

    /// The codes the relay closed client sockets with, in order.
    pub fn client_closes(&self) -> Vec<u16> {
        self.hub.lock().unwrap().client_closes.clone()
    }

    /// Whether a host's control socket is attached now.
    pub fn host_attached(&self) -> bool {
        self.hub.lock().unwrap().control.as_ref().is_some_and(|c| !c.is_closed())
    }
}

async fn accept(tcp: TcpStream, hub: Arc<Mutex<Hub>>) {
    let seen: Arc<Mutex<String>> = Arc::default();
    let into = seen.clone();
    #[allow(clippy::result_large_err, reason = "tungstenite's own callback type")]
    let callback = move |request: &Request, response: Response| {
        *into.lock().unwrap() = request.uri().query().unwrap_or_default().to_string();
        Ok(response)
    };
    let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(tcp, callback).await else { return };
    let params = query_of(&seen.lock().unwrap());
    let role = params.get("role").cloned().unwrap_or_default();
    let server_id = params.get("serverId").cloned().unwrap_or_default();
    let connection = params.get("connectionId").cloned();
    if role == "server" {
        let token = match params.get("hostToken").cloned() {
            Some(token) => token,
            None => {
                let frame = tokio::time::timeout(Duration::from_secs(5), socket.next()).await;
                let Ok(Some(Ok(Message::Text(text)))) = frame else {
                    close(socket, 4401).await;
                    return;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(text.as_str()) else {
                    close(socket, 4400).await;
                    return;
                };
                if value.get("type").and_then(serde_json::Value::as_str) != Some("host-auth") {
                    close(socket, 4401).await;
                    return;
                }
                match value.get("hostToken").and_then(serde_json::Value::as_str) {
                    Some(token) => token.to_string(),
                    None => {
                        close(socket, 4401).await;
                        return;
                    }
                }
            }
        };
        let hashed: String = Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        if hashed != server_id {
            close(socket, 4401).await;
            return;
        }
        match connection {
            None => control(socket, hub).await,
            Some(id) => {
                let waiting = hub.lock().unwrap().waiting.remove(&id);
                match waiting {
                    Some(client) => {
                        let _ = client.send(socket);
                    }
                    None => close(socket, 4404).await,
                }
            }
        }
        return;
    }
    let Some(id) = connection else {
        close(socket, 4400).await;
        return;
    };
    let (attached, host) = oneshot::channel();
    let notice = {
        let mut hub = hub.lock().unwrap();
        match hub.control.clone().filter(|c| !c.is_closed()) {
            Some(control) => {
                hub.waiting.insert(id.clone(), attached);
                Some(control)
            }
            None => None,
        }
    };
    let Some(control) = notice else {
        hub.lock().unwrap().client_closes.push(4404);
        close(socket, 4404).await;
        return;
    };
    let _ = control.send(Message::text(format!(r#"{{"type":"connected","connectionId":"{id}"}}"#)));
    let host = match tokio::time::timeout(Duration::from_secs(10), host).await {
        Ok(Ok(host)) => host,
        _ => {
            hub.lock().unwrap().client_closes.push(4504);
            close(socket, 4504).await;
            return;
        }
    };
    let code = pipe(socket, host).await;
    hub.lock().unwrap().client_closes.push(code);
    let _ = control.send(Message::text(format!(r#"{{"type":"disconnected","connectionId":"{id}"}}"#)));
}

/// The host's control socket: notices written from the hub, its reads drained until it ends.
async fn control(socket: Socket, hub: Arc<Mutex<Hub>>) {
    let (mut sink, mut stream) = socket.split();
    let (notices, mut queue) = mpsc::unbounded_channel();
    hub.lock().unwrap().control = Some(notices);
    loop {
        tokio::select! {
            notice = queue.recv() => match notice {
                Some(notice) => if sink.send(notice).await.is_err() { break },
                None => break,
            },
            read = stream.next() => match read {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Both ends joined until one closes; the other is closed with its code, which is returned (1000 when none was given).
async fn pipe(client: Socket, host: Socket) -> u16 {
    let (mut client_sink, mut client_stream) = client.split();
    let (mut host_sink, mut host_stream) = host.split();
    let code = loop {
        tokio::select! {
            from_client = client_stream.next() => match from_client {
                Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                    if host_sink.send(message).await.is_err() { break 1000 }
                }
                Some(Ok(Message::Close(frame))) => {
                    let code = frame.map(|f| u16::from(f.code)).unwrap_or(1000);
                    let _ = host_sink.send(Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: "".into() }))).await;
                    break code;
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => {
                    let _ = host_sink.send(Message::Close(None)).await;
                    break 1000;
                }
            },
            from_host = host_stream.next() => match from_host {
                Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                    if client_sink.send(message).await.is_err() { break 1000 }
                }
                Some(Ok(Message::Close(frame))) => {
                    let code = frame.map(|f| u16::from(f.code)).unwrap_or(1000);
                    let _ = client_sink.send(Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: "".into() }))).await;
                    break code;
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => {
                    let _ = client_sink.send(Message::Close(None)).await;
                    break 1000;
                }
            },
        }
    };
    let _ = client_sink.close().await;
    let _ = host_sink.close().await;
    code
}
