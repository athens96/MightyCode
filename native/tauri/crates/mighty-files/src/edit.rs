//! Lightweight safe editing in the files pane (`MC-6`): atomic sibling-temp save preserving encoding, BOM, line endings and
//! permissions, plus SHA-256 disk-change checks so external edits (e.g. a running agent modifying the same file) surface a conflict
//! prompt instead of silently overwriting work.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::time::SystemTime;

use mighty_composer::text::{TextEncodingKind, decode_text};
use mighty_core::workspace_files;
use sha2::{Digest, Sha256};

use crate::classify;
use crate::revert::{encode, is_reparse_point, same_file};
use crate::tree::MAXIMUM_SOURCE_UNITS;
use crate::utf16;

/// Lowercase SHA-256 hex digest of raw file bytes on disk.
pub fn disk_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether the file on disk still matches the snapshot the editor loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskStatus {
    Unchanged { disk_hash: String },
    Modified { disk_hash: String },
    Missing,
}

/// Why `save` refused to overwrite a workspace file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveRefusal {
    /// Disk bytes changed since `expected_hash` was read (for instance, an agent edited the file).
    Conflict { disk_hash: String },
    /// Path escaped workspace, disappeared, or is a symlink/folder/reparse point.
    Missing,
    /// Existing file or edited text exceeds the files pane's editable text cap.
    TooLarge,
    /// Existing file's single-byte encoding (e.g. CP949) cannot represent every character in `text`.
    EncodingUnsupported { encoding: &'static str },
    /// Operating system I/O failure.
    Io(String),
}

/// Metadata of a completed atomic save.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveOutcome {
    pub disk_hash: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub encoding: TextEncodingKind,
    pub eol: &'static str,
}

/// Checks whether `relative` inside `root` still hashes to `expected_hash`.
pub fn check_disk(root: &Path, relative: &str, expected_hash: &str) -> DiskStatus {
    let Some(real) = workspace_files::resolve(relative, root) else {
        return DiskStatus::Missing;
    };
    let Ok(meta) = std::fs::symlink_metadata(&real) else {
        return DiskStatus::Missing;
    };
    if meta.file_type().is_symlink()
        || is_reparse_point(&meta)
        || !meta.file_type().is_file()
        || meta.len() > classify::MAXIMUM_TEXT_BYTES as u64
    {
        return DiskStatus::Missing;
    }
    let Ok(file) = std::fs::File::open(&real) else {
        return DiskStatus::Missing;
    };
    let Ok(opened_meta) = file.metadata() else {
        return DiskStatus::Missing;
    };
    if !opened_meta.file_type().is_file() || !same_file(&meta, &opened_meta) {
        return DiskStatus::Missing;
    }
    let mut bytes = Vec::new();
    if file.take(classify::MAXIMUM_TEXT_BYTES as u64 + 1).read_to_end(&mut bytes).is_err() || bytes.len() > classify::MAXIMUM_TEXT_BYTES {
        return DiskStatus::Missing;
    }
    let now_hash = disk_hash(&bytes);
    if now_hash == expected_hash { DiskStatus::Unchanged { disk_hash: now_hash } } else { DiskStatus::Modified { disk_hash: now_hash } }
}

