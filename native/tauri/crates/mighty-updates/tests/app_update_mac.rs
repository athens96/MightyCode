//! M-H: the Mac update's checks, the Swift ones ported by name (`native/macos/Tests/MightyCoreTests/AppUpdateTests.swift`,
//! `AppUpdateAutoStepsTests.swift`), and each update field of the profile read at its site with both values.
//!
//! The decisions run on every OS over the fakes: the coordinator with the Mac's profile over `FakeUpdateTransport` (never the real update
//! address) and a stand-in stager; the install script's text; the checks before the install. What needs the Mac itself — `ditto`, the
//! real script under `/bin/sh`, BSD `pgrep`, `codesign` — runs on macOS only (`mod mac`), against fake apps in temporary folders, as the
//! Swift checks do; the scripts run there call `/usr/bin/true` in `lsregister`'s place, so no check touches LaunchServices (plan §3
//! rule 3: `lsregister` only in the disposable rehearsal job).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use mighty_core::locale::Locale;
use mighty_platform::PlatformProfile;
use mighty_platform::fake::{FakeRoute, FakeUpdateTransport};
use mighty_platform::profile::UpdatePackage;
use mighty_updates::app_update::coordinator::{Coordinator, Options, Stager, UpdateState};
use mighty_updates::app_update::install_script::{self, install_script};
use mighty_updates::app_update::manifest::{self, Availability, ENVELOPE_FORMAT};
use mighty_updates::app_update::plan::InstallPlan;
use mighty_updates::app_update::service::{MAC_PACKAGE_NAME, PACKAGE_NAME, STAGED_FOLDER, Service, ServiceError};
use mighty_updates::app_update::{Phase, UpdateError, auto, housekeeping, stage_mac};

/// A fresh temporary folder, deleted when dropped.
struct Temp(PathBuf);

