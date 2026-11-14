//! The direct listener's HTTP upgrade (C# `MobileDirectListener`'s head reader; plan §1.4): a head of at most 8192 bytes with nothing
//! after the blank line; `GET /ws[?…] HTTP/1.1`, `Upgrade: websocket`, `Connection` containing `Upgrade`, `Sec-WebSocket-Version:
//! 13`, a 16-byte base64 key; anything else gets no reply; the 101 carries `Sec-WebSocket-Accept`, no subprotocol and no extension;
//! then the stream is wrapped with `WebSocketStream::from_raw_socket(Role::Server)`. Owner: W8-C.
//!
//! At rest (W8 Step 0): the limit only.

pub const MAXIMUM_HEAD_BYTES: usize = 8192;
