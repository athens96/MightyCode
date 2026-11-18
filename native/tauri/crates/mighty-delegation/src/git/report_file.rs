//! `ChildReportFile` (C# Core/DelegationReports.cs:19-49; M/DelegationReports.swift:17-45): a child's REPORT.md as a run left it, the
//! start its copy keeps (at most 64 KiB + 1 bytes), its whole size and the SHA-256 of all of it, byte for byte. Owner: W7-B.

use std::io::Read;

use sha2::{Digest, Sha256};

/// `DelegationFileStore.MaximumCopyBytes + 1`: how much of the file a copy may need to tell it was cut.
const KEEP: usize = 64 * 1024 + 1;

/// `ChildReportFile(Start, TotalBytes, Digest)`: `digest` is the lowercase hex SHA-256 of every byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildReportFile {
    pub start: Vec<u8>,
    pub total_bytes: u64,
    pub digest: String,
}

/// `ChildReportFile.Read`: the report at `path`, or None when there is none: no regular file there (a link is not followed), it could
/// not be read, or the part a copy keeps holds no text (only white space and byte-order marks once decoded by its BOM). Reads the file
/// in pieces, so a huge one is never loaded whole; another program may still write or delete it meanwhile.
pub fn read(path: &str) -> Option<ChildReportFile> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut start = Vec::with_capacity(KEEP);
    let mut piece = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut piece).ok()?;
        if count == 0 {
            break;
        }
        hasher.update(&piece[..count]);
        total += count as u64;
        if start.len() < KEEP {
            let room = (KEEP - start.len()).min(count);
            start.extend_from_slice(&piece[..room]);
        }
    }
    let digest: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    decode(&start).chars().any(|c| !c.is_whitespace() && c != '\u{feff}').then_some(ChildReportFile { start, total_bytes: total, digest })
}

/// `DelegationCopy.Decode`: UTF-16 LE or BE by their byte-order mark, UTF-8 with or without one; what does not decode becomes U+FFFD.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let units = rest.chunks(2).map(|pair| if pair.len() == 2 { unit([pair[0], pair[1]]) } else { 0xFFFD });
        char::decode_utf16(units).map(|c| c.unwrap_or('\u{FFFD}')).collect()
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_reads_the_byte_order_mark() {
        let text = "# Report\n\nDone, \u{D55C}\u{AE00}.\n";
        let le: Vec<u8> = [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        let be: Vec<u8> = [0xFE, 0xFF].into_iter().chain(text.encode_utf16().flat_map(u16::to_be_bytes)).collect();
        let bom: Vec<u8> = [0xEF, 0xBB, 0xBF].into_iter().chain(text.bytes()).collect();
        for bytes in [le, be, bom, text.as_bytes().to_vec()] {
            assert_eq!(decode(&bytes), text);
        }
        assert_eq!(decode(&[0xFF, 0xFE, 0x41]), "\u{FFFD}");
    }
}