impl Temp {
    fn new() -> Temp {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path =
            std::env::temp_dir().join(format!("mighty-macupdate-{}-{nanos}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&path).unwrap();
        // `/var` is `/private/var` on the Mac: the checks compare what the code under test resolves.
        Temp(std::fs::canonicalize(&path).unwrap_or(path))
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn en() -> Locale {
    Locale::new("en")
}

/// The checks' own key pair.
const SEED: [u8; 32] = [7; 32];

fn public_key() -> [u8; 32] {
    SigningKey::from_bytes(&SEED).verifying_key().to_bytes()
}

fn envelope(plain: &str) -> Vec<u8> {
    let payload = plain.as_bytes();
    serde_json::to_vec(&serde_json::json!({
        "format": ENVELOPE_FORMAT,
        "payload": STANDARD.encode(payload),
        "signature": STANDARD.encode(SigningKey::from_bytes(&SEED).sign(payload).to_bytes()),
    }))
    .unwrap()
}

const MANIFEST_URL: &str = "https://updates.example/latest.json";
const PACKAGE_URL: &str = "https://updates.example/MightyCode-macos.zip";

/// A Mac manifest: `version`, the Mac package (when `macos`) and the lowest macOS (when `minimum`).
fn manifest_text(version: &str, macos: bool, minimum: Option<&str>, package: &[u8]) -> String {
    let mut fields = vec![format!(r#""version":"{version}""#)];
    if macos {
        fields.push(format!(r#""macos":{{"url":"{PACKAGE_URL}","sha256":"{}","size":{}}}"#, sha256_of(package), package.len()));
    }
    if let Some(minimum) = minimum {
        fields.push(format!(r#""minimumSystemVersion":"{minimum}""#));
    }
    format!("{{{}}}", fields.join(","))
}

fn sha256_of(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The stand-in for `ditto`: the staging folder gets one `MightyCode.app`; the packages it was handed are recorded.
fn stand_in_stager(staged: Arc<Mutex<Vec<PathBuf>>>) -> Stager {
    Arc::new(move |package: PathBuf, destination: PathBuf| {
        let staged = staged.clone();
        Box::pin(async move {
            staged.lock().unwrap().push(package);
            let app = destination.join("MightyCode.app");
            std::fs::create_dir_all(&app).map_err(|e| UpdateError::Plain(e.to_string()))?;
            Ok(app)
        })
    })
}

struct Rig {
    transport: Arc<FakeUpdateTransport>,
    coordinator: Coordinator,
    staged: Arc<Mutex<Vec<PathBuf>>>,
    _root: Temp,
}

const PACKAGE: &[u8] = b"PK fake Mac package";

/// The coordinator with the Mac's profile on a Mac at 15.6.0 running `current`, the manifest `latest` answering.
fn rig(current: &str, latest: &str, automatic: bool) -> Rig {
    let root = Temp::new();
    let transport = Arc::new(FakeUpdateTransport::default());
    transport.route(MANIFEST_URL, FakeRoute::ok(envelope(latest)));
    transport.route(PACKAGE_URL, FakeRoute::ok(PACKAGE.to_vec()));
    let profile = PlatformProfile::macos();
    let service = Arc::new(Service::new(transport.clone(), root.path(), Some(public_key())).with_profile(&profile));
    let staged = Arc::new(Mutex::new(Vec::new()));
    let options = Options {
        built_in_manifest_url: Some(MANIFEST_URL.into()),
        locale: en(),
        profile,
        system_version: Some("15.6.0".into()),
        stage: Some(stand_in_stager(staged.clone())),
        staging_busy: Some(Arc::new(|| false)),
        automatic: Arc::new(move || automatic),
        ..Options::new(current)
    };
    Rig { transport, coordinator: Coordinator::new(service, options), staged, _root: root }
}

/// The box as an update `version` staged and ready.
fn staged_state(version: &str) -> UpdateState {
    let manifest = manifest::parse_plain(manifest_text("0.0.1", true, None, PACKAGE).as_bytes()).unwrap();
    let manifest = manifest::Manifest { version: version.into(), ..manifest };
    UpdateState {
        phase: Phase::Ready,
        availability: Some(Availability { current: "0.4.34".into(), manifest }),
        staged_directory: Some(PathBuf::from("/tmp/updates/staged/MightyCode.app")),
        ..UpdateState::default()
    }
}

fn downloads(rig: &Rig) -> usize {
    rig.transport.asked().iter().filter(|url| url.as_str() == PACKAGE_URL).count()
}

fn version_shown(rig: &Rig) -> Option<String> {
    rig.coordinator.state().availability.map(|a| a.manifest.version)
}

/// AppUpdateAutoStepsTests.swift:13 "theSameVersionStagedStaysReadyAndIsNotDownloadedAgain".
#[tokio::test]
async fn the_same_version_staged_stays_ready_and_is_not_downloaded_again() {
    for automatic in [true, false] {
        let rig = rig("0.4.34", &manifest_text("0.4.35", true, None, PACKAGE), automatic);
        rig.coordinator.show_for_smoke(staged_state("0.4.35"));
        rig.coordinator.check(None).await;
        assert_eq!(rig.coordinator.state().phase, Phase::Ready);
        assert_eq!(downloads(&rig), 0);
    }
}

/// AppUpdateAutoStepsTests.swift:18 "aNewerVersionThanTheStagedOneIsDownloadedWhenAutomatic".
#[tokio::test]
async fn a_newer_version_than_the_staged_one_is_downloaded_when_automatic() {
    let rig = rig("0.4.34", &manifest_text("0.4.36", true, None, PACKAGE), true);
    rig.coordinator.show_for_smoke(staged_state("0.4.35"));
    rig.coordinator.check(None).await;
    assert_eq!((rig.coordinator.state().phase, version_shown(&rig).as_deref()), (Phase::Ready, Some("0.4.36")));
    assert_eq!(downloads(&rig), 1);
    let package = rig.staged.lock().unwrap()[0].clone();
    assert!(package.ends_with(Path::new("updates").join("0.4.36").join(MAC_PACKAGE_NAME)), "{package:?}");

    let fresh = rig_with_nothing_staged("0.4.36", true);
    fresh.coordinator.check(None).await;
    assert_eq!((fresh.coordinator.state().phase, downloads(&fresh)), (Phase::Ready, 1));
}

fn rig_with_nothing_staged(latest: &str, automatic: bool) -> Rig {
    rig("0.4.34", &manifest_text(latest, true, None, PACKAGE), automatic)
}

/// AppUpdateAutoStepsTests.swift:23 "turnedOffANewerVersionIsOnlyOffered".
#[tokio::test]
async fn turned_off_a_newer_version_is_only_offered() {
    let rig = rig_with_nothing_staged("0.4.36", false);
    rig.coordinator.check(None).await;
    assert_eq!((rig.coordinator.state().phase, downloads(&rig)), (Phase::Available, 0));
}

/// AppUpdateAutoStepsTests.swift:27 "nothingNewerOrNoMacPackageIsUpToDate".
#[tokio::test]
async fn nothing_newer_or_no_mac_package_is_up_to_date() {
    let current = rig_with_nothing_staged("0.4.34", true);
    current.coordinator.check(None).await;
    assert_eq!(current.coordinator.state().phase, Phase::UpToDate);
    // Windows' packages only: nothing for this Mac, and no failure either (Windows would say its architecture has no package).
    let windows_only = r#"{"version":"0.4.36","windows":{"x64":{"url":"https://updates.example/w.zip","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":5}}}"#;
    let rig = rig("0.4.34", windows_only, true);
    rig.coordinator.check(None).await;
    assert_eq!((rig.coordinator.state().phase, rig.coordinator.state().error_message), (Phase::UpToDate, None));
    assert_eq!(downloads(&rig), 0);
}

/// AppUpdateAutoStepsTests.swift:32 "aVersionThisMacCannotRunIsBlocked".
#[tokio::test]
async fn a_version_this_mac_cannot_run_is_blocked() {
    let rig = rig("0.4.34", &manifest_text("0.4.36", true, Some("99.0"), PACKAGE), true);
    rig.coordinator.check(None).await;
    let state = rig.coordinator.state();
    assert_eq!(state.phase, Phase::Failed);
    assert_eq!(state.error_message.as_deref(), Some("Version 0.4.36 needs macOS 99.0 or later. This Mac has 15.6.0."));
    assert_eq!((state.last_error, downloads(&rig)), (None, 0));
}

/// AppUpdateAutoStepsTests.swift:39 "aStagedUpdateStaysReadyWhenTheNewerVersionCannotRunOnThisMac".
#[tokio::test]
async fn a_staged_update_stays_ready_when_the_newer_version_cannot_run_on_this_mac() {
    for automatic in [true, false] {
        let rig = rig("0.4.34", &manifest_text("0.4.36", true, Some("99.0"), PACKAGE), automatic);
        rig.coordinator.show_for_smoke(staged_state("0.4.35"));
        rig.coordinator.check(None).await;
        assert_eq!((rig.coordinator.state().phase, version_shown(&rig).as_deref()), (Phase::Ready, Some("0.4.35")));
        assert_eq!(downloads(&rig), 0);
    }
}

/// AppUpdateAutoStepsTests.swift:56 "quittingInstallsOnlyAReadyUpdateWithTheSwitchOnAndNoPowerOff".
#[test]
fn quitting_installs_only_a_ready_update_with_the_switch_on_and_no_power_off() {
    assert!(auto::installs_on_quit(true, true, true, false));
    assert!(!auto::installs_on_quit(false, true, true, false));
    assert!(!auto::installs_on_quit(true, false, true, false));
    assert!(!auto::installs_on_quit(true, true, false, false));
    assert!(!auto::installs_on_quit(true, true, true, true));
}

/// AppUpdateAutoStepsTests.swift:66 "atLaunchStagedCopiesOfTheRunningOrAnOlderVersionAreDiscarded".
#[test]
fn at_launch_staged_copies_of_the_running_or_an_older_version_are_discarded() {
    let names: Vec<String> = ["0.4.35", "0.4.34", "0.4.36", "unknown", "0.4.35-beta.1"].map(String::from).to_vec();
    assert_eq!(housekeeping::stale_staged_folders(&names, "0.4.35"), ["0.4.35", "0.4.34", "0.4.35-beta.1"]);
    assert!(housekeeping::stale_staged_folders(&[], "0.4.35").is_empty());
    let root = Temp::new();
    for name in &names {
        std::fs::create_dir_all(root.path().join(name).join(STAGED_FOLDER)).unwrap();
    }
    assert_eq!(housekeeping::discard_stale_updates(root.path(), "0.4.35"), 3);
    let mut left: Vec<String> =
        std::fs::read_dir(root.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["0.4.36", "unknown"]);
    assert_eq!(housekeeping::discard_stale_updates(&root.path().join("missing"), "0.4.35"), 0);
}

/// AppUpdateAutoStepsTests.swift:72 "versionsThatDoNotParseNeverKeepAStagedUpdate".
#[tokio::test]
async fn versions_that_do_not_parse_never_keep_a_staged_update() {
    let rig = rig_with_nothing_staged("0.4.36", true);
    rig.coordinator.show_for_smoke(staged_state("not-a-version"));
    rig.coordinator.check(None).await;
    assert_eq!((rig.coordinator.state().phase, version_shown(&rig).as_deref()), (Phase::Ready, Some("0.4.36")));
    assert_eq!(downloads(&rig), 1);
}

/// AppUpdateTests.swift:66 "availabilityHonoursTheMinimumSystemVersionAndRedirectsStayOnHTTPS".
#[tokio::test]
async fn availability_honours_the_minimum_system_version_and_redirects_stay_on_https() {
    let blocked = rig("0.1.0", &manifest_text("0.9.0", true, Some("99.0"), PACKAGE), false);
    blocked.coordinator.check(None).await;
    assert!(blocked.coordinator.state().error_message.is_some_and(|reason| reason.contains("99.0")));
    let fine = rig("0.1.0", &manifest_text("0.9.0", true, Some("14.0"), PACKAGE), false);
    fine.coordinator.check(None).await;
    assert_eq!(fine.coordinator.state().phase, Phase::Available);
    let same = rig("0.9.0", &manifest_text("0.9.0", true, Some("99.0"), PACKAGE), false);
    same.coordinator.check(None).await;
    assert_eq!(same.coordinator.state().phase, Phase::UpToDate, "not newer, so not blocked either");

    let root = Temp::new();
    let transport = Arc::new(FakeUpdateTransport::default());
    let service = Service::new(transport.clone(), root.path(), Some(public_key())).with_profile(&PlatformProfile::macos());
    transport.route(MANIFEST_URL, FakeRoute::redirect(302, "http://updates.example/plain.json"));
    let refused = service.check(Some(MANIFEST_URL), "0.1.0").await.unwrap_err();
    assert_eq!(refused, ServiceError::Refused(UpdateError::key("appUpdate.error.redirectNotHTTPS")));
    transport.route(MANIFEST_URL, FakeRoute::redirect(302, "https://cdn.updates.example/next.json"));
    transport.route("https://cdn.updates.example/next.json", FakeRoute::ok(envelope(&manifest_text("0.9.0", true, None, PACKAGE))));
    assert_eq!(service.check(Some(MANIFEST_URL), "0.1.0").await.unwrap().manifest.version, "0.9.0");
}

/// The manifest's Mac fields as Swift's `parsePlain` reads them; Windows' result unchanged.
#[test]
fn the_manifest_reads_the_mac_package_and_the_lowest_macos_as_swift_does() {
    let sha = "a".repeat(64);
    let asset = |url: &str| format!(r#"{{"url":"{url}","sha256":"{sha}","size":10}}"#);
    let read = |text: &str| manifest::parse_plain(text.as_bytes()).unwrap();
    let plain = read(&format!(r#"{{"version":"1.0","macos":{},"minimumSystemVersion":"14.0"}}"#, asset("https://u.example/m.zip")));
    let mac = plain.macos.unwrap();
    assert_eq!((mac.url.as_str(), mac.size, plain.minimum_system_version.as_deref()), ("https://u.example/m.zip", 10, Some("14.0")));
    assert!(read(&format!(r#"{{"version":"1.0","mac":{}}}"#, asset("https://u.example/a.zip"))).macos.is_some());
    assert!(read(&format!(r#"{{"version":"1.0","darwin":{}}}"#, asset("https://u.example/a.zip"))).macos.is_some());
    assert!(read(&format!(r#"{{"version":"1.0","platforms":{{"macos":{}}}}}"#, asset("https://u.example/a.zip"))).macos.is_some());
    // The first name present wins even when it holds nothing usable, as Swift's `??` does.
    assert!(
        read(&format!(r#"{{"version":"1.0","macos":"https://u.example/a.zip","mac":{}}}"#, asset("https://u.example/a.zip")))
            .macos
            .is_none()
    );
    assert_eq!(read(r#"{"version":"1.0","minOS":"v14.2"}"#).minimum_system_version.as_deref(), Some("14.2"));
    assert_eq!(read(r#"{"version":"1.0","minimumSystemVersion":14,"minOS":"14.2"}"#).minimum_system_version, None);
    for bad in [
        format!(r#"{{"url":"http://u.example/a.zip","sha256":"{sha}","size":10}}"#),
        format!(r#"{{"url":"https://u.example/a.zip","sha256":"{sha}","size":"10"}}"#),
        format!(r#"{{"url":"https://u.example/a.zip","sha256":"{sha}","size":0}}"#),
        format!(r#"{{"url":"https://u.example/a.zip","sha256":"{sha}"}}"#),
        r#"{"url":"https://u.example/a.zip","sha256":"xyz","size":10}"#.to_string(),
    ] {
        assert!(read(&format!(r#"{{"version":"1.0","macos":{bad}}}"#)).macos.is_none(), "{bad}");
    }
    // A Mac asset Swift cannot read is no error (C# never reads one), and Windows' assets read as before.
    let both = read(&format!(r#"{{"version":"1.0","macos":{{"size":"x"}},"windows":{{"x64":{}}}}}"#, asset("https://u.example/w.zip")));
    assert_eq!((both.macos, both.windows.len()), (None, 1));
}

/// The profile's `build_update_url_wins` at its site, both values; `update_needs_public_key`'s words, both values.
#[tokio::test]
async fn the_stamped_address_wins_unless_the_profile_says_otherwise_and_a_keyless_build_says_its_oses_words() {
    let root = Temp::new();
    let transport = Arc::new(FakeUpdateTransport::default());
    let typed = "https://mine.example/latest.json";
    for (wins, expected) in [(true, MANIFEST_URL), (false, typed)] {
        let profile = PlatformProfile { build_update_url_wins: wins, ..PlatformProfile::macos() };
        let service = Arc::new(Service::new(transport.clone(), root.path(), Some(public_key())));
        let options = Options { built_in_manifest_url: Some(MANIFEST_URL.into()), profile, ..Options::new("0.1.0") };
        let coordinator = Coordinator::new(service, options);
        assert_eq!(coordinator.effective_manifest_url(Some(typed)).as_deref(), Some(expected));
        assert_eq!(coordinator.effective_manifest_url(None).as_deref(), Some(MANIFEST_URL));
        assert_eq!(coordinator.effective_manifest_url(Some("http://plain.example/x")).as_deref(), Some(MANIFEST_URL));
    }
    for (needs, words) in [
        (true, "This build has no update public key and does not support update checks."),
        (false, "This build does not support update checks."),
    ] {
        let profile = PlatformProfile { update_needs_public_key: needs, ..PlatformProfile::macos() };
        let service = Arc::new(Service::new(transport.clone(), root.path(), None).with_profile(&profile));
        let options = Options { built_in_manifest_url: Some(MANIFEST_URL.into()), profile, locale: en(), ..Options::new("0.1.0") };
        let coordinator = Coordinator::new(service.clone(), options);
        coordinator.check(None).await;
        assert_eq!(coordinator.state().error_message.as_deref(), Some(words));
        assert!(service.check(Some(MANIFEST_URL), "0.1.0").await.is_err());
    }
    assert!(transport.asked().is_empty(), "no check went out without a key");
}

/// The Mac's package name and kept update: `MightyCode-macos.zip` beside a staged folder holding exactly one `.app`.
#[test]
fn the_mac_package_is_kept_beside_one_staged_app() {
    let root = Temp::new();
    let transport = Arc::new(FakeUpdateTransport::default());
    let mac = Service::new(transport.clone(), root.path(), Some(public_key())).with_profile(&PlatformProfile::macos());
    let windows = Service::new(transport, root.path(), Some(public_key()));
    assert_eq!((mac.package_name(), windows.package_name()), (MAC_PACKAGE_NAME, PACKAGE_NAME));
    let folder = root.path().join("updates").join("0.5.1");
    std::fs::create_dir_all(folder.join(STAGED_FOLDER).join("MightyCode.app")).unwrap();
    std::fs::write(folder.join(MAC_PACKAGE_NAME), "zip").unwrap();
    assert_eq!(mac.kept_version("0.5.0").as_deref(), Some("0.5.1"));
    assert_eq!(windows.kept_version("0.5.0"), None, "Windows wants its own package and exe");
    assert_eq!(mac.kept_version("0.5.1"), None);
    std::fs::create_dir_all(folder.join(STAGED_FOLDER).join("Other.app")).unwrap();
    assert_eq!(mac.kept_version("0.5.0"), None, "two bundles are no whole update");
}

/// AppUpdateTests.swift:254 "installOnQuitNeitherRelaunchesNorSwapsUnderARunningApp".
#[test]
fn install_on_quit_neither_relaunches_nor_swaps_under_a_running_app() {
    let staged = Path::new("/tmp/updates/0.2.0/staged/MightyCode.app");
    let destination = Path::new("/Applications/Mighty (it's) [1].app");
    let quiet = install_script(staged, destination, None, "MightyCode", 42, false);
    assert!(!quiet.contains("open -n"));
    assert!(quiet.contains("pgrep -lf '^/Applications/Mighty \\(it'\\''s\\) \\[1\\]\\.app/Contents/MacOS/MightyCode( |$)'"), "{quiet}");
    assert!(quiet.contains("grep -Ev -- ' (--agent-io-mcp|--agent-delegation-mcp|--verify-resources)( |$)'"));
    for part in ["OLD=\"$DESTINATION.update-old\"", "mv \"$DESTINATION\" \"$OLD\"", "mv \"$NEW\" \"$DESTINATION\"", "rm -rf \"$OLD\""] {
        assert!(quiet.contains(part), "{part}");
    }
    let at = |text: &str| quiet.find(text).unwrap_or_else(|| panic!("{text}"));
    let check = at("pgrep -lf");
    assert!(at("ditto \"$SOURCE\" \"$NEW\"") < check && check < at("mv \"$DESTINATION\" \"$OLD\""));
    assert!(quiet.contains("mv \"$OLD\" \"$DESTINATION\" || echo \"Could not restore the previous version"));
    assert!(
        at("trap '' HUP INT TERM") < at("mv \"$DESTINATION\" \"$OLD\"") && at("mv \"$NEW\" \"$DESTINATION\"") < at("trap - HUP INT TERM")
    );
    assert!(quiet.contains(
        "if [ -e \"$OLD\" ]; then echo \"Could not remove the leftover $OLD; skipping the update.\" >&2; rm -rf \"$NEW\"; exit 1; fi"
    ));
    assert!(quiet.contains("\"$LSREGISTER\" -u \"$OLD\""));
    assert!(at("if [ ! -e \"$DESTINATION\" ] && [ -d \"$OLD\" ]; then") < at("kill -0 \"$PID\""));
    assert!(!quiet.contains("BACKUP") && !quiet.contains("MightyCode.app.bak"));

    let relaunching = install_script(staged, destination, None, "MightyCode", 42, true);
    assert!(
        relaunching.contains("open -n \"$DESTINATION\"") && !relaunching.contains("pgrep") && relaunching.contains("trap '' HUP INT TERM")
    );
    assert!(relaunching.contains("OLD=\"$DESTINATION.update-old\"") && relaunching.contains("rm -rf \"$OLD\""));
    // AppUpdateTests.swift:169's text checks.
    assert!(
        quiet.starts_with("#!/bin/sh")
            && quiet.contains("PATH=/usr/bin:/bin")
            && quiet.contains("PID=42\n")
            && quiet.contains("\nsleep 2\n")
    );
    assert!(
        install_script(staged, Path::new("/Users/x/Apps (it's here)/MightyCode.app"), None, "MightyCode", 1, true)
            .contains("DESTINATION='/Users/x/Apps (it'\\''s here)/MightyCode.app'")
    );
    // AppUpdateTests.swift:218's: the renamed app replaces the old one; the same name as before writes the script it always did.
    let renamed =
        install_script(staged, Path::new("/Apps/MightyCode.app"), Some(Path::new("/Apps/MightyClaude.app")), "MightyClaude", 1, false);
    assert!(renamed.contains("REPLACED='/Apps/MightyClaude.app'\n") && renamed.contains("OLD=\"$REPLACED.update-old\""));
    assert!(renamed.contains("pgrep -lf '^/Apps/MightyClaude\\.app/Contents/MacOS/MightyClaude( |$)'"));
    let same = Path::new("/Apps/MightyCode.app");
    assert_eq!(
        install_script(staged, same, Some(Path::new("/Apps/./MightyCode.app/")), "MightyCode", 1, false),
        install_script(staged, same, None, "MightyCode", 1, false)
    );
}

/// A fake app (AppUpdateTests.swift `makeFakeApp`): Info.plist (XML) and an executable script.
fn fake_app(root: &Path, name: &str, executable: &str, bundle_id: &str, version: &str) -> PathBuf {
    let app = root.join(name);
    std::fs::create_dir_all(app.join("Contents").join("MacOS")).unwrap();
    let mut info = plist::Dictionary::new();
    info.insert("CFBundleIdentifier".into(), bundle_id.into());
    info.insert("CFBundleExecutable".into(), executable.into());
    info.insert("CFBundleShortVersionString".into(), version.into());
    plist::Value::Dictionary(info).to_file_xml(app.join("Contents").join("Info.plist")).unwrap();
    let binary = app.join("Contents").join("MacOS").join(executable);
    std::fs::write(&binary, "#!/bin/sh\necho hi\n").unwrap();
    runnable(&binary);
    app
}

#[cfg(unix)]
fn runnable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn runnable(_path: &Path) {}

/// Whether a file is "runnable" for the checks that run everywhere: it is there (the Mac's `access(X_OK)` is the platform's).
fn present(path: &Path) -> bool {
    path.is_file()
}

/// The plan the coordinator makes for a staged `app` and its package.
fn plan_for(app: &Path, installed: &Path, package: &Path, relaunch: bool) -> InstallPlan {
    let bytes = std::fs::read(package).unwrap();
    InstallPlan {
        relaunch,
        ..InstallPlan::create(app, installed, package, &sha256_of(&bytes), bytes.len() as i64, 4242, None, chrono::Utc::now())
    }
}

/// AppStore+AppUpdate.swift:408-447 `prepareAppUpdateInstall` and `verifyAppUpdatePackage`: each refusal in Swift's order and words;
/// the profile's `update_rechecks_sha256` at its site, both values.
#[test]
fn a_mac_install_is_refused_where_the_swift_app_refuses_it() {
    let root = Temp::new();
    let updates = root.path().join("updates").join("0.5.1");
    let staged = fake_app(&updates.join(STAGED_FOLDER), "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.5.1");
    let package = updates.join(MAC_PACKAGE_NAME);
    std::fs::write(&package, b"the package").unwrap();
    let installed = fake_app(&root.path().join("Applications"), "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.5.0");
    let locale = en();
    let yes = |_: &Path| true;
    let signed = |_: &Path| Ok(());
    let tree = stage_mac::tree_digest(&staged, &present).unwrap();
    let prepare = |plan: &InstallPlan, recheck: bool, writable: &dyn Fn(&Path) -> bool| {
        let seams = install_script::MacSeams { may_write: writable, may_execute: &present, signature: &signed, staged_tree: Some(&tree) };
        install_script::prepare(plan, "MightyCode", plan.app_process_id, recheck, &seams, &locale)
    };

    let plan = plan_for(&staged, &installed, &package, true);
    let ready = prepare(&plan, true, &yes).unwrap();
    assert_eq!(ready.folder, updates);
    assert_eq!(ready.script, install_script(&staged, &installed, Some(&installed), "MightyCode", 4242, true));

    let not_an_app = plan_for(&staged, &root.path().join("Applications"), &package, true);
    assert!(prepare(&not_an_app, true, &yes).unwrap_err().starts_with("The running app's location ("));
    let no = |_: &Path| false;
    assert!(prepare(&plan, true, &no).unwrap_err().contains("is not writable"));

    let other = fake_app(&root.path().join("other"), "MightyCode.app", "MightyCode", "com.example.other", "0.5.1");
    let wrong = prepare(&plan_for(&other, &installed, &package, true), true, &yes).unwrap_err();
    assert!(wrong.starts_with("The check right before installing failed: "), "{wrong}");

    let mut changed = plan.clone();
    changed.size += 1;
    assert_eq!(
        prepare(&changed, true, &yes).unwrap_err(),
        "Could not install the update: The package size does not match the update information — download it again."
    );
    let mut digest = plan.clone();
    digest.sha256 = "0".repeat(64);
    assert!(prepare(&digest, true, &yes).unwrap_err().contains("SHA-256 does not match"));
    // Without the profile's re-check the package on disk is not read again.
    assert!(prepare(&digest, false, &yes).is_ok() && prepare(&changed, false, &yes).is_ok());

    // Review M-H 2: the staged tree must be the one staging unpacked (a file changed, or no digest recorded, refuses).
    std::fs::write(staged.join("Contents").join("MacOS").join("MightyCode"), "#!/bin/sh\necho changed\n").unwrap();
    assert!(prepare(&plan, true, &yes).unwrap_err().contains("SHA-256 does not match"));
    assert!(prepare(&plan, false, &yes).is_ok());
    let unrecorded = install_script::MacSeams { may_write: &yes, may_execute: &present, signature: &signed, staged_tree: None };
    assert!(install_script::prepare(&plan, "MightyCode", 1, true, &unrecorded, &locale).is_err());

    // Review M-H 1: a signature the platform refuses stops the install before any script.
    let unsigned = |_: &Path| Err(UpdateError::key("appUpdate.error.bundleIdentity"));
    let refusing = install_script::MacSeams { may_write: &yes, may_execute: &present, signature: &unsigned, staged_tree: None };
    let refused = install_script::prepare(&plan, "MightyCode", 1, false, &refusing, &locale).unwrap_err();
    assert!(refused.starts_with("The check right before installing failed: The package's app bundle isn't Mighty Code"), "{refused}");

    // Review M-H 4: only a bundle named as the app is staged; a renamed install is replaced in place, never beside it.
    let odd = fake_app(&updates.join(STAGED_FOLDER), "Other.app", "MightyCode", "dev.mightycode.native", "0.5.1");
    assert!(prepare(&plan_for(&odd, &installed, &package, false), false, &yes).unwrap_err().contains("isn't Mighty Code"));
    let renamed = fake_app(&root.path().join("Applications"), "Mighty Code Old.app", "MightyCode", "dev.mightycode.native", "0.5.0");
    let in_place = prepare(&plan_for(&staged, &renamed, &package, false), false, &yes).unwrap();
    assert_eq!(in_place.script, install_script(&staged, &renamed, Some(&renamed), "MightyCode", 4242, false));
    let old_name = fake_app(&root.path().join("Applications"), "MightyClaude.app", "MightyClaude", "dev.mightyclaude.native", "0.4.0");
    let beside = prepare(&plan_for(&staged, &old_name, &package, false), false, &yes).unwrap();
    let landing = root.path().join("Applications").join("MightyCode.app");
    assert_eq!(beside.script, install_script(&staged, &landing, Some(&old_name), "MightyCode", 4242, false));
    assert!(PlatformProfile::macos().update_rechecks_sha256 && PlatformProfile::macos().update_package == UpdatePackage::MacBundle);
}

/// Review M-H 1 (stricter than Swift, Sparkle's rule): a team-signed app asks the same team of the staged bundle; an ad hoc or unsigned
/// one (development, CI) asks for the strict validity alone.
#[test]
fn a_staged_bundle_must_be_signed_by_the_running_apps_team() {
    let bundle = Path::new("/tmp/updates/0.5.1/staged/MightyCode.app");
    let asked = Mutex::new(Vec::new());
    let team_signed = |signed_by: Option<&'static str>| {
        move |_: &Path, requirement: Option<&str>| {
            // The platform's answer for a bundle signed by `signed_by`: valid, and meeting a team requirement only for that team.
            requirement.is_none_or(|requirement| signed_by.is_some_and(|team| requirement == stage_mac::team_requirement(team)))
        }
    };
    let record = |requirement: Option<&str>| asked.lock().unwrap().push(requirement.map(String::from));
    // The same team.
    let same = team_signed(Some("ABCDE12345"));
    assert_eq!(
        stage_mac::check_code_signature(bundle, Some("ABCDE12345"), &|b, r| {
            record(r);
            same(b, r)
        }),
        Ok(())
    );
    assert_eq!(
        asked.lock().unwrap().last().cloned().flatten().as_deref(),
        Some("anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\"")
    );
    // Another team, or ad hoc, under a team-signed app.
    for signed_by in [Some("ZZZZZ99999"), None] {
        let other = team_signed(signed_by);
        assert_eq!(
            stage_mac::check_code_signature(bundle, Some("ABCDE12345"), &other),
            Err(UpdateError::key("appUpdate.error.bundleIdentity"))
        );
    }
    // An ad hoc running app: strict validity only, no team asked.
    let adhoc = team_signed(None);
    assert_eq!(
        stage_mac::check_code_signature(bundle, None, &|b, r| {
            record(r);
            adhoc(b, r)
        }),
        Ok(())
    );
    assert_eq!(asked.lock().unwrap().last().cloned(), Some(None));
    assert_eq!(stage_mac::check_code_signature(bundle, None, &|_, _| false), Err(UpdateError::key("appUpdate.error.bundleIdentity")));
    assert!(!stage_mac::team_requirement("A\"B").contains("A\"B"), "a team id never closes the requirement's quotes");
}

/// Review M-H 2: the tree digest changes with a file's contents, its executable bit, a new entry or a link's target.
#[test]
fn the_staged_trees_digest_follows_its_contents() {
    let root = Temp::new();
    let app = fake_app(root.path(), "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.5.1");
    let first = stage_mac::tree_digest(&app, &present).unwrap();
    assert_eq!(stage_mac::tree_digest(&app, &present).unwrap(), first);
    assert_ne!(stage_mac::tree_digest(&app, &|_| false).unwrap(), first, "the executable bit counts");
    std::fs::write(app.join("Contents").join("extra"), "x").unwrap();
    let second = stage_mac::tree_digest(&app, &present).unwrap();
    assert_ne!(second, first);
    std::fs::write(app.join("Contents").join("extra"), "y").unwrap();
    assert_ne!(stage_mac::tree_digest(&app, &present).unwrap(), second);
}

/// Swift `validate` on everything but the executable bit (the platform's): the id or its successor, no `/` in the executable's name, the
/// bundle inside its folder.
#[test]
fn staging_validates_the_bundles_identity_and_place() {
    let root = Temp::new();
    let folder = root.path().join(STAGED_FOLDER);
    let good = fake_app(&folder, "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.5.1");
    assert_eq!(stage_mac::validate(&good, Some(&folder), stage_mac::BUNDLE_IDENTIFIER, &present), Ok(()));
    assert_eq!(stage_mac::validate(&good, Some(&folder), "dev.mightyclaude.native", &present), Ok(()), "the old id takes the new app");
    let key = |error: Result<(), UpdateError>| match error {
        Err(UpdateError::Key { key, .. }) => key,
        other => panic!("{other:?}"),
    };
    assert_eq!(key(stage_mac::validate(&good, Some(&folder), "com.example.other", &present)), "appUpdate.error.bundleIdentity");
    assert_eq!(
        key(stage_mac::validate(&good, Some(&root.path().join("elsewhere")), stage_mac::BUNDLE_IDENTIFIER, &present)),
        "appUpdate.error.bundleLocation"
    );
    assert_eq!(
        key(stage_mac::validate(&good, Some(&folder), stage_mac::BUNDLE_IDENTIFIER, &|_| false)),
        "appUpdate.error.bundleExecutable"
    );
    let slashed = fake_app(&folder, "Slash.app", "MightyCode", "dev.mightycode.native", "0.5.1");
    let mut info = plist::Dictionary::new();
    info.insert("CFBundleIdentifier".into(), "dev.mightycode.native".into());
    info.insert("CFBundleExecutable".into(), "../MightyCode".into());
    plist::Value::Dictionary(info).to_file_binary(slashed.join("Contents").join("Info.plist")).unwrap();
    assert_eq!(key(stage_mac::validate(&slashed, Some(&folder), stage_mac::BUNDLE_IDENTIFIER, &present)), "appUpdate.error.bundleIdentity");
    std::fs::write(slashed.join("Contents").join("Info.plist"), "not a plist").unwrap();
    assert_eq!(key(stage_mac::validate(&slashed, Some(&folder), stage_mac::BUNDLE_IDENTIFIER, &present)), "appUpdate.error.bundleIdentity");
}

/// The staging's unzip step and bundle count, over a stand-in unzip.
#[tokio::test]
async fn staging_wants_exactly_one_app_from_the_unzip() {
    let root = Temp::new();
    let package = root.path().join(MAC_PACKAGE_NAME);
    std::fs::write(&package, "zip").unwrap();
    let destination = root.path().join(STAGED_FOLDER);
    std::fs::create_dir_all(destination.join("leftover")).unwrap();
    let failed = stage_mac::stage(&package, &destination, stage_mac::BUNDLE_IDENTIFIER, present, |_, _| async {
        Err("ditto: bad zip\n".to_string())
    })
    .await;
    assert_eq!(failed, Err(UpdateError::Key { key: "appUpdate.error.unzip", args: vec![("error", "ditto: bad zip".into())] }));
    assert!(!destination.join("leftover").exists(), "the folder is made afresh");
    let none = stage_mac::stage(&package, &destination, stage_mac::BUNDLE_IDENTIFIER, present, |_, _| async { Ok(()) }).await;
    assert_eq!(none, Err(UpdateError::Key { key: "appUpdate.error.bundleCount", args: vec![("count", "0".into())] }));
    let odd = stage_mac::stage(&package, &destination, stage_mac::BUNDLE_IDENTIFIER, present, |_, folder: PathBuf| async move {
        fake_app(&folder, "Other.app", "MightyCode", "dev.mightycode.native", "0.5.1");
        Ok(())
    })
    .await;
    assert_eq!(odd, Err(UpdateError::key("appUpdate.error.bundleIdentity")), "review M-H 4: only the app's own names");
    let one = stage_mac::stage(&package, &destination, stage_mac::BUNDLE_IDENTIFIER, present, |_, folder: PathBuf| async move {
        fake_app(&folder, "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.5.1");
        Ok(())
    })
    .await;
    assert_eq!(one, Ok(destination.join("MightyCode.app")));
}

/// What needs the Mac itself: `ditto`, `/bin/sh` running the real script, BSD `pgrep`, `codesign`.
#[cfg(target_os = "macos")]
mod mac {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::*;

    /// The checks start their own children (the code under test hands its starts to the platform's process host, M-A's).
    #[allow(clippy::disallowed_methods, reason = "the checks' own children: ditto, sh, codesign, /usr/bin/true and the fake apps")]
    fn command(program: &str) -> Command {
        Command::new(program)
    }

    fn run(program: &str, args: &[&str]) -> (i32, String) {
        let out = command(program).args(args).stdin(Stdio::null()).output().unwrap();
        (out.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    fn path(p: &Path) -> &str {
        p.to_str().unwrap()
    }

    fn zip(item: &Path, zip: &Path) {
        assert_eq!(run("/usr/bin/ditto", &["-c", "-k", "--keepParent", path(item), path(zip)]).0, 0);
    }

    async fn ditto(package: PathBuf, folder: PathBuf) -> Result<(), String> {
        let (code, text) = run("/usr/bin/ditto", &["-x", "-k", path(&package), path(&folder)]);
        if code == 0 { Ok(()) } else { Err(text) }
    }

    fn executable(file: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(file).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }

    async fn stage(package: &Path, expected: &str) -> Result<PathBuf, UpdateError> {
        let destination = package.parent().unwrap().join(STAGED_FOLDER);
        stage_mac::stage(package, &destination, expected, executable, ditto).await
    }

    /// A pid that is certainly gone: a child that ran and was reaped (macOS hands pids out in rising order).
    fn gone_pid() -> i32 {
        let mut gone = command("/usr/bin/true").spawn().unwrap();
        let pid = gone.id() as i32;
        gone.wait().unwrap();
        pid
    }

    /// The script as the checks run it: LaunchServices left alone.
    fn without_lsregister(script: &str) -> String {
        let line = format!("LSREGISTER={}\n", install_script::LSREGISTER);
        assert!(script.contains(&line));
        script.replace(&line, "LSREGISTER=/usr/bin/true\n")
    }

    fn short_version(app: &Path) -> Option<String> {
        let info = plist::Value::from_file(app.join("Contents").join("Info.plist")).ok()?;
        info.as_dictionary()?.get("CFBundleShortVersionString")?.as_string().map(String::from)
    }

    /// AppUpdateTests.swift:98 "downloadVerifiesDigestAndSizeThenStagingValidatesTheBundle".
    #[tokio::test]
    async fn download_verifies_digest_and_size_then_staging_validates_the_bundle() {
        let root = Temp::new();
        let app = fake_app(&root.path().join("source"), "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.2.0");
        let package = root.path().join("MightyCode-macos.zip");
        zip(&app, &package);
        let data = std::fs::read(&package).unwrap();
        let transport = Arc::new(FakeUpdateTransport::default());
        transport.route(PACKAGE_URL, FakeRoute::ok(data.clone()));
        let service = Service::new(transport.clone(), root.path(), Some(public_key())).with_profile(&PlatformProfile::macos());
        std::fs::create_dir_all(root.path().join("updates").join("0.0.9")).unwrap();
        let asset = manifest::Asset { url: PACKAGE_URL.into(), sha256: sha256_of(&data), size: data.len() as u64 };
        let cancel = tokio_util::sync::CancellationToken::new();
        let downloaded = service.download(&asset, "0.2.0", Arc::new(|_| {}), &cancel).await.unwrap();
        assert_eq!(std::fs::read(&downloaded).unwrap(), data);
        assert_eq!(downloaded, root.path().join("updates").join("0.2.0").join(MAC_PACKAGE_NAME));
        assert!(!root.path().join("updates").join("0.0.9").exists(), "older folders pruned");
        let wrong = manifest::Asset { sha256: "0".repeat(64), ..asset.clone() };
        assert!(service.download(&wrong, "0.2.1", Arc::new(|_| {}), &cancel).await.is_err());
        let longer = manifest::Asset { size: asset.size + 1, ..asset.clone() };
        assert!(service.download(&longer, "0.2.2", Arc::new(|_| {}), &cancel).await.is_err());
        assert!(!root.path().join("updates").join("0.2.1").exists());

        let good = service.download(&asset, "0.2.0", Arc::new(|_| {}), &cancel).await.unwrap();
        let staged = stage(&good, stage_mac::BUNDLE_IDENTIFIER).await.unwrap();
        assert_eq!(staged.file_name().unwrap(), "MightyCode.app");
        assert!(executable(&staged.join("Contents/MacOS/MightyCode")));
        assert!(stage(&good, "com.example.other").await.is_err());
        let wrong = fake_app(&root.path().join("other"), "MightyCode.app", "MightyCode", "com.example.other", "0.2.0");
        let wrong_zip = root.path().join("wrong").join("wrong.zip");
        std::fs::create_dir_all(wrong_zip.parent().unwrap()).unwrap();
        zip(&wrong, &wrong_zip);
        assert!(stage(&wrong_zip, stage_mac::BUNDLE_IDENTIFIER).await.is_err());
        // A linked bundle is refused, also as an entry of the zip (the folder archived, so ditto keeps the link).
        let links = root.path().join("links");
        std::fs::create_dir_all(&links).unwrap();
        std::os::unix::fs::symlink(&app, links.join("MightyCode.app")).unwrap();
        assert!(stage_mac::validate(&links.join("MightyCode.app"), Some(&links), stage_mac::BUNDLE_IDENTIFIER, &executable).is_err());
        let link_zip = root.path().join("linkzip").join("link.zip");
        std::fs::create_dir_all(link_zip.parent().unwrap()).unwrap();
        assert_eq!(run("/usr/bin/ditto", &["-c", "-k", path(&links), path(&link_zip)]).0, 0);
        assert!(stage(&link_zip, stage_mac::BUNDLE_IDENTIFIER).await.is_err());
        // A link to the plist inside an otherwise real bundle is refused too.
        let plist_linked = fake_app(&root.path().join("plist-link"), "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.2.0");
        std::fs::remove_file(plist_linked.join("Contents/Info.plist")).unwrap();
        std::os::unix::fs::symlink(app.join("Contents/Info.plist"), plist_linked.join("Contents/Info.plist")).unwrap();
        assert_eq!(
            stage_mac::validate(&plist_linked, None, stage_mac::BUNDLE_IDENTIFIER, &executable),
            Err(UpdateError::key("appUpdate.error.bundleSymlink"))
        );
    }

    /// Waits for `log` to say the script is done (its last word), at most 240 s as Swift's check does.
    fn finished(log: &Path) -> (bool, String) {
        let failures =
            ["skipping the update", "No app to install", "Could not copy", "Could not back up", "restoring the previous version"];
        let deadline = Instant::now() + Duration::from_secs(240);
        loop {
            let text = std::fs::read_to_string(log).unwrap_or_default();
            if text.contains("Installed: ") {
                return (true, text);
            }
            if failures.iter().any(|failure| text.contains(failure)) || Instant::now() > deadline {
                return (false, text);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// AppUpdateTests.swift:169 "installHelperSwapsTheBundleKeepsNoBackupOnSuccessAndLogs".
    #[test]
    fn install_helper_swaps_the_bundle_keeps_no_backup_on_success_and_logs() {
        let root = Temp::new();
        let version = root.path().join("updates").join("0.2.0");
        let staged = fake_app(&version.join(STAGED_FOLDER), "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.2.0");
        let installed = fake_app(&root.path().join("Apps (it's here)"), "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.1.0");
        let pid = gone_pid();
        let script = install_script(&staged, &installed, None, "MightyCode", pid, false);
        assert!(script.contains(&format!("PID={pid}\n")));
        // Started as the app starts it (detached, its output in install.log beside the package).
        let helper = version.join("install.sh");
        let log = version.join("install.log");
        std::fs::write(&helper, without_lsregister(&script)).unwrap();
        let output = std::fs::File::create(&log).unwrap();
        let mut helper_process = command("/bin/sh")
            .arg(&helper)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", std::env::var("HOME").unwrap())
            .stdin(Stdio::null())
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .spawn()
            .unwrap();
        let (done, text) = finished(&log);
        assert!(done, "the install helper did not finish; install.log: {text}");
        assert!(helper_process.wait().unwrap().success(), "{text}");
        assert_eq!(short_version(&installed).as_deref(), Some("0.2.0"));
        assert!(!staged.exists() && !version.join(STAGED_FOLDER).exists(), "the staged folder is removed");
        assert!(!Path::new(&format!("{}.update-new", path(&installed))).exists());
        assert!(!Path::new(&format!("{}.update-old", path(&installed))).exists(), "no backup once the swap succeeded");
    }

    /// AppUpdateTests.swift:218 "theOldIdInstallsTheRenamedAppUnderItsNewName".
    #[test]
    fn the_old_id_installs_the_renamed_app_under_its_new_name() {
        let root = Temp::new();
        let staged_folder = root.path().join("updates").join("0.3.0").join(STAGED_FOLDER);
        let staged = fake_app(&staged_folder, "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.3.0");
        stage_mac::validate(&staged, Some(&staged_folder), "dev.mightyclaude.native", &executable).unwrap();
        let apps = root.path().join("Apps");
        let installed = fake_app(&apps, "MightyClaude.app", "MightyClaude", "dev.mightyclaude.native", "0.2.0");
        let destination = apps.join("MightyCode.app");
        let script = install_script(&staged, &destination, Some(&installed), "MightyClaude", gone_pid(), false);
        let helper = root.path().join("install.sh");
        std::fs::write(&helper, without_lsregister(&script)).unwrap();
        let (code, log) = run("/bin/sh", &[path(&helper)]);
        assert!(code == 0 && log.contains("Installed: "), "{log}");
        assert_eq!(short_version(&destination).as_deref(), Some("0.3.0"));
        assert!(!installed.exists());
        assert!(!Path::new(&format!("{}.update-old", path(&installed))).exists());
        assert!(!Path::new(&format!("{}.update-new", path(&destination))).exists());
    }

    /// AppUpdateTests.swift:295 "installOnQuitWaitsForTheAppButNotForItsHeadlessChildren".
    #[test]
    fn install_on_quit_waits_for_the_app_but_not_for_its_headless_children() {
        let root = Temp::new();
        let staged = fake_app(&root.path().join("updates/0.2.0/staged"), "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.2.0");
        let installed = fake_app(&root.path().join("X (1) [a]"), "MightyCode.app", "MightyCode", "dev.mightycode.native", "0.1.0");
        let binary = installed.join("Contents/MacOS/MightyCode");
        std::fs::remove_file(&binary).unwrap();
        std::fs::copy("/bin/bash", &binary).unwrap();
        // A copied system binary is killed at launch unless it carries its own (ad-hoc) signature.
        let (signed, text) = run("/usr/bin/codesign", &["-f", "-s", "-", path(&binary)]);
        assert_eq!(signed, 0, "codesign: {text}");
        let start = |arguments: &[&str]| {
            command(path(&binary)).args(arguments).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
        };
        let helper = root.path().join("install.sh");
        let script = install_script(&staged, &installed, None, "MightyCode", gone_pid(), false);
        std::fs::write(&helper, without_lsregister(&script)).unwrap();

        // The app itself runs again from the destination: nothing is replaced.
        let mut window = start(&[]);
        let (code, log) = run("/bin/sh", &[path(&helper)]);
        assert!(code == 0 && log.contains("running again"), "{log}");
        assert_eq!(short_version(&installed).as_deref(), Some("0.1.0"));
        assert!(staged.exists(), "still staged for the next quit");
        assert!(!Path::new(&format!("{}.update-new", path(&installed))).exists());
        assert!(window.try_wait().unwrap().is_none(), "it was there the whole time");
        window.kill().unwrap();
        window.wait().unwrap();

        // Only a headless child of the app is left: the update goes in. `--` keeps bash from reading the flag as its own.
        let mut child = start(&["-s", "--", "--agent-io-mcp"]);
        let (code, log) = run("/bin/sh", &[path(&helper)]);
        assert!(code == 0 && log.contains("Installed: "), "{log}");
        assert_eq!(short_version(&installed).as_deref(), Some("0.2.0"));
        assert!(!Path::new(&format!("{}.update-old", path(&installed))).exists());
        assert!(child.try_wait().unwrap().is_none(), "it did not hold the update back");
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