/// Atomically writes `text` to the existing regular file `relative` under `root`, preserving its text encoding, BOM, line-ending
/// convention (`\r\n` vs `\n`) and permissions. When `force` is false and `expected_hash` differs from the file currently on disk,
/// returns `Err(SaveRefusal::Conflict)` without touching the file.
pub fn save(root: &Path, relative: &str, text: &str, expected_hash: Option<&str>, force: bool) -> Result<SaveOutcome, SaveRefusal> {
    if text.len() > classify::MAXIMUM_TEXT_BYTES || utf16::units(text).len() > MAXIMUM_SOURCE_UNITS {
        return Err(SaveRefusal::TooLarge);
    }
    let real = workspace_files::resolve(relative, root).ok_or(SaveRefusal::Missing)?;
    let meta = std::fs::symlink_metadata(&real).map_err(|_| SaveRefusal::Missing)?;
    if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.file_type().is_file() {
        return Err(SaveRefusal::Missing);
    }
    if meta.len() > classify::MAXIMUM_TEXT_BYTES as u64 {
        return Err(SaveRefusal::TooLarge);
    }
    let permissions = meta.permissions();
    let file = std::fs::File::open(&real).map_err(|e| SaveRefusal::Io(e.to_string()))?;
    let opened_meta = file.metadata().map_err(|e| SaveRefusal::Io(e.to_string()))?;
    if !opened_meta.file_type().is_file() || !same_file(&meta, &opened_meta) {
        return Err(SaveRefusal::Missing);
    }
    let mut current = Vec::new();
    file.take(classify::MAXIMUM_TEXT_BYTES as u64 + 1).read_to_end(&mut current).map_err(|e| SaveRefusal::Io(e.to_string()))?;
    if current.len() > classify::MAXIMUM_TEXT_BYTES {
        return Err(SaveRefusal::TooLarge);
    }
    let current_hash = disk_hash(&current);
    if !force
        && let Some(expected) = expected_hash.filter(|h| !h.is_empty())
        && current_hash != expected
    {
        return Err(SaveRefusal::Conflict { disk_hash: current_hash });
    }

    let decoded = decode_text(&current, false);
    let encoding = decoded.as_ref().map_or(TextEncodingKind::Utf8, |d| d.encoding);
    let use_crlf = decoded.as_ref().is_some_and(|d| d.text.contains("\r\n"));
    let normalized = text.replace("\r\n", "\n");
    let target_text = if use_crlf { normalized.replace('\n', "\r\n") } else { normalized };

    let unsupported = || SaveRefusal::EncodingUnsupported { encoding: classify::display_name(encoding) };
    let bytes = encode(&target_text, encoding).ok_or_else(unsupported)?;
    if bytes.len() > classify::MAXIMUM_TEXT_BYTES {
        return Err(SaveRefusal::TooLarge);
    }
    if !target_text.is_empty() && decode_text(&bytes, false).is_none_or(|d| d.text != target_text) {
        return Err(unsupported());
    }

    let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = real.with_file_name(format!(".{name}.{}.edit.tmp", uuid::Uuid::new_v4().simple()));
    let written = (|| -> std::io::Result<()> {
        let mut temp_file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        temp_file.write_all(&bytes)?;
        temp_file.sync_all()?;
        drop(temp_file);
        std::fs::set_permissions(&temporary, permissions)?;
        std::fs::rename(&temporary, &real)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(SaveRefusal::Io(error.to_string()));
    }

    let modified = std::fs::metadata(&real).ok().and_then(|m| m.modified().ok());
    Ok(SaveOutcome {
        disk_hash: disk_hash(&bytes),
        size: bytes.len() as u64,
        modified,
        encoding,
        eol: if use_crlf { "crlf" } else { "lf" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mighty-edit-{tag}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    #[test]
    fn save_preserves_crlf_and_detects_external_conflicts() {
        let root = temp_workspace("conflict");
        let file = root.join("src/app.ts");
        let initial = b"const a = 1;\r\nconst b = 2;\r\n";
        std::fs::write(&file, initial).unwrap();
        let initial_hash = disk_hash(initial);

        assert_eq!(check_disk(&root, "src/app.ts", &initial_hash), DiskStatus::Unchanged { disk_hash: initial_hash.clone() });

        let saved = save(&root, "src/app.ts", "const a = 10;\nconst b = 20;\n", Some(&initial_hash), false).unwrap();
        assert_eq!(saved.eol, "crlf");
        assert_eq!(std::fs::read(&file).unwrap(), b"const a = 10;\r\nconst b = 20;\r\n");

        // Simulate an external agent modifying the file behind the editor's back.
        let external = b"const a = 999;\r\n";
        std::fs::write(&file, external).unwrap();
        let external_hash = disk_hash(external);
        assert_eq!(check_disk(&root, "src/app.ts", &saved.disk_hash), DiskStatus::Modified { disk_hash: external_hash.clone() });

        // Saving with stale hash must refuse with Conflict and leave external content intact.
        let conflict = save(&root, "src/app.ts", "const a = 42;\n", Some(&saved.disk_hash), false);
        assert_eq!(conflict, Err(SaveRefusal::Conflict { disk_hash: external_hash }));
        assert_eq!(std::fs::read(&file).unwrap(), external);

        // Force save overwrites cleanly when user confirms.
        let forced = save(&root, "src/app.ts", "const a = 42;\n", Some(&saved.disk_hash), true).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"const a = 42;\r\n");
        assert_ne!(forced.disk_hash, saved.disk_hash);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_preserves_cp949_and_rejects_symlinks_or_traversal() {
        let root = temp_workspace("encoding-sec");
        let cp949_file = root.join("src/korean.txt");
        let (encoded, _, _) = encoding_rs::EUC_KR.encode("안녕 세상\n");
        std::fs::write(&cp949_file, encoded.as_ref()).unwrap();
        let h = disk_hash(encoded.as_ref());

        let ok = save(&root, "src/korean.txt", "반가워 세상\n", Some(&h), false).unwrap();
        assert_eq!(ok.encoding, TextEncodingKind::Cp949);
        let decoded = decode_text(&std::fs::read(&cp949_file).unwrap(), false).unwrap();
        assert_eq!(decoded.encoding, TextEncodingKind::Cp949);
        assert_eq!(decoded.text, "반가워 세상\n");

        // Unmappable emoji in CP949 returns EncodingUnsupported without corrupting the file.
        let bad = save(&root, "src/korean.txt", "이모지 🚀\n", Some(&ok.disk_hash), false);
        assert_eq!(bad, Err(SaveRefusal::EncodingUnsupported { encoding: "CP949 (EUC-KR)" }));
        assert_eq!(decode_text(&std::fs::read(&cp949_file).unwrap(), false).unwrap().text, "반가워 세상\n");

        // Path traversal is rejected.
        assert_eq!(save(&root, "../outside.txt", "x", None, true), Err(SaveRefusal::Missing));

        // Symlink inside workspace is rejected without modifying target.
        #[cfg(unix)]
        {
            let outside = std::env::temp_dir().join(format!("mighty-edit-outside-{}", uuid::Uuid::new_v4().simple()));
            std::fs::write(&outside, "secret\n").unwrap();
            let link = root.join("src/link.txt");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert_eq!(save(&root, "src/link.txt", "hacked\n", None, true), Err(SaveRefusal::Missing));
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret\n");
            let _ = std::fs::remove_file(&outside);
        }

        let _ = std::fs::remove_dir_all(&root);
    }
}
