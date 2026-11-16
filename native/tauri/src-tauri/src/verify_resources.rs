//! M-H: the exe mode `--verify-resources` (Swift `ResourceVerifier.swift:10-37` over `ResourceHealthChecker`; run by the Mac build,
//! notarize and install scripts, `scripts/build-macos.sh:199`, and M-I's `build-tauri-macos.sh`, and left out of 0.4.x's install-on-quit
//! running check, `AppUpdate.swift:344-351`). Served before anything else (main.rs `exe_mode`): no window, no Tauri, no profile.
//!
//! The checks, in Swift's order and names: each language's catalogue (`ko.json` …; embedded in this exe, so they are read, not searched:
//! a complete language's must hold keys), the default pet (`pets/mighty-raccoon`: a `pet.json` naming a `spritesheetPath`, beside the
//! exe, in the bundle's `Contents/Resources`, or — a development build — the repository's `assets/pets`), the help capture's two files
//! and the setup catalogue (embedded too: they must parse as JSON objects), then, for an exe inside an `.app`, Ghostty's runtime resources
//! and, when the bundle carries CEF, its framework, bridge and helpers. Each found resource prints `<name>: <path>` on stdout, each
//! missing one `<name>: MISSING` and the paths searched on stderr; the last line is `VERIFY_RESOURCES_OK` (exit 0) or
//! `VERIFY_RESOURCES_FAILED: missing <names>` on stderr (exit 1).
//!
//! Ghostty's resources (M-D's terminal, M-I's bundle): the Swift app's layout, `Contents/Resources/GhosttyKit_GhosttyTerminal.bundle`
//! holding `Ghostty` (shell integration) and `terminfo` (at its top or under its `Contents/Resources`), or Ghostty.app's own,
//! `Contents/Resources/ghostty` beside `Contents/Resources/terminfo`.

use std::path::{Path, PathBuf};

/// The argument (Swift's).
pub const ARGUMENT: &str = "--verify-resources";

/// The embedded files the help capture and the setup screen read (the same `include_str!` sources as help_capture.rs and mighty-tools).
const DEMO_PROFILE: &str = include_str!("../../../contracts/fixtures/demo-profile.json");
const SHOTS: &str = include_str!("../../../../docs/help/shots.json");
const SETUP_CATALOG: &str = include_str!("../../../contracts/fixtures/setup-catalog.json");

/// The languages whose catalogue must hold keys (Swift `AppLanguage.isComplete`); the others may still be empty.
const COMPLETE_LANGUAGES: [&str; 2] = ["ko", "en"];

/// The CEF parts a bundle that carries the framework must hold (scripts/build-macos.sh:111-178).
const CEF_FRAMEWORK: &str = "Chromium Embedded Framework.framework";
const CEF_PARTS: [&str; 6] = [
    "Chromium Embedded Framework.framework/Chromium Embedded Framework",
    "MightyCEFBridge.dylib",
    "MightyCode Helper.app",
    "MightyCode Helper (GPU).app",
    "MightyCode Helper (Renderer).app",
    "MightyCode Helper (Plugin).app",
];

/// One resource's verdict (Swift `ResourceCheckResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub resource: String,
    pub resolved: Option<String>,
    pub tried: Vec<String>,
}

