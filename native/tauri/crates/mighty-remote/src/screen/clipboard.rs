//! The clipboard over the control channel (C# R\ScreenClipboard.cs; plan §1.8): one transfer at a time, in chunks of 32 KiB, 1 MiB of
//! text at most, raw or zstd, with explicit compressed and plain limits. Decoding streams with a bounded zstd window (2^20) and never
//! trusts a declared size for an allocation. zstd is ruzstd's (pure Rust): it writes at its fastest level (about zstd's 1, where
//! C#'s ZstdSharp writes 3 — the bytes differ, any zstd decoder reads both) and reads every frame C#'s does. Owner: W8-F.

use std::io::Read as _;

use chrono::{DateTime, TimeDelta, Utc};
use mighty_core::json::{self, JsonOut};
use serde_json::Value;

use super::policy::{from_base64, to_base64};
use super::wire::int32;

/// `ScreenClipboard.MaximumBytes`: the most text one transfer carries, and the most compressed bytes it may take.
pub const MAXIMUM_BYTES: usize = 1_048_576;
const CHUNK: usize = 32_768;

/// C# `ScreenClipboard`: the phone's transfer being put together (`dir:"to-mac"` frames, in order, within 30 s).
#[derive(Default)]
pub struct ScreenClipboard {
    id: Option<String>,
    encoding: Option<String>,
    expected: usize,
    total: i32,
    next: i32,
    started: Option<DateTime<Utc>>,
    bytes: Option<Vec<u8>>,
}

impl std::fmt::Debug for ScreenClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenClipboard").field("next", &self.next).field("total", &self.total).finish_non_exhaustive()
    }
}

impl ScreenClipboard {
    /// `Receive`: the text once the last chunk of a whole, valid transfer came; none before, and none (the transfer dropped) for a
    /// frame out of order, a changed member, a transfer older than 30 s, bytes past the limits, bad base64 or zstd, or text that is
    /// not UTF-8.
    pub fn receive(&mut self, value: &Value, now: DateTime<Utc>) -> Option<String> {
        let incoming = json::text(value, "id").filter(|id| (1..=64).contains(&mighty_core::wire::utf16_len(id)));
        let encoding = json::text(value, "enc").filter(|enc| matches!(*enc, "raw" | "zstd"));
        let (Some(incoming), Some(encoding), Some(index), Some(count), Some(declared)) =
            (incoming, encoding, int32(value, "seq"), int32(value, "total"), int32(value, "bytes"))
        else {
            self.clear();
            return None;
        };
        let declared = usize::try_from(declared).unwrap_or(0);
        if json::text(value, "dir") != Some("to-mac")
            || !(1..=MAXIMUM_BYTES).contains(&declared)
            || !(1..=64).contains(&count)
            || index < 0
            || index >= count
        {
            self.clear();
            return None;
        }
        let expired = self.bytes.is_some() && self.started.is_some_and(|started| now - started > TimeDelta::seconds(30));
        if self.id.as_deref() != Some(incoming) || expired {
            self.clear();
            if index != 0 {
                return None;
            }
            self.id = Some(incoming.to_string());
            self.encoding = Some(encoding.to_string());
            self.expected = declared;
            self.total = count;
            self.started = Some(now);
            self.bytes = Some(Vec::new());
        }
        if self.bytes.is_none()
            || self.encoding.as_deref() != Some(encoding)
            || self.expected != declared
            || self.total != count
            || index != self.next
        {
            self.clear();
            return None;
        }
        let Some(chunk) = from_base64(json::text(value, "data").unwrap_or("")) else {
            self.clear();
            return None;
        };
        let bytes = self.bytes.as_mut().expect("a transfer is open");
        if bytes.len() + chunk.len() > MAXIMUM_BYTES {
            self.clear();
            return None;
        }
        bytes.extend_from_slice(&chunk);
        self.next += 1;
        if self.next != self.total {
            return None;
        }
        let packed = self.bytes.take().expect("a transfer is open");
        let plain = if self.encoding.as_deref() == Some("raw") { Some(packed) } else { unpack(&packed, self.expected) };
        let text = plain.filter(|plain| plain.len() == self.expected).and_then(|plain| String::from_utf8(plain).ok());
        self.clear();
        text
    }

    fn clear(&mut self) {
        *self = ScreenClipboard::default();
    }

