//! The transport's framing (C# Core/AgentIO/AgentIOPipe.cs:167-179; M/AgentIOSocketServer.swift), chosen by mighty-platform's
//! `McpTransport`: Windows a 4-byte big-endian length then UTF-8 JSON (1..1,048,576; `0x7fffffff` refused), the Mac one JSON line.
//! Pure, tested on both OSes. Owner: W7-C.
//!
//! Also the JSON reading both ends share: System.Text.Json's `JsonDocument`/`JsonNode` read a string escape lazily, so a lone surrogate
//! escape parses and only reading that string fails (an `InvalidOperationException`, which each C# caller answers in its own way).
//! [`Lenient`] reproduces that over serde_json, which refuses such a document whole.

use std::io;

use mighty_core::json::{MAXIMUM_DEPTH, depth};
use mighty_platform::McpTransport;
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// `AgentIOPipe.MaximumBytes`: a packet's (and a stdio line's) largest size.
pub const MAXIMUM_BYTES: usize = 1_048_576;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// `AgentIOPipe.ReadPacket`: one message. Windows: the 4-byte big-endian size (1..1,048,576, else "Invalid tool packet size." before
/// anything is allocated), then that many bytes. The Mac: one line of at most 1 MiB, its `\n` dropped.
pub async fn read_packet<R: AsyncRead + Unpin>(stream: &mut R, transport: McpTransport) -> io::Result<Vec<u8>> {
    match transport {
        McpTransport::NamedPipe => {
            let mut prefix = [0u8; 4];
            stream.read_exact(&mut prefix).await?;
            let size = i32::from_be_bytes(prefix);
            if size < 1 || size as usize > MAXIMUM_BYTES {
                return Err(invalid("Invalid tool packet size."));
            }
            let mut bytes = vec![0u8; size as usize];
            stream.read_exact(&mut bytes).await?;
            Ok(bytes)
        }
        McpTransport::UnixSocket => {
            let mut reader = BufReader::new(stream.take(MAXIMUM_BYTES as u64 + 1));
            let mut bytes = Vec::new();
            reader.read_until(b'\n', &mut bytes).await?;
            if bytes.last() != Some(&b'\n') {
                return Err(if bytes.len() > MAXIMUM_BYTES {
                    invalid("Invalid tool packet size.")
                } else {
                    io::ErrorKind::UnexpectedEof.into()
                });
            }
            bytes.pop();
            if bytes.is_empty() {
                return Err(invalid("Invalid tool packet size."));
            }
            Ok(bytes)
        }
    }
}

/// The bytes [`write_packet`] sends for `bytes` (1..1,048,576, else "Tool response exceeds its limit.").
pub fn frame(bytes: &[u8], transport: McpTransport) -> io::Result<Vec<u8>> {
    if bytes.is_empty() || bytes.len() > MAXIMUM_BYTES {
        return Err(invalid("Tool response exceeds its limit."));
    }
    let mut framed = Vec::with_capacity(bytes.len() + 4);
    match transport {
        McpTransport::NamedPipe => {
            framed.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            framed.extend_from_slice(bytes);
        }
        McpTransport::UnixSocket => {
            framed.extend_from_slice(bytes);
            framed.push(b'\n');
        }
    }
    Ok(framed)
}

/// `AgentIOPipe.WritePacket`: one message, flushed.
pub async fn write_packet<W: AsyncWrite + Unpin>(stream: &mut W, bytes: &[u8], transport: McpTransport) -> io::Result<()> {
    let framed = frame(bytes, transport)?;
    stream.write_all(&framed).await?;
    stream.flush().await
}

/// What a lone surrogate escape reads as here: a noncharacter, U+FDD0 (a document already holding one is read strictly).
const UNREADABLE: char = '\u{FDD0}';

/// Reading a string that held a lone surrogate escape (C#'s `InvalidOperationException` when the string is decoded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreadable;

/// One JSON document as System.Text.Json reads it: syntax and depth (at most 64) checked up front, a lone surrogate escape only where
/// a string holding one is read ([`Lenient::text`], [`Lenient::keys`], [`Lenient::writable`]).
#[derive(Debug, Clone)]
pub struct Lenient {
    pub value: Value,
    lone: bool,
}