fn shown(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// An embedded file that must be a JSON object.
fn embedded_object(resource: &str, text: &str) -> Check {
    let object = serde_json::from_str::<serde_json::Value>(text).is_ok_and(|value| value.is_object());
    Check { resource: resource.into(), resolved: object.then(|| "(embedded)".into()), tried: vec!["(embedded)".into()] }
}

/// Swift `ResourceHealthChecker.petDirectoryValid`: a `pet.json` object whose `spritesheetPath` is a non-empty string.
fn pet_valid(folder: &Path) -> bool {
    let manifest = std::fs::read(folder.join("pet.json")).ok().and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    manifest.as_ref().and_then(|m| m.get("spritesheetPath")).and_then(|p| p.as_str()).is_some_and(|p| !p.is_empty())
}

/// The first of `candidates` that `found` accepts.
fn first(resource: &str, candidates: Vec<PathBuf>, found: impl Fn(&Path) -> bool) -> Check {
    let resolved = candidates.iter().find(|path| found(path)).map(|path| shown(path));
    Check { resource: resource.into(), resolved, tried: candidates.iter().map(|path| shown(path)).collect() }
}

/// Every check for the exe at `exe` (`repository`: a development build's own checkout, whose `assets/pets` it may use).
pub fn checks(exe: &Path, repository: Option<&Path>) -> Vec<Check> {
    let mut checks: Vec<Check> = mighty_core::locale::LANGUAGES
        .iter()
        .map(|language| {
            let keys = !mighty_core::locale::catalogue(language).is_empty();
            let fine = keys || !COMPLETE_LANGUAGES.contains(language);
            Check { resource: format!("{language}.json"), resolved: fine.then(|| "(embedded)".into()), tried: vec!["(embedded)".into()] }
        })
        .collect();
    let folder = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let bundle = mighty_updates::app_update::stage_mac::bundle_of(exe);
    let resources = bundle.as_ref().map(|bundle| bundle.join("Contents").join("Resources"));
    let pet = mighty_companion::pet::DEFAULT_PET;
    let mut pets = vec![folder.join("pets").join(pet)];
    pets.extend(resources.iter().map(|resources| resources.join("pets").join(pet)));
    pets.extend(repository.map(|repository| repository.join("assets").join("pets").join(pet)));
    checks.push(first(&format!("pets/{pet}"), pets, pet_valid));
    checks.push(embedded_object("Help/demo-profile.json", DEMO_PROFILE));
    checks.push(embedded_object("Help/shots.json", SHOTS));
    checks.push(embedded_object("Setup/setup-catalog.json", SETUP_CATALOG));
    if let (Some(bundle), Some(resources)) = (&bundle, &resources) {
        let swift = resources.join("GhosttyKit_GhosttyTerminal.bundle");
        let candidates = vec![swift.clone(), swift.join("Contents").join("Resources"), resources.join("ghostty")];
        checks.push(first("Ghostty runtime resources", candidates, |place| {
            if place.ends_with("ghostty") {
                place.join("shell-integration").is_dir() && place.with_file_name("terminfo").is_dir()
            } else {
                place.join("Ghostty").is_dir() && place.join("terminfo").is_dir()
            }
        }));
        let frameworks = bundle.join("Contents").join("Frameworks");
        if frameworks.join(CEF_FRAMEWORK).exists() {
            for part in CEF_PARTS {
                let path = frameworks.join(part);
                checks.push(first(&format!("Frameworks/{part}"), vec![path], Path::exists));
            }
        }
    }
    checks
}

/// What the mode prints and its exit code: (stdout, stderr, code).
pub fn report(checks: &[Check]) -> (String, String, i32) {
    let (mut out, mut err) = (String::new(), String::new());
    for check in checks {
        match &check.resolved {
            Some(path) => out.push_str(&format!("{}: {path}\n", check.resource)),
            None => {
                let tried: Vec<String> = check.tried.iter().map(|path| format!("  {path}")).collect();
                err.push_str(&format!("{}: MISSING\n  searched:\n{}\n", check.resource, tried.join("\n")));
            }
        }
    }
    let missing: Vec<&str> = checks.iter().filter(|check| check.resolved.is_none()).map(|check| check.resource.as_str()).collect();
    if missing.is_empty() {
        out.push_str("VERIFY_RESOURCES_OK\n");
        (out, err, 0)
    } else {
        err.push_str(&format!("VERIFY_RESOURCES_FAILED: missing {}\n", missing.join(", ")));
        (out, err, 1)
    }
}

/// The mode when any argument is [`ARGUMENT`] (Swift's `CommandLine.arguments.contains`), with its exit code; None otherwise.
pub fn try_run(args: &[String]) -> Option<i32> {
    if !args.iter().any(|argument| argument == ARGUMENT) {
        return None;
    }
    #[cfg(debug_assertions)]
    let repository = Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".."));
    #[cfg(not(debug_assertions))]
    let repository: Option<PathBuf> = None;
    let exe = std::env::current_exe().unwrap_or_default();
    let (out, err, code) = report(&checks(&exe, repository.as_deref()));
    use std::io::Write as _;
    let _ = std::io::stdout().write_all(out.as_bytes());
    let _ = std::io::stderr().write_all(err.as_bytes());
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("mighty-verify-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Temp(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pet(folder: &Path) {
        std::fs::create_dir_all(folder).unwrap();
        std::fs::write(folder.join("pet.json"), r#"{"id":"mighty-raccoon","spritesheetPath":"spritesheet.webp"}"#).unwrap();
    }

    fn missing(checks: &[Check]) -> Vec<String> {
        checks.iter().filter(|check| check.resolved.is_none()).map(|check| check.resource.clone()).collect()
    }

    /// ResourceVerifier.swift:10-37: an exe outside a bundle needs its pet; the embedded files always resolve.
    #[test]
    fn a_plain_exe_needs_only_its_pet() {
        let root = Temp::new("plain");
        let exe = root.0.join("MightyCode.exe");
        let checks = checks(&exe, None);
        assert_eq!(missing(&checks), ["pets/mighty-raccoon"]);
        let (out, err, code) = report(&checks);
        assert_eq!(code, 1);
        assert!(out.contains("ko.json: (embedded)\n") && out.contains("Setup/setup-catalog.json: (embedded)\n"), "{out}");
        assert!(
            err.contains("pets/mighty-raccoon: MISSING\n  searched:\n  ")
                && err.ends_with("VERIFY_RESOURCES_FAILED: missing pets/mighty-raccoon\n")
        );
        pet(&root.0.join("pets").join("mighty-raccoon"));
        let (out, err, code) = report(&super::checks(&exe, None));
        assert_eq!((code, err.as_str()), (0, ""));
        assert!(out.ends_with("VERIFY_RESOURCES_OK\n"), "{out}");
    }

    /// A packaged Mac app: the pet in Contents/Resources, Ghostty's resources in either layout, CEF's parts when it carries CEF.
    #[test]
    fn a_bundle_needs_ghosttys_resources_and_all_of_cef_when_it_carries_it() {
        let root = Temp::new("bundle");
        let app = root.0.join("MightyCode.app");
        let exe = app.join("Contents").join("MacOS").join("MightyCode");
        let resources = app.join("Contents").join("Resources");
        pet(&resources.join("pets").join("mighty-raccoon"));
        assert_eq!(missing(&checks(&exe, None)), ["Ghostty runtime resources"]);
        let swift = resources.join("GhosttyKit_GhosttyTerminal.bundle");
        std::fs::create_dir_all(swift.join("Ghostty")).unwrap();
        assert_eq!(missing(&checks(&exe, None)), ["Ghostty runtime resources"], "terminfo too");
        std::fs::create_dir_all(swift.join("terminfo")).unwrap();
        assert!(missing(&checks(&exe, None)).is_empty());
        std::fs::remove_dir_all(&swift).unwrap();
        std::fs::create_dir_all(resources.join("ghostty").join("shell-integration")).unwrap();
        std::fs::create_dir_all(resources.join("terminfo")).unwrap();
        assert!(missing(&checks(&exe, None)).is_empty());

        let frameworks = app.join("Contents").join("Frameworks");
        std::fs::create_dir_all(frameworks.join(CEF_FRAMEWORK)).unwrap();
        assert_eq!(missing(&checks(&exe, None)).len(), CEF_PARTS.len());
        for part in CEF_PARTS {
            let path = frameworks.join(part);
            if part.ends_with(".app") {
                std::fs::create_dir_all(&path).unwrap();
            } else {
                std::fs::write(&path, "").unwrap();
            }
        }
        let (out, _, code) = report(&checks(&exe, None));
        assert_eq!(code, 0, "{out}");
    }

    #[test]
    fn the_mode_is_its_argument_anywhere() {
        assert_eq!(try_run(&["--profile".into(), "x".into()]), None);
    }
}
