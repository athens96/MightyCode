//! M-H: the Mac update against the Swift app's own code (`tools/parity-mac`, mac-plan.md §3 Step 0e): Rust's `install_script` text
//! equals Swift's `AppUpdateService.installScript` byte for byte for a table of inputs (relaunch on and off, `replacing` given, equal to
//! the destination or not, quotes and regex characters, a non-ASCII path, trailing slashes), and Rust's manifest parse reads the Mac's
//! fields as Swift's `AppUpdateManifest.parse` does (the `macos` asset under each of its names and tables, `minimumSystemVersion` and
//! `minOS`, what Swift ignores, a refused signature).
//!
//! CI's Mac leg builds the tool and names it in `MIGHTY_PARITY_MAC_TOOL` with `MIGHTY_PARITY_MAC=require`; elsewhere (Windows, a local
//! run without the tool) these checks say so and pass.

use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use mighty_updates::app_update::install_script::install_script;
use mighty_updates::app_update::manifest::{self, ENVELOPE_FORMAT};
use serde_json::{Value, json};

/// The tool, or None when it is not asked for (a run with `MIGHTY_PARITY_MAC=require` and no tool fails).
fn tool() -> Option<PathBuf> {
    match std::env::var_os("MIGHTY_PARITY_MAC_TOOL") {
        Some(path) => Some(PathBuf::from(path)),
        None if std::env::var("MIGHTY_PARITY_MAC").as_deref() == Ok("require") => {
            panic!("MIGHTY_PARITY_MAC=require without MIGHTY_PARITY_MAC_TOOL")
        }
        None => {
            eprintln!("tools/parity-mac not given (MIGHTY_PARITY_MAC_TOOL): Swift's side not compared");
            None
        }
    }
}

