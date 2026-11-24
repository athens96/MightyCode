//! MC-54: the diagnostics bundle's files and its zip. Pure but for reading the logs and writing the zip: what goes in is
//! [`Inputs`], every text passes the [`Redactor`], and `manifest.json` lists each file with what it holds and how many values each
//! redaction rule took out of it.
//!
//! - `manifest.json`: the list, the rules, what is never included, and that the bundle was sent nowhere;
//! - `system.json`: the app's version, the OS and architecture, and each agent CLI's installed version as the app last read it;
//! - `settings-summary.json`: the switches and counts of [`super::summary::settings_summary`];
//! - `logs/<name>`: the app's logs (`host.log` and its previous file, the daemon's, the start-up and performance logs), each its last
//!   [`MAX_LOG_BYTES`] cut at a line.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::redact::{RULES, Redactor, Tally};

/// The most of one log a bundle carries: its end, where the trouble usually is.
pub const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
pub const MANIFEST: &str = "manifest.json";
pub const FORMAT: u32 = 1;

/// One log a bundle may carry: its name in the zip (`logs/host.log`) and where it is.
#[derive(Debug, Clone)]
pub struct LogSource {
    pub name: String,
    pub path: PathBuf,
}

/// One agent CLI as the run engine last read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub version: Option<String>,
}

/// Everything a bundle is made of.
#[derive(Debug, Clone)]
pub struct Inputs {
    /// ISO 8601, UTC.
    pub created_at: String,
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub os_version: Option<String>,
    pub clis: Vec<Cli>,
    pub settings: Value,
    pub logs: Vec<LogSource>,
}

/// A bundle's files, in the zip's order (the manifest first), and what the redaction took out of all of them.
#[derive(Debug, Clone)]
pub struct Built {
    pub files: Vec<(String, Vec<u8>)>,
    pub tally: Tally,
}

/// What each file holds, in the manifest's words.
fn about(name: &str) -> &'static str {
    match name {
        MANIFEST => "This list: every file in the bundle, what it holds and how many values were redacted from it.",
        "system.json" => "The app's version, the OS and architecture, and each agent CLI's installed version.",
        "settings-summary.json" => "Which features are on and how many workspaces and panes there are; no names, paths or addresses.",
        "logs/host.log" => "The app's log (its last 2 MB).",
        "logs/host.log.1" => "The app's previous log (its last 2 MB).",
        "logs/daemon.log" => "The background daemon's log.",
        "logs/startup.log" => "The app's start-up stages.",
        "logs/perf.log" => "The app's performance measurements.",
        _ => "A log.",
    }
}

/// The last `max` bytes of the file at `path`, cut after its first line break when it was cut, as text; its whole size; whether it
/// was cut. None when there is no such file.
pub fn read_tail(path: &Path, max: u64) -> Option<(String, u64, bool)> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let cut = size > max;
    if cut {
        file.seek(SeekFrom::Start(size - max)).ok()?;
    }
    let mut bytes = Vec::with_capacity(size.min(max) as usize);
    file.take(max).read_to_end(&mut bytes).ok()?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if cut && let Some(at) = text.find('\n') {
        text.drain(..=at);
    }
    Some((text, size, cut))
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default() + "\n"
}

