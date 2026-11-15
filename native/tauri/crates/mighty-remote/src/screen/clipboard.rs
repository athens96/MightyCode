//! The clipboard over the control channel (C# R\ScreenClipboard.cs; plan §1.8): one transfer at a time, in chunks of 32 KiB, 1 MiB of
//! text at most, raw or zstd, with explicit compressed and plain limits. Decoding streams with a bounded zstd window (2^20) and never
//! trusts a declared size for an allocation. zstd is ruzstd's (pure Rust): it writes at its fastest level (about zstd's 1, where
//! C#'s ZstdSharp writes 3, so the bytes differ) and reads every frame C#'s does. ruzstd leaves the frame's content size out, which
//! the phone requires (`mobile/src/lib/screen-share/clipboard.ts` `zstdFrameContentSize` must equal `bytes`: `react-native-zstd`
//! allocates that much), so [`ScreenClipboard::encode`] writes the 4-byte `Frame_Content_Size` into ruzstd's header (RFC 8878
//! §3.1.1.1). Owner: W8-F.

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
        let plain = if self.encoding.as_deref() == Some("raw") { Some(packed) } else { unpack_guarded(&packed, self.expected) };
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
        let packed = pack(plain).filter(|packed| packed.len() < plain.len());
        let (data, encoding) = match &packed {
            Some(packed) => (packed.as_slice(), "zstd"),
            None => (plain, "raw"),
        };
        let count = data.len().div_ceil(CHUNK);
        data.chunks(CHUNK).enumerate().map(|(seq, chunk)| frame(&id, seq, count, encoding, plain.len(), &to_base64(chunk), false)).collect()
    }
}

const MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// One zstd frame of `plain` that declares its content size: ruzstd's frame (`Frame_Header_Descriptor` with no content size, no
/// single segment, no dictionary, maybe the checksum bit; then the window byte) with `Frame_Content_Size_flag` set to 2 and the size
/// as 4 little-endian bytes after the window byte. None (sent raw) for any other header shape, which a newer ruzstd might write.
fn pack(plain: &[u8]) -> Option<Vec<u8>> {
    let size = u32::try_from(plain.len()).ok()?;
    let mut frame =
        std::panic::catch_unwind(|| ruzstd::encoding::compress_to_vec(plain, ruzstd::encoding::CompressionLevel::Fastest)).ok()?;
    // Bits 7-6 Frame_Content_Size_flag, 5 Single_Segment_flag, 4 unused, 3 reserved, 2 Content_Checksum_flag, 1-0 Dictionary_ID_flag.
    if frame.len() < 6 || frame[..4] != MAGIC || frame[4] & !0x04 != 0 {
        return None;
    }
    frame[4] |= 0x80;
    frame.splice(6..6, size.to_le_bytes());
    (frame_content_size(&frame) == Some(u64::from(size))).then_some(frame)
}

/// The phone's `zstdFrameContentSize` (mobile/src/lib/screen-share/clipboard.ts): the content size a zstd frame's header declares,
/// or none when it has none (or is no zstd frame).
pub fn frame_content_size(frame: &[u8]) -> Option<u64> {
    if frame.len() < 6 || frame[..4] != MAGIC {
        return None;
    }
    let descriptor = frame[4];
    let fcs_flag = descriptor >> 6;
    let single_segment = (descriptor >> 5) & 1;
    let fcs_size = match fcs_flag {
        0 => usize::from(single_segment),
        1 => 2,
        2 => 4,
        _ => 8,
    };
    if fcs_size == 0 {
        return None;
    }
    let dict_size = [0, 1, 2, 4][usize::from(descriptor & 3)];
    let offset = 5 + usize::from(single_segment == 0) + dict_size;
    let bytes = frame.get(offset..offset + fcs_size)?;
    let value = bytes.iter().rev().fold(0u64, |value, byte| value * 256 + u64::from(*byte));
    Some(if fcs_size == 2 { value + 256 } else { value })
}

/// [`unpack`] with a panic in the decoder read as a refusal: no hostile frame takes the hub down.
fn unpack_guarded(packed: &[u8], expected: usize) -> Option<Vec<u8>> {
    std::panic::catch_unwind(|| unpack(packed, expected)).ok().flatten()
}