#[allow(clippy::disallowed_methods, reason = "the Swift oracle, run by the check itself")]
fn ask(tool: &Path, args: &[&str]) -> Value {
    let out = Command::new(tool).args(args).output().unwrap();
    assert!(out.status.success(), "parity-mac {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn temp(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let folder = std::env::temp_dir().join(format!("mighty-macparity-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

/// (name, staged app, destination, replacing, running executable, pid, relaunch).
type Case = (&'static str, &'static str, &'static str, Option<&'static str>, &'static str, i32, bool);

const CASES: [Case; 8] = [
    ("relaunch", "/tmp/updates/0.5.1/staged/MightyCode.app", "/Applications/MightyCode.app", None, "MightyCode", 42, true),
    ("on quit", "/tmp/updates/0.5.1/staged/MightyCode.app", "/Applications/MightyCode.app", None, "MightyCode", 42, false),
    (
        "renamed on quit",
        "/Users/me/Library/Application Support/MightyCode/updates/0.5.1/staged/MightyCode.app",
        "/Applications/MightyCode.app",
        Some("/Applications/MightyClaude.app"),
        "MightyClaude",
        7,
        false,
    ),
    (
        "renamed relaunch",
        "/tmp/u/staged/MightyCode.app",
        "/Users/me/Applications/MightyCode.app",
        Some("/Users/me/Applications/MightyClaude.app"),
        "MightyClaude",
        7,
        true,
    ),
    (
        "replacing itself",
        "/tmp/u/staged/MightyCode.app",
        "/Applications/MightyCode.app",
        Some("/Applications/./MightyCode.app/"),
        "MightyCode",
        9,
        false,
    ),
    (
        "quotes and regex characters",
        "/tmp/it's [x]/staged/MightyCode.app",
        "/Users/me/Apps (it's) [1] $x ^y {2} a+b?c|d*e\\f.app",
        None,
        "MightyCode",
        12345,
        false,
    ),
    ("non-ASCII", "/tmp/업데이트/staged/MightyCode.app", "/Users/나/응용 프로그램/Mïghty Cöde.app", None, "MightyCode", 99, false),
    ("trailing slashes", "/tmp/u/staged/MightyCode.app/", "/Applications/MightyCode.app/", None, "MightyCode", 1, true),
];

/// mac-plan.md §3 M-H "The install-script parity": Rust's text equals Swift's for every case.
#[test]
fn the_install_script_is_swifts_byte_for_byte() {
    let Some(tool) = tool() else { return };
    let folder = temp("script");
    let cases: Vec<Value> = CASES
        .iter()
        .map(|(name, staged, destination, replacing, running, pid, relaunch)| {
            let mut case = json!({"name": name, "stagedApp": staged, "destination": destination, "runningExecutable": running, "pid": pid, "relaunch": relaunch});
            if let Some(replacing) = replacing {
                case["replacing"] = json!(replacing);
            }
            case
        })
        .collect();
    let file = folder.join("cases.json");
    std::fs::write(&file, serde_json::to_vec(&json!({"cases": cases})).unwrap()).unwrap();
    let swift = ask(&tool, &["install-script", file.to_str().unwrap()]);
    let answered = swift["cases"].as_array().unwrap();
    assert_eq!(answered.len(), CASES.len());
    for ((name, staged, destination, replacing, running, pid, relaunch), swift) in CASES.iter().zip(answered) {
        assert_eq!(swift["name"], *name);
        let rust = install_script(Path::new(staged), Path::new(destination), replacing.map(Path::new), running, *pid, *relaunch);
        assert_eq!(rust, swift["script"].as_str().unwrap(), "case {name}");
    }
    let _ = std::fs::remove_dir_all(folder);
}

const SEED: [u8; 32] = [11; 32];

fn signed(plain: &str, seed: &[u8; 32]) -> Vec<u8> {
    let payload = plain.as_bytes();
    serde_json::to_vec(&json!({
        "format": ENVELOPE_FORMAT,
        "payload": STANDARD.encode(payload),
        "signature": STANDARD.encode(SigningKey::from_bytes(seed).sign(payload).to_bytes()),
    }))
    .unwrap()
}

/// mac-plan.md §3 M-H "The manifest parity": the `macos` asset under its fallbacks, `minimumSystemVersion`, and Windows' corpus
/// unchanged (tests/app_update_parity.rs).
#[test]
fn the_manifest_reads_the_mac_fields_as_swift_does() {
    let Some(tool) = tool() else { return };
    let folder = temp("manifest");
    let key = SigningKey::from_bytes(&SEED).verifying_key().to_bytes();
    let key_text = STANDARD.encode(key);
    let sha = "ab".repeat(32);
    let asset = |url: &str, size: &str| format!(r#"{{"url":"{url}","sha256":"{sha}","size":{size}}}"#);
    let good = asset("https://u.example/0.5.1/MightyCode-macos.zip", "26395344");
    let plains = vec![
        (
            "macos",
            format!(
                r#"{{"version":"v0.5.1","build":12,"notes":"n","publishedAt":"2026-11-16T00:00:00Z","minimumSystemVersion":"14.0","macos":{good}}}"#
            ),
        ),
        ("mac", format!(r#"{{"version":"0.5.1","mac":{good},"minOS":"14.2"}}"#)),
        ("darwin", format!(r#"{{"version":"0.5.1","darwin":{good}}}"#)),
        ("platforms", format!(r#"{{"version":"0.5.1","platforms":{{"macos":{good}}}}}"#)),
        ("downloads", format!(r#"{{"version":"0.5.1","downloads":{{"mac":{good}}}}}"#)),
        ("assets", format!(r#"{{"version":"0.5.1","assets":{{"darwin":{good}}}}}"#)),
        ("first name wins", format!(r#"{{"version":"0.5.1","macos":"https://u.example/a.zip","mac":{good}}}"#)),
        ("bare string", r#"{"version":"0.5.1","macos":"https://u.example/a.zip"}"#.to_string()),
        ("http", format!(r#"{{"version":"0.5.1","macos":{}}}"#, asset("http://u.example/a.zip", "5"))),
        ("size as text", format!(r#"{{"version":"0.5.1","macos":{}}}"#, asset("https://u.example/a.zip", "\"5\""))),
        ("size zero", format!(r#"{{"version":"0.5.1","macos":{}}}"#, asset("https://u.example/a.zip", "0"))),
        ("no size", format!(r#"{{"version":"0.5.1","macos":{{"url":"https://u.example/a.zip","sha256":"{sha}"}}}}"#)),
        ("bad digest", r#"{"version":"0.5.1","macos":{"url":"https://u.example/a.zip","sha256":"xyz","size":5}}"#.to_string()),
        (
            "digest with prefix",
            format!(
                r#"{{"version":"0.5.1","macos":{{"url":"https://u.example/a.zip","sha256":" SHA256:{} ","size":5}}}}"#,
                sha.to_uppercase()
            ),
        ),
        ("minimum not text", r#"{"version":"0.5.1","minimumSystemVersion":14,"minOS":"14.2"}"#.to_string()),
        ("minimum not a version", r#"{"version":"0.5.1","minimumSystemVersion":"fourteen"}"#.to_string()),
        ("windows beside", format!(r#"{{"version":"0.5.1","macos":{good},"windows":{{"x64":{good}}}}}"#)),
    ];
    let mut cases: Vec<(String, Vec<u8>)> = plains.into_iter().map(|(name, plain)| (name.to_string(), signed(&plain, &SEED))).collect();
    cases.push(("another key".into(), signed(r#"{"version":"0.5.1"}"#, &[12; 32])));
    for (name, bytes) in cases {
        let file = folder.join("latest.json");
        std::fs::write(&file, &bytes).unwrap();
        let swift = ask(&tool, &["manifest", file.to_str().unwrap(), &key_text]);
        let rust = manifest::parse(&bytes, &key);
        match rust {
            Err(error) => assert_eq!(swift["verdict"], "refused", "{name}: Rust refused ({error}), Swift read {swift}"),
            Ok(parsed) => {
                assert_eq!(swift["verdict"], "ok", "{name}: Swift refused {swift}");
                assert_eq!(swift["version"], parsed.version.as_str(), "{name}");
                assert_eq!(swift["minimumSystemVersion"], json!(parsed.minimum_system_version), "{name}");
                let rust_mac = parsed.macos.map(|a| json!({"url": a.url, "sha256": a.sha256, "size": a.size}));
                assert_eq!(swift["macos"], rust_mac.unwrap_or(Value::Null), "{name}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(folder);
}