    /// `Encode`: the host's clipboard as `dir:"to-phone"` frames — one `concealed` frame with no text for a concealed source (none),
    /// nothing for empty text or more than 1 MiB, else zstd when that is smaller, in base64 chunks of 32 KiB.
    pub fn encode(text: Option<&str>) -> Vec<String> {
        let id = mighty_core::wire::new_id();
        let Some(text) = text else {
            return vec![frame(&id, 0, 1, "raw", 0, "", true)];
        };
        let plain = text.as_bytes();
        if plain.is_empty() || plain.len() > MAXIMUM_BYTES {
            return Vec::new();
        }
        let packed = ruzstd::encoding::compress_to_vec(plain, ruzstd::encoding::CompressionLevel::Fastest);
        let (data, encoding) = if packed.len() < plain.len() { (packed.as_slice(), "zstd") } else { (plain, "raw") };
        let count = data.len().div_ceil(CHUNK);
        data.chunks(CHUNK).enumerate().map(|(seq, chunk)| frame(&id, seq, count, encoding, plain.len(), &to_base64(chunk), false)).collect()
    }
}

/// zstd with a window of at most 2^20 (ZstdSharp's `ZSTD_d_windowLogMax` 20), frame after frame (skippable ones skipped) as
/// `DecompressionStream` reads them, each frame's checksum compared when it has one, read 8 KiB at a time and stopped as soon as it
/// passes the declared size (or the limit).
fn unpack(mut packed: &[u8], expected: usize) -> Option<Vec<u8>> {
    use ruzstd::decoding::StreamingDecoder;
    use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    while !packed.is_empty() {
        let mut decoder = match StreamingDecoder::new_with_max_window_size(&mut packed, 1 << 20) {
            Ok(decoder) => decoder,
            Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::SkipFrame { length, .. })) => {
                packed = packed.get(usize::try_from(length).ok()?..)?;
                continue;
            }
            Err(_) => return None,
        };
        loop {
            let length = decoder.read(&mut buffer).ok()?;
            if length == 0 {
                break;
            }
            if output.len() + length > expected || output.len() + length > MAXIMUM_BYTES {
                return None;
            }
            output.extend_from_slice(&buffer[..length]);
        }
        let frame = &decoder.decoder;
        if !frame.is_finished() || frame.get_checksum_from_data().is_some_and(|sum| frame.get_calculated_checksum() != Some(sum)) {
            return None;
        }
    }
    Some(output)
}

fn frame(id: &str, seq: usize, total: usize, encoding: &str, bytes: usize, data: &str, concealed: bool) -> String {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_str("t", Some("clipboard"));
    out.prop_str("dir", Some("to-phone"));
    out.prop_str("id", Some(id));
    out.prop_int("seq", seq as i64);
    out.prop_int("total", total as i64);
    out.prop_str("enc", Some(encoding));
    out.prop_int("bytes", bytes as i64);
    out.prop_str("data", Some(data));
    if concealed {
        out.prop_bool("concealed", true);
    }
    out.end_object();
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::unpack;

    /// One frame holding `a` in a raw block, with the window descriptor byte and frame descriptor given.
    fn frame(descriptor: u8, window: u8, checksum: Option<[u8; 4]>) -> Vec<u8> {
        let mut bytes = vec![0x28, 0xB5, 0x2F, 0xFD, descriptor, window, 0x09, 0x00, 0x00, b'a'];
        if let Some(sum) = checksum {
            bytes.extend_from_slice(&sum);
        }
        bytes
    }

    #[test]
    fn frames_read_as_zstdsharp_reads_them() {
        assert_eq!(unpack(&frame(0, 0x50, None), 8), Some(b"a".to_vec()), "a window of 2^20 is read");
        assert_eq!(unpack(&frame(0, 0x58, None), 8), None, "a window of 2^21 is refused (windowLogMax 20)");
        assert_eq!(unpack(&[frame(0, 0x50, None), frame(0, 0x50, None)].concat(), 8), Some(b"aa".to_vec()), "frame after frame");
        let skipped = [vec![0x50, 0x2A, 0x4D, 0x18, 0x02, 0x00, 0x00, 0x00, 0xEE, 0xEE], frame(0, 0x50, None)].concat();
        assert_eq!(unpack(&skipped, 8), Some(b"a".to_vec()), "a skippable frame is skipped");
        assert_eq!(unpack(&frame(0x04, 0x50, Some([0, 0, 0, 0])), 8), None, "a wrong checksum is refused");
        assert_eq!(unpack(&frame(0, 0x50, None)[..8], 8), None, "a cut frame is refused");
        assert_eq!(unpack(&[frame(0, 0x50, None), frame(0, 0x50, None)].concat(), 1), None, "past the declared size");
    }
}