/// zstd with a window of at most 2^20 (ZstdSharp's `ZSTD_d_windowLogMax` 20), frame after frame (skippable ones skipped) as
/// `DecompressionStream` reads them, each frame's checksum compared when it has one, read 8 KiB at a time and stopped as soon as it
/// passes the declared size (or the limit); a frame that declares its content size must decode to exactly that many bytes, as
/// ZstdSharp (libzstd) checks.
fn unpack(mut packed: &[u8], expected: usize) -> Option<Vec<u8>> {
    use ruzstd::decoding::StreamingDecoder;
    use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    while !packed.is_empty() {
        let declared = frame_content_size(packed);
        let start = output.len();
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
        if declared.is_some_and(|declared| declared != (output.len() - start) as u64) {
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
    use super::{frame_content_size, pack, unpack, unpack_guarded};

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

    /// A single-segment frame (no window byte) whose 1-byte content size says `declared`, holding `a` in a raw block.
    fn sized(declared: u8) -> Vec<u8> {
        vec![0x28, 0xB5, 0x2F, 0xFD, 0x20, declared, 0x09, 0x00, 0x00, b'a']
    }

    #[test]
    fn a_declared_content_size_must_match() {
        assert_eq!(frame_content_size(&sized(1)), Some(1));
        assert_eq!(unpack(&sized(1), 8), Some(b"a".to_vec()), "the size it declares");
        assert_eq!(unpack(&sized(2), 8), None, "a frame that decodes to less than it declares is refused, as ZstdSharp refuses it");
    }

    /// The phone's reader over the header shapes RFC 8878 allows.
    #[test]
    fn the_phones_size_reader() {
        let header = |descriptor: u8, rest: &[u8]| [&[0x28, 0xB5, 0x2F, 0xFD, descriptor][..], rest].concat();
        assert_eq!(frame_content_size(&header(0x00, &[0x50])), None, "no size, a window");
        assert_eq!(frame_content_size(&header(0x20, &[7])), Some(7), "single segment, 1 byte");
        assert_eq!(frame_content_size(&header(0x40, &[0x50, 0x00, 0x01])), Some(256 + 256), "2 bytes after the window, plus 256");
        assert_eq!(frame_content_size(&header(0x80, &[0x50, 0x10, 0x27, 0, 0])), Some(10_000), "4 bytes after the window");
        assert_eq!(frame_content_size(&header(0xA0, &[0x10, 0x27, 0, 0])), Some(10_000), "C#'s shape: single segment, 4 bytes");
        assert_eq!(frame_content_size(&header(0x81, &[0x50, 0x09, 0x10, 0x27, 0, 0])), Some(10_000), "after a 1-byte dictionary id");
        assert_eq!(frame_content_size(&header(0xC0, &[0x50, 1, 0, 0, 0, 0, 0, 0, 0])), Some(1), "8 bytes");
        assert_eq!(frame_content_size(&header(0x80, &[0x50, 0x10])), None, "cut short");
        assert_eq!(frame_content_size(&[0x28, 0xB5, 0x2F, 0xFC, 0x20, 1]), None, "no zstd magic");
    }

    #[test]
    fn our_frames_declare_their_size_and_read_back() {
        for plain in [
            "x".repeat(50_000),
            "한글 clipboard
"
            .repeat(20_000),
            (0..40_000).map(|i| ((i * 7919 % 251) as u8 as char).to_string()).collect(),
        ] {
            let packed = pack(plain.as_bytes()).expect("ruzstd's header takes the size");
            assert_eq!(packed[4] & 0xE0, 0x80, "4-byte content size, no single segment");
            assert_eq!(frame_content_size(&packed), Some(plain.len() as u64), "the phone reads the size");
            assert_eq!(unpack(&packed, plain.len()).as_deref(), Some(plain.as_bytes()), "and ruzstd reads it back");
        }
    }

    /// Hostile frames: cut, flipped and random bytes are refused or read within their bound, and never take the caller down.
    #[test]
    fn hostile_frames_are_refused_without_a_panic() {
        let plain = "screen clipboard 화면 ".repeat(3000);
        let packed = pack(plain.as_bytes()).unwrap();
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..3000 {
            let mut hostile = packed.clone();
            match round % 3 {
                0 => hostile.truncate((next() as usize) % packed.len()),
                1 => {
                    for _ in 0..1 + next() % 4 {
                        let at = (next() as usize) % hostile.len();
                        hostile[at] ^= 1 << (next() % 8);
                    }
                }
                _ => {
                    let length = 6 + (next() as usize) % 200;
                    hostile = [&packed[..5], &(0..length).map(|_| next() as u8).collect::<Vec<_>>()[..]].concat();
                }
            }
            if let Some(read) = unpack_guarded(&hostile, plain.len()) {
                assert!(read.len() <= plain.len());
            }
        }
    }
}