impl Lenient {
    /// `JsonDocument.Parse` / `JsonNode.Parse`: None where they throw a `JsonException`.
    pub fn parse(text: &str) -> Option<Lenient> {
        let (value, lone) = match serde_json::from_str::<Value>(text) {
            Ok(value) => (value, false),
            Err(_) => {
                let marked = mark_lone_surrogates(text)?;
                (serde_json::from_str::<Value>(&marked).ok()?, true)
            }
        };
        (depth(&value) <= MAXIMUM_DEPTH).then_some(Lenient { value, lone })
    }

    /// The same as [`Lenient::parse`] for bytes that must be UTF-8 (`JsonDocument.Parse(byte[])`).
    pub fn parse_bytes(bytes: &[u8]) -> Option<Lenient> {
        Lenient::parse(std::str::from_utf8(bytes).ok()?)
    }

    /// A string as C# decodes it (`GetString`, `GetValue<string>`): Err when it held a lone surrogate.
    pub fn read<'a>(&self, text: &'a str) -> Result<&'a str, Unreadable> {
        if self.lone && text.contains(UNREADABLE) { Err(Unreadable) } else { Ok(text) }
    }

    /// `value` as a string, if it is one (`JsonValueKind.String` then `GetString`).
    pub fn text<'a>(&self, value: Option<&'a Value>) -> Result<Option<&'a str>, Unreadable> {
        match value {
            Some(Value::String(text)) => self.read(text).map(Some),
            _ => Ok(None),
        }
    }

    /// An object's property names decoded, as `JsonObject` decodes them all when it is first indexed.
    pub fn keys(&self, map: &Map<String, Value>) -> Result<(), Unreadable> {
        map.keys().try_for_each(|key| self.read(key).map(|_| ()))
    }

    /// Whether `value` can be written again (`WriteTo` decodes every string and name in it).
    pub fn writable(&self, value: &Value) -> Result<(), Unreadable> {
        match value {
            Value::String(text) => self.read(text).map(|_| ()),
            Value::Array(items) => items.iter().try_for_each(|item| self.writable(item)),
            Value::Object(map) => {
                self.keys(map)?;
                map.values().try_for_each(|item| self.writable(item))
            }
            _ => Ok(()),
        }
    }
}

/// The JSON text with every escape of a surrogate that has no partner made the [`UNREADABLE`] mark, or None when there is none (or
/// the text already holds the mark, so it could not be told apart).
fn mark_lone_surrogates(text: &str) -> Option<String> {
    let mark = format!("\\u{:04x}", UNREADABLE as u32);
    if text.contains(UNREADABLE) || text.to_ascii_lowercase().contains(&mark) {
        return None;
    }
    let b = text.as_bytes();
    let unit = |at: usize| -> Option<u16> {
        (b.get(at) == Some(&b'\\') && b.get(at + 1) == Some(&b'u')).then_some(())?;
        u16::from_str_radix(text.get(at + 2..at + 6)?, 16).ok()
    };
    let mut out = String::with_capacity(text.len());
    let (mut at, mut kept, mut changed) = (0, 0, false);
    while at < b.len() {
        if b[at] != b'\\' {
            at += 1;
            continue;
        }
        match unit(at) {
            Some(0xD800..=0xDBFF) if unit(at + 6).is_some_and(|low| (0xDC00..=0xDFFF).contains(&low)) => at += 12,
            Some(0xD800..=0xDFFF) => {
                out.push_str(&text[kept..at]);
                out.push_str(&mark);
                changed = true;
                at += 6;
                kept = at;
            }
            _ => at += 2,
        }
    }
    out.push_str(&text[kept..]);
    changed.then_some(out)
}