/// The bundle's files from `inputs`, each redacted.
pub fn build(inputs: &Inputs, redactor: &Redactor) -> Built {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut entries: Vec<Value> = Vec::new();
    let mut tally = Tally::default();
    let mut add = |name: &str, text: String, extra: Value| {
        let (clean, taken) = redactor.redact(&text);
        let mut entry = json!({ "name": name, "bytes": clean.len(), "contains": about(name), "redacted": taken.0 });
        if let (Some(entry), Some(extra)) = (entry.as_object_mut(), extra.as_object()) {
            entry.extend(extra.clone());
        }
        entries.push(entry);
        tally.merge(&taken);
        files.push((name.to_string(), clean.into_bytes()));
    };
    let system = json!({
        "app": { "version": inputs.app_version },
        "os": { "name": inputs.os, "arch": inputs.arch, "version": inputs.os_version },
        "clis": inputs.clis.iter().map(|cli| json!({
            "id": cli.id, "name": cli.name, "installed": cli.available, "version": cli.version,
        })).collect::<Vec<_>>(),
        "clisRead": !inputs.clis.is_empty(),
    });
    add("system.json", pretty(&system), json!({}));
    add("settings-summary.json", pretty(&inputs.settings), json!({}));
    let mut missing = Vec::new();
    for log in &inputs.logs {
        match read_tail(&log.path, MAX_LOG_BYTES) {
            Some((text, size, cut)) => add(&log.name, text, json!({ "originalBytes": size, "cut": cut })),
            None => missing.push(log.name.clone()),
        }
    }
    let manifest = json!({
        "format": FORMAT,
        "app": "Mighty Code",
        "appVersion": inputs.app_version,
        "createdAt": inputs.created_at,
        "sentAnywhere": false,
        "note": "Made on this computer and saved only where you chose. Mighty Code did not send it anywhere.",
        "files": entries,
        "notFound": missing,
        "redactionRules": RULES,
        "redactedTotal": tally.0,
        "neverIncluded": [
            "conversations, requests and replies",
            "file contents and diffs",
            "workspace and pane names, paths and addresses",
            "environment variables",
            "keys, tokens, passwords and pairing keys",
        ],
    });
    // The manifest says itself first; it is redacted as the rest (it holds nothing to take out).
    let (manifest, _) = redactor.redact(&pretty(&manifest));
    files.insert(0, (MANIFEST.to_string(), manifest.into_bytes()));
    Built { files, tally }
}

/// `built` written as a zip at `path`: into a file beside it first, moved in place once whole, so a failure leaves no half bundle.
pub fn write_zip(path: &Path, built: &Built) -> std::io::Result<()> {
    let staged = path.with_extension("zip.part");
    let result = (|| {
        let mut zip = zip::ZipWriter::new(File::create(&staged)?);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in &built.files {
            zip.start_file(name.as_str(), options).map_err(std::io::Error::other)?;
            zip.write_all(bytes)?;
        }
        zip.finish().map_err(std::io::Error::other)?.sync_all()
    })();
    match result {
        Ok(()) => std::fs::rename(&staged, path),
        Err(error) => {
            let _ = std::fs::remove_file(&staged);
            Err(error)
        }
    }
}

