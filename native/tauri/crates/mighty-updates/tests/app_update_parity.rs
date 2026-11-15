//! The app update against C# itself: each `native/contracts/fixtures/update-*` file holds fixed inputs and what MightyCode.Core answered
//! for them (`csharp`), written by `tools/parity/Update.cs` (`dotnet parity.dll update <mode> <fixture> <fixture>` writes it again).
//! Each Rust function must answer the same. The path-shaped cases (the plan's arguments, the mutex name, the staged packages) are
//! Windows paths and Windows' file search, read on Windows; the rest runs everywhere.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mighty_core::locale::Locale;
use mighty_updates::app_update::{UpdateError, dotnet, log, manifest, replacement, signature, version};
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures")
}

fn load(name: &str) -> Value {
    let path = fixtures().join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).expect("a JSON fixture")
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_else(|| panic!("a string: {value}"))
}

fn hex(value: &Value) -> Vec<u8> {
    let text = text(value);
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

fn ko() -> Locale {
    Locale::new("ko")
}

#[test]
fn ed25519_verdicts_match_csharp() {
    let corpus = load("update-ed25519.json");
    let cases = corpus["cases"].as_array().unwrap();
    assert!(cases.len() >= 40, "the corpus holds every hard case");
    for case in cases {
        let rust = signature::verify(&hex(&case["key"]), &hex(&case["message"]), &hex(&case["signature"]));
        assert_eq!(rust, case["csharp"].as_bool().unwrap(), "{}", case["name"]);
    }
}

/// C#'s refusal as the fixture keeps it, compared with Rust's.
fn assert_same_refusal(name: &str, rust: &UpdateError, csharp: &Value, rust_key: Option<&str>) {
    match text(&csharp["verdict"]) {
        "key" => {
            assert!(matches!(rust, UpdateError::Key { key, .. } if *key == text(&csharp["key"])), "{name}: C# {csharp}, Rust {rust:?}");
            assert_eq!(rust.message(&ko()), text(&csharp["message"]), "{name}: the sentence");
        }
        "exception" if text(&csharp["type"]).starts_with("Json") => {
            // .NET's JsonException names a line and a byte; the port answers the locale key the fixture names.
            let expected = rust_key.unwrap_or_else(|| panic!("{name}: a JsonException case names the port's key"));
            assert!(matches!(rust, UpdateError::Key { key, .. } if *key == expected), "{name}: Rust {rust:?}, expected {expected}");
        }
        "exception" => assert_eq!(rust, &UpdateError::Plain(text(&csharp["message"]).to_string()), "{name}: .NET's own text"),
        other => panic!("{name}: C# answered {other}"),
    }
}

#[test]
fn manifest_verdicts_match_csharp() {
    let cases = load("update-manifests/cases.json");
    let key = |name: &str| -> [u8; 32] { STANDARD.decode(text(&cases[name])).unwrap().try_into().unwrap() };
    let answers = cases["csharp"].as_array().unwrap();
    let inputs = cases["cases"].as_array().unwrap();
    assert_eq!(inputs.len(), answers.len());
    assert!(inputs.len() >= 80, "the corpus holds every hard case");
    for (input, csharp) in inputs.iter().zip(answers) {
        let name = text(&input["name"]);
        assert_eq!(text(&csharp["name"]), name);
        let data = std::fs::read(fixtures().join("update-manifests").join(text(&input["file"]))).unwrap();
        let rust = manifest::parse(&data, &key(input["with"].as_str().unwrap_or("fixture")));
        match (text(&csharp["verdict"]), rust) {
            ("ok", Ok(parsed)) => {
                assert_eq!(parsed.version, text(&csharp["version"]), "{name}: version");
                assert_eq!(parsed.build.map(i64::from), csharp["build"].as_i64(), "{name}: build");
                assert_eq!(parsed.notes.as_deref(), csharp["notes"].as_str(), "{name}: notes");
                assert_eq!(parsed.published_at.as_deref(), csharp["publishedAt"].as_str(), "{name}: publishedAt");
                let windows = csharp["windows"].as_object().unwrap();
                assert_eq!(parsed.windows.len(), windows.len(), "{name}: assets");
                for (architecture, asset) in windows {
                    let rust = &parsed.windows[architecture];
                    assert_eq!(rust.url, text(&asset["url"]), "{name}: {architecture} url");
                    assert_eq!(rust.sha256, text(&asset["sha256"]), "{name}: {architecture} sha256");
                    assert_eq!(rust.size, asset["size"].as_u64().unwrap(), "{name}: {architecture} size");
                }
            }
            ("ok", Err(error)) => panic!("{name}: C# read it, Rust refused it: {error:?}"),
            (_, Ok(parsed)) => panic!("{name}: C# refused it ({csharp}), Rust read {parsed:?}"),
            (_, Err(error)) => assert_same_refusal(name, &error, csharp, input["rust"].as_str()),
        }
    }
}

/// Addresses .NET's `Uri` repairs and accepts but the port refuses: a space inside (.NET escapes it before the request).
const STRICTER_ADDRESSES: [&str; 1] = ["https://updates.example/a b"];

#[test]
fn versions_and_addresses_match_csharp() {
    let fixture = load("update-versions.json");
    for case in fixture["normalize"].as_array().unwrap() {
        assert_eq!(version::normalized(text(&case["text"])).as_deref(), case["csharp"].as_str(), "normalize {:?}", case["text"]);
    }
    for case in fixture["compare"].as_array().unwrap() {
        let sign = version::compare(text(&case["left"]), text(&case["right"])) as i64;
        assert_eq!(sign, case["csharp"].as_i64().unwrap(), "compare {} {}", case["left"], case["right"]);
    }
    for case in fixture["allowed"].as_array().unwrap() {
        let url = text(&case["url"]);
        let expected = case["csharp"].as_bool().unwrap() && !STRICTER_ADDRESSES.contains(&url);
        assert_eq!(manifest::allowed(url), expected, "allowed {url}");
    }
}

#[test]
fn install_log_lines_match_csharp() {
    for case in load("update-install-log.json")["cases"].as_array().unwrap() {
        let flag = |name: &str| case[name].as_bool().unwrap();
        let result = replacement::ReplacementResult {
            replaced: flag("replaced"),
            rolled_back: flag("rolledBack"),
            error: case["error"].as_str().map(String::from),
            moves: Vec::new(),
            started: flag("started"),
            backup_removed: flag("backupRemoved"),
            skipped: flag("skipped"),
        };
        let now = dotnet::from_ticks(case["ticks"].as_i64().unwrap()).unwrap();
        let plan = mighty_updates::app_update::plan::InstallPlan {
            relaunch: flag("relaunch"),
            ..mighty_updates::app_update::plan::InstallPlan::create(
                Path::new("s"),
                Path::new("i"),
                Path::new("p"),
                "",
                1,
                1,
                Some(Path::new("b")),
                now,
            )
        };
        assert_eq!(log::line(&result, Some(&plan), now), text(&case["csharp"]), "{case}");
    }
}

/// The strict Ed25519 check refuses small-order points C# accepts: no update source may call it (plan D6).
#[test]
fn the_update_code_never_uses_the_strict_signature_check() {
    let strict = ["verify", "_strict"].concat();
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app_update");
    for entry in std::fs::read_dir(&folder).unwrap() {
        let path = entry.unwrap().path();
        assert!(!std::fs::read_to_string(&path).unwrap().contains(&strict), "{} calls the strict check", path.display());
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use mighty_updates::app_update::plan::{self, InstallPlan};
    use mighty_updates::app_update::{guard, stage};

    #[test]
    fn plan_arguments_match_csharp() {
        let fixture = load("update-plan.json");
        for case in fixture["create"].as_array().unwrap() {
            let path = |name: &str| PathBuf::from(text(&case[name]));
            let plan = InstallPlan::create(
                &path("staged"),
                &path("install"),
                &path("package"),
                text(&case["sha256"]),
                case["size"].as_i64().unwrap(),
                i32::try_from(case["pid"].as_i64().unwrap()).unwrap(),
                Some(&path("backup")),
                chrono::Utc::now(),
            );
            let plan = InstallPlan { relaunch: case["relaunch"].as_bool().unwrap(), ..plan };
            let expected: Vec<String> = case["csharp"].as_array().unwrap().iter().map(|a| text(a).to_string()).collect();
            assert_eq!(plan.helper_arguments(), expected, "{case}");
        }
        for case in fixture["parse"].as_array().unwrap() {
            let arguments: Vec<String> = case["arguments"].as_array().unwrap().iter().map(|a| text(a).to_string()).collect();
            let rust = plan::try_parse(&arguments);
            let csharp = &case["csharp"];
            match rust {
                None => assert!(csharp.is_null(), "C# made a plan of {arguments:?}: {csharp}"),
                Some(plan) => {
                    assert!(csharp.is_object(), "C# made no plan of {arguments:?}; Rust {plan:?}");
                    let path = |p: &Path| p.to_string_lossy().into_owned();
                    let pairs = [
                        ("staged", path(&plan.staged_directory)),
                        ("install", path(&plan.install_directory)),
                        ("backup", path(&plan.backup_directory)),
                        ("package", path(&plan.package_path)),
                        ("sha256", plan.sha256.clone()),
                        ("newExecutable", path(&plan.new_executable)),
                        ("previousExecutable", path(&plan.previous_executable)),
                        ("newDirectory", path(&plan.new_directory())),
                        ("stagedHelper", path(&plan.staged_helper_executable())),
                    ];
                    for (name, value) in pairs {
                        assert_eq!(value, text(&csharp[name]), "{name} of {arguments:?}");
                    }
                    assert_eq!(plan.size, csharp["size"].as_i64().unwrap(), "size of {arguments:?}");
                    assert_eq!(i64::from(plan.app_process_id), csharp["pid"].as_i64().unwrap(), "pid of {arguments:?}");
                    assert_eq!(plan.relaunch, csharp["relaunch"].as_bool().unwrap(), "relaunch of {arguments:?}");
                }
            }
        }
    }

    #[test]
    fn mutex_names_match_csharp() {
        for case in load("update-mutex.json")["paths"].as_array().unwrap() {
            assert_eq!(guard::mutex_name(Path::new(text(&case["path"]))), text(&case["csharp"]), "{}", case["path"]);
        }
    }

    /// Every file under `root`, relative with `/`, in C#'s ordinal order.
    fn files(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(folder) = pending.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    found.push(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        found.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        found
    }

    #[test]
    fn staging_matches_csharp() {
        let cases = load("update-stage/cases.json");
        for case in cases["cases"].as_array().unwrap() {
            let name = text(&case["name"]);
            let work = std::env::temp_dir().join(format!("mighty-parity-stage-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&work);
            std::fs::create_dir_all(&work).unwrap();
            let package = work.join("package.zip");
            std::fs::copy(fixtures().join("update-stage").join(text(&case["zip"])), &package).unwrap();
            let root = work.join("staged");
            let rust = stage::stage(&package, &root, text(&case["arch"]));
            let csharp = &case["csharp"];
            match (text(&csharp["verdict"]), rust) {
                ("ok", Ok(folder)) => {
                    let relative = folder.strip_prefix(dotnet::full_path(&root).unwrap()).unwrap().to_string_lossy().replace('\\', "/");
                    assert_eq!(
                        if relative.is_empty() { ".".to_string() } else { relative },
                        text(&csharp["folder"]),
                        "{name}: the app folder"
                    );
                    let expected: Vec<String> = csharp["files"].as_array().unwrap().iter().map(|f| text(f).to_string()).collect();
                    assert_eq!(files(&root), expected, "{name}: the files");
                }
                ("ok", Err(error)) => panic!("{name}: C# staged it, Rust refused it: {error:?}"),
                (_, Ok(folder)) => panic!("{name}: C# refused it ({csharp}), Rust staged {}", folder.display()),
                (_, Err(error)) => assert_same_refusal(name, &error, csharp, None),
            }
            std::fs::remove_dir_all(&work).unwrap();
        }
    }
}
