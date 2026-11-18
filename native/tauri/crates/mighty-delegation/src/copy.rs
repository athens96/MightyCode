//! `DelegationCopy` (C# Core/DelegationFileStore.cs:10-88; M/DelegationFileStore.swift:3-69): a stored copy of a child's TASK.md or
//! REPORT.md. A copy's text never takes more than [`MAXIMUM_COPY_BYTES`] inside the delegation file, its JSON escapes included: a
//! longer file is cut at a text-element boundary and ends with [`TRUNCATION_MARKER`], whose own escapes come on top, while the full file
//! stays in the worktree until cleanup. Owner: W7-A.

use std::io::Read as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::json;
use crate::request::delegation_names;

/// `DelegationFileStore.MaximumCopyBytes`: 64 KiB.
pub const MAXIMUM_COPY_BYTES: usize = 64 * 1024;

/// `DelegationCopy.TruncationMarker`.
pub const TRUNCATION_MARKER: &str = "\n\n[truncated at 64 KiB: the full file stays in the child's worktree until cleanup]\n";

/// `DelegationCopyKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DelegationCopyKind {
    Task,
    Report,
}

delegation_names!(serde DelegationCopyKind { Task => "task", Report => "report" });

/// `DelegationCopy`: `revision` is the report revision the copy holds (0 for TASK.md); `original_bytes` the whole file's size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationCopy {
    pub child_id: String,
    pub kind: DelegationCopyKind,
    pub revision: i64,
    pub text: String,
    pub original_bytes: i64,
    pub truncated: bool,
}

impl DelegationCopy {
    /// `Of`: the copy of a file whose bytes start with `contents`. Pass `total_bytes` when they hold only the start of a longer file.
    pub fn of(child_id: &str, kind: DelegationCopyKind, revision: i64, contents: &[u8], total_bytes: Option<i64>) -> DelegationCopy {
        let length = contents.len() as i64;
        let original = total_bytes.unwrap_or(length).max(length);
        let decoded = decode(&contents[..contents.len().min(MAXIMUM_COPY_BYTES + 1)]);
        let (text, truncated) = if original <= MAXIMUM_COPY_BYTES as i64 && stored_bytes(&decoded) <= MAXIMUM_COPY_BYTES {
            (decoded, false)
        } else {
            (format!("{}{TRUNCATION_MARKER}", start(&decoded, MAXIMUM_COPY_BYTES - TRUNCATION_MARKER.len())), true)
        };
        DelegationCopy { child_id: child_id.into(), kind, revision, text, original_bytes: original, truncated }
    }

    /// `Read`: at most a copy's worth of the file at `path`, so a huge file is never loaded whole.
    pub fn read(child_id: &str, kind: DelegationCopyKind, revision: i64, path: &Path) -> std::io::Result<DelegationCopy> {
        let file = std::fs::File::open(path)?;
        let total = file.metadata()?.len();
        let mut head = Vec::with_capacity(total.min(MAXIMUM_COPY_BYTES as u64 + 1) as usize);
        file.take(MAXIMUM_COPY_BYTES as u64 + 1).read_to_end(&mut head)?;
        Ok(DelegationCopy::of(child_id, kind, revision, &head, Some(total as i64)))
    }

    /// `TryRead`: [`DelegationCopy::read`], or None when the file is not there or cannot be read.
    pub fn try_read(child_id: &str, kind: DelegationCopyKind, revision: i64, path: &Path) -> Option<DelegationCopy> {
        DelegationCopy::read(child_id, kind, revision, path).ok()
    }
}

/// `Decode`: a file's text by its byte-order mark — UTF-16 little or big endian, or UTF-8 with or without its mark. Windows PowerShell
/// 5.1's `>` writes UTF-16, so a report an agent wrote that way still reads as its words. Malformed bytes read as U+FFFD, as .NET's
/// decoders give them.
pub fn decode(bytes: &[u8]) -> String {
    fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
        let units = bytes.as_chunks::<2>().0.iter().map(|pair| unit(*pair));
        let mut text: String = char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect();
        if bytes.len() % 2 == 1 {
            text.push(char::REPLACEMENT_CHARACTER);
        }
        text
    }
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// `StoredBytes`: the bytes `text` takes inside the delegation file, its escapes included.
pub fn stored_bytes(text: &str) -> usize {
    json::escaped_bytes(text)
}

/// `Start`: the longest whole-text-element start of `text` that takes at most `budget` bytes inside the delegation file.
pub fn start(text: &str, budget: usize) -> &str {
    // Escapes only add bytes, so the cut by UTF-8 bytes is the longest candidate.
    let ends: Vec<usize> = text.grapheme_indices(true).map(|(at, element)| at + element.len()).collect();
    let end = |count: usize| if count == 0 { 0 } else { ends[count - 1] };
    let count = ends.iter().take_while(|end| **end <= budget).count();
    if stored_bytes(&text[..end(count)]) <= budget {
        return &text[..end(count)];
    }
    // The first `fits` elements fit; the first `over` do not.
    let (mut fits, mut over) = (0, count);
    while over - fits > 1 {
        let middle = (fits + over) / 2;
        if stored_bytes(&text[..end(middle)]) <= budget {
            fits = middle;
        } else {
            over = middle;
        }
    }
    &text[..end(fits)]
}