/// The name the save dialog suggests: `mighty-code-diagnostics-20261124-153000.zip` (local time).
pub fn suggested_name(now: chrono::DateTime<chrono::Local>) -> String {
    format!("mighty-code-diagnostics-{}.zip", now.format("%Y%m%d-%H%M%S"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mighty-diagnostics-{name}-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn inputs(logs: Vec<LogSource>) -> Inputs {
        Inputs {
            created_at: "2026-11-24T06:30:00Z".into(),
            app_version: "0.6.0+abc".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            os_version: Some("27.2.0".into()),
            clis: vec![
                Cli { id: "claude".into(), name: "Claude".into(), available: true, version: Some("2.1.0 (Claude Code)".into()) },
                Cli { id: "codex".into(), name: "Codex".into(), available: false, version: None },
            ],
            settings: json!({"workspaces": {"count": 2}}),
            logs,
        }
    }

    #[test]
    fn a_bundle_lists_every_file_and_carries_no_secret() {
        let dir = temp_dir("build");
        let token = "0123456789abcdef".repeat(4);
        let host_log = dir.join("host.log");
        std::fs::write(
            &host_log,
            format!(
                "2026-11-24T06:00:00Z opened /Users/alice/MightyCode\n\
                 2026-11-24T06:00:01Z relay Authorization: Bearer abcdefghijklmnop\n\
                 2026-11-24T06:00:02Z daemon token {token}\n\
                 2026-11-24T06:00:03Z pairingKey=q7Zx9abcdEFG\n"
            ),
        )
        .unwrap();
        let built = build(
            &inputs(vec![
                LogSource { name: "logs/host.log".into(), path: host_log },
                LogSource { name: "logs/daemon.log".into(), path: dir.join("missing.log") },
            ]),
            &Redactor::new(Some("/Users/alice")),
        );
        let names: Vec<&str> = built.files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec![MANIFEST, "system.json", "settings-summary.json", "logs/host.log"]);
        let log = String::from_utf8(built.files[3].1.clone()).unwrap();
        assert!(log.contains("opened ~/MightyCode"), "{log}");
        for secret in ["/Users/alice", "abcdefghijklmnop", token.as_str(), "q7Zx9abcdEFG"] {
            assert!(!log.contains(secret), "{secret} left in {log}");
        }
        let manifest: Value = serde_json::from_slice(&built.files[0].1).unwrap();
        assert_eq!(manifest["sentAnywhere"], false);
        assert_eq!(manifest["notFound"], json!(["logs/daemon.log"]));
        let files = manifest["files"].as_array().unwrap();
        assert_eq!(
            files.iter().map(|f| f["name"].as_str().unwrap()).collect::<Vec<_>>(),
            vec!["system.json", "settings-summary.json", "logs/host.log"]
        );
        let host = &files[2];
        assert_eq!(host["bytes"].as_u64().unwrap() as usize, log.len());
        assert_eq!(host["redacted"]["homePath"], 1);
        assert_eq!(host["redacted"]["authorizationHeader"], 1);
        assert_eq!(host["redacted"]["longHex"], 1);
        assert_eq!(host["redacted"]["secretFields"], 1);
        assert_eq!(manifest["redactionRules"].as_array().unwrap().len(), RULES.len());
        assert!(built.tally.total() >= 4);
        let system: Value = serde_json::from_slice(&built.files[1].1).unwrap();
        assert_eq!(system["clis"][0]["version"], "2.1.0 (Claude Code)");
        assert_eq!(system["clis"][1]["installed"], false);
        assert_eq!(system["os"]["arch"], "aarch64");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_long_log_keeps_its_end_from_a_whole_line() {
        let dir = temp_dir("tail");
        let path = dir.join("host.log");
        let lines: String = (0..1000).map(|n| format!("line {n:04}\n")).collect();
        std::fs::write(&path, &lines).unwrap();
        let (text, size, cut) = read_tail(&path, 100).unwrap();
        assert!(cut);
        assert_eq!(size, lines.len() as u64);
        assert!(text.starts_with("line ") && text.ends_with("line 0999\n"), "{text}");
        assert!(text.len() <= 100);
        let (whole, _, cut) = read_tail(&path, 1 << 20).unwrap();
        assert!(!cut);
        assert_eq!(whole, lines);
        assert!(read_tail(&dir.join("none"), 10).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_zip_holds_the_files_as_built() {
        let dir = temp_dir("zip");
        let built = build(&inputs(vec![]), &Redactor::new(None));
        let path = dir.join("bundle.zip");
        write_zip(&path, &built).unwrap();
        assert!(!dir.join("bundle.zip.part").exists());
        let mut archive = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
        assert_eq!(archive.len(), built.files.len());
        for (index, (name, bytes)) in built.files.iter().enumerate() {
            let mut entry = archive.by_index(index).unwrap();
            assert_eq!(entry.name().unwrap(), name.as_str());
            let mut read = Vec::new();
            entry.read_to_end(&mut read).unwrap();
            assert_eq!(&read, bytes);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_suggested_name_carries_the_local_time() {
        use chrono::TimeZone;
        let at = chrono::Local.with_ymd_and_hms(2026, 11, 24, 15, 30, 5).unwrap();
        assert_eq!(suggested_name(at), "mighty-code-diagnostics-20261124-153005.zip");
    }
}