/// The text of the top-level member `name` of an object as it was written in `text` (C# writes a number it read back as its
/// literal: `1.50` stays `1.50`), or None.
pub fn raw_member(text: &str, name: &str) -> Option<String> {
    let members: indexmap::IndexMap<String, Box<serde_json::value::RawValue>> = serde_json::from_str(text).ok()?;
    members.get(name).map(|raw| raw.get().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JSON `\uXXXX` escape (written so, never as the character).
    fn esc(hex: &str) -> String {
        format!("\\u{hex}")
    }

    /// The pipe's framing: a 4-byte big-endian size, then the bytes; a size out of 1..1 MiB is refused before anything is read.
    #[tokio::test]
    async fn the_pipe_frames_with_a_big_endian_size() {
        let framed = frame(b"{}", McpTransport::NamedPipe).unwrap();
        assert_eq!(framed, [0, 0, 0, 2, b'{', b'}']);
        assert_eq!(read_packet(&mut &framed[..], McpTransport::NamedPipe).await.unwrap(), b"{}");
        for header in [[0x7f, 0xff, 0xff, 0xff], [0, 0, 0, 0], [0xff, 0xff, 0xff, 0xff], [0, 0x10, 0, 1]] {
            let error = read_packet(&mut &header[..], McpTransport::NamedPipe).await.unwrap_err();
            assert_eq!(error.to_string(), "Invalid tool packet size.");
        }
        let largest = vec![b'x'; MAXIMUM_BYTES];
        let framed = frame(&largest, McpTransport::NamedPipe).unwrap();
        assert_eq!(&framed[..4], &[0, 0x10, 0, 0]);
        assert_eq!(read_packet(&mut &framed[..], McpTransport::NamedPipe).await.unwrap().len(), MAXIMUM_BYTES);
        assert!(frame(&[], McpTransport::NamedPipe).is_err() && frame(&vec![0; MAXIMUM_BYTES + 1], McpTransport::NamedPipe).is_err());
        // Cut short: the stream's end, not a packet.
        assert!(read_packet(&mut &[0u8, 0, 0, 5, b'{'][..], McpTransport::NamedPipe).await.is_err());
    }

    /// The socket's framing: one JSON line of at most 1 MiB.
    #[tokio::test]
    async fn the_socket_frames_one_line() {
        let framed = frame(b"{\"a\":1}", McpTransport::UnixSocket).unwrap();
        assert_eq!(framed, b"{\"a\":1}\n");
        assert_eq!(read_packet(&mut &framed[..], McpTransport::UnixSocket).await.unwrap(), b"{\"a\":1}");
        let mut long = vec![b'x'; MAXIMUM_BYTES + 1];
        long.push(b'\n');
        assert!(read_packet(&mut &long[..], McpTransport::UnixSocket).await.is_err());
        assert!(read_packet(&mut &b"{}"[..], McpTransport::UnixSocket).await.is_err(), "no line end");
        assert!(read_packet(&mut &b"\n"[..], McpTransport::UnixSocket).await.is_err(), "an empty line");
    }

    /// A lone surrogate parses and fails only where it is read; a pair is a character; a broken document or one deeper than 64 does
    /// not parse.
    #[test]
    fn a_lone_surrogate_fails_only_when_read() {
        let text = format!(r#"{{"token":"{}","ok":"x","pair":"{}{}"}}"#, esc("d800"), esc("d83d"), esc("de00"));
        let doc = Lenient::parse(&text).unwrap();
        assert_eq!(doc.text(doc.value.get("token")), Err(Unreadable));
        assert_eq!(doc.text(doc.value.get("ok")), Ok(Some("x")));
        assert_eq!(doc.text(doc.value.get("pair")), Ok(Some("\u{1F600}")));
        assert!(doc.writable(&doc.value).is_err() && doc.writable(&doc.value["ok"]).is_ok());
        let named = Lenient::parse(&format!(r#"{{"{}":1}}"#, esc("dc00"))).unwrap();
        assert!(named.keys(named.value.as_object().unwrap()).is_err());
        assert!(Lenient::parse(&format!(r#"{{"a":"{}"#, esc("d800"))).is_none());
        assert!(Lenient::parse("{,}").is_none());
        let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(Lenient::parse(&deep(64)).is_some() && Lenient::parse(&deep(65)).is_none());
        assert_eq!(raw_member(r#"{"id":1.50,"x":2}"#, "id").as_deref(), Some("1.50"));
    }
}
