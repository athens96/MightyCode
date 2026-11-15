//! C#'s app update checks (`Tests\Verification.cs` "app update …", `AppUpdateVerification.cs`, and "an app started from a bare
//! environment takes the user's own variables back", `UserEnvironmentVerification.cs`), ported one to one by name. "app update strings
//! match macOS" is mighty-core's (`app_update_view.rs`), beside the words it checks.
//!
//! Nothing here opens a socket: the service is handed `mighty_platform::fake::FakeUpdateTransport` (or a gated wrapper of it) that
//! answers from memory, at `https://updates.example/…` (never the real update address). Nothing here starts a process: the
//! replacement is handed its seams. The key pair is made for these checks only. Every folder is a fresh temporary one; paths are
//! compared as the code under test builds them (full paths are never resolved through links, so `/var` and an 8.3 `%TEMP%` stay as
//! written on both sides).

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, TimeZone, Utc};
use ed25519_dalek::{Signer, SigningKey};
use mighty_core::app_update_view::{describe, text};
use mighty_core::locale::Locale;
use mighty_core::{AppSnapshot, Wire};
use mighty_platform::fake::{FakeRoute, FakeUpdateTransport};
use mighty_platform::{BoxFuture, DownloadRequest, Downloaded, UpdateResponse, UpdateTransport};
use mighty_updates::app_update::auto::{self, IdleBlocker, IdleFacts, RestartAction, RestartDecision, RestartState};
use mighty_updates::app_update::coordinator::{Coordinator, KeptCheck, LaunchHelper, Options, UpdateState};
use mighty_updates::app_update::housekeeping::{self, NEW_COPY_GRACE};
use mighty_updates::app_update::manifest::{self, Asset, ENVELOPE_FORMAT};
use mighty_updates::app_update::plan::{self, HELPER_ENVIRONMENT, HELPER_FLAG, InstallPlan, RELAUNCH_FLAG};
use mighty_updates::app_update::replacement::{self, MOVE_ATTEMPTS, MoveKind, Seams};
use mighty_updates::app_update::service::{MAX_PACKAGE_BYTES, Service, ServiceError};
use mighty_updates::app_update::{EXECUTABLE_NAME, Phase, environment, guard, installer_entry, signature, smoke, stage, version};
use tokio_util::sync::CancellationToken;

// — Shared fixtures —

fn ko() -> Locale {
    Locale::new("ko")
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

fn sha256_of(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh temporary folder, deleted when dropped.
struct Temp(PathBuf);

impl Temp {
    fn new() -> Temp {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path =
            std::env::temp_dir().join(format!("mighty-appupdate-{}-{nanos}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&path).unwrap();
        Temp(path)
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

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// The fixture key pair: made for these checks only.
const FIXTURE_SEED: [u8; 32] = {
    let mut seed = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        seed[i] = (i * 7 + 3) as u8;
        i += 1;
    }
    seed
};

fn public_key(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

fn fixture_key() -> [u8; 32] {
    public_key(&FIXTURE_SEED)
}

fn other_key() -> [u8; 32] {
    public_key(&std::array::from_fn(|i| (i + 1) as u8))
}

fn sign(seed: &[u8; 32], message: &[u8]) -> [u8; 64] {
    SigningKey::from_bytes(seed).sign(message).to_bytes()
}

/// C#'s `PlainManifest` and its defaults.
struct Plain {
    version: &'static str,
    x64_sha: String,
    x64_size: Option<i64>,
    arm64_sha: String,
    arm64_size: Option<i64>,
    scheme: &'static str,
}

impl Default for Plain {
    fn default() -> Plain {
        Plain {
            version: "9.9.9",
            x64_sha: "a".repeat(64),
            x64_size: Some(4096),
            arm64_sha: "b".repeat(64),
            arm64_size: Some(4096),
            scheme: "https",
        }
    }
}

impl Plain {
    fn text(&self) -> String {
        let asset = |architecture: &str, sha: &str, size: Option<i64>| {
            let mut parts =
                vec![format!(r#""url":"{}://updates.example/{}/MightyCode-windows-{architecture}.zip""#, self.scheme, self.version)];
            parts.push(format!(r#""sha256":"{sha}""#));
            if let Some(size) = size {
                parts.push(format!(r#""size":{size}"#));
            }
            format!("{{{}}}", parts.join(","))
        };
        format!(
            r#"{{"version":"{}","notes":"픽스처 릴리스 노트","windows":{{"x64":{},"arm64":{}}}}}"#,
            self.version,
            asset("x64", &self.x64_sha, self.x64_size),
            asset("arm64", &self.arm64_sha, self.arm64_size)
        )
    }
}

fn envelope_with(plain: &str, seed: &[u8; 32], format: &str) -> Vec<u8> {
    let payload = plain.as_bytes();
    serde_json::to_vec(&serde_json::json!({
        "format": format,
        "version": "1",
        "payload": STANDARD.encode(payload),
        "signature": STANDARD.encode(sign(seed, payload)),
    }))
    .unwrap()
}

fn envelope(plain: &str) -> Vec<u8> {
    envelope_with(plain, &FIXTURE_SEED, ENVELOPE_FORMAT)
}

const MANIFEST_URL: &str = "https://updates.example/latest.json";

fn service(transport: &Arc<FakeUpdateTransport>, state: &Path, key: Option<[u8; 32]>) -> Arc<Service> {
    Arc::new(Service::new(transport.clone(), state, key))
}

/// A transport over the fake whose manifest answer waits for a gate when one is set (C#'s `GatedHttp`).
struct Gated {
    inner: Arc<FakeUpdateTransport>,
    gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

impl UpdateTransport for Gated {
    fn get_capped(&self, url: &str, cap: usize, timeout: Duration) -> BoxFuture<'_, mighty_platform::Result<UpdateResponse>> {
        let gate = self.gate.lock().unwrap().clone();
        let url = url.to_string();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            self.inner.get_capped(&url, cap, timeout).await
        })
    }
    fn download(&self, request: DownloadRequest) -> BoxFuture<'_, mighty_platform::Result<Downloaded>> {
        self.inner.download(request)
    }
}

fn launches() -> (Arc<Mutex<Vec<InstallPlan>>>, LaunchHelper) {
    let launched = Arc::new(Mutex::new(Vec::new()));
    let record = launched.clone();
    (
        launched,
        Arc::new(move |plan: &InstallPlan| {
            record.lock().unwrap().push(plan.clone());
            Ok(())
        }),
    )
}

fn flag(value: bool) -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(move || value)
}

fn shared_flag(value: &Arc<Mutex<bool>>) -> Arc<dyn Fn() -> bool + Send + Sync> {
    let value = value.clone();
    Arc::new(move || *value.lock().unwrap())
}

fn options(architecture: &str, install: Option<&Path>) -> Options {
    Options {
        built_in_manifest_url: Some(MANIFEST_URL.into()),
        architecture: architecture.into(),
        install_directory: install.map(Path::to_path_buf),
        locale: ko(),
        ..Options::new("0.1.0")
    }
}

fn downloads(transport: &FakeUpdateTransport) -> usize {
    transport.asked().iter().filter(|url| url.as_str() != MANIFEST_URL).count()
}

/// A minimal PE header: e_lfanew at 0x3c, then the signature and the machine.
fn fake_executable(architecture: &str) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x100];
    bytes[0] = b'M';
    bytes[1] = b'Z';
    bytes[0x3c..0x40].copy_from_slice(&0x80i32.to_le_bytes());
    bytes[0x80] = b'P';
    bytes[0x81] = b'E';
    let machine: u16 = if architecture == "arm64" { 0xaa64 } else { 0x8664 };
    bytes[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
    bytes
}

enum Entry<'a> {
    File(&'a str, &'a [u8]),
    Link(&'a str, &'a str),
}

fn make_zip(folder: &Path, name: &str, entries: &[Entry]) -> PathBuf {
    let path = folder.join(name);
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for entry in entries {
        match entry {
            Entry::File(name, bytes) => {
                zip.start_file(*name, options).unwrap();
                zip.write_all(bytes).unwrap();
            }
            Entry::Link(name, target) => zip.add_symlink(*name, *target, options).unwrap(),
        }
    }
    zip.finish().unwrap();
    path
}

fn make_install(path: &Path, marker: &str) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::write(path.join(EXECUTABLE_NAME), marker).unwrap();
}

fn create_plan(staged: &Path, install: &Path, package: &Path, sha256: &str, size: i64, pid: i32) -> InstallPlan {
    InstallPlan::create(staged, install, package, sha256, size, pid, None, Utc::now())
}

/// C#'s `SwapFixture`: an install holding "old", a staged folder holding "new", a package and its plan.
fn swap_fixture(root: &Path, relaunch: bool) -> (PathBuf, PathBuf, InstallPlan) {
    let install = root.join("MightyCode");
    let staged = root.join("staged");
    make_install(&install, "old");
    make_install(&staged, "new");
    let package = root.join("package.zip");
    std::fs::write(&package, b"package").unwrap();
    let plan = create_plan(&staged, &install, &package, &sha256_of(b"package"), 7, 4242);
    (install, package, InstallPlan { relaunch, ..plan })
}

type Step<'a> = Box<dyn Fn(&Path, &Path) -> io::Result<()> + 'a>;
type Start<'a> = Box<dyn Fn(&Path) -> io::Result<()> + 'a>;
type Spoil = fn(&Path);

/// The replacement's seams for these checks (C#'s `waitForExit`, `startApp`, `copier`, `mover`, `runningAgain`, `moveRetryDelay`).
struct Fake<'a> {
    wait: bool,
    start: Start<'a>,
    copier: Option<Step<'a>>,
    mover: Option<Step<'a>>,
    running_again: Option<Box<dyn Fn() -> io::Result<bool> + 'a>>,
}

impl<'a> Fake<'a> {
    fn new(wait: bool, start: impl Fn(&Path) -> io::Result<()> + 'a) -> Fake<'a> {
        Fake { wait, start: Box::new(start), copier: None, mover: None, running_again: None }
    }
    fn copier(mut self, copier: impl Fn(&Path, &Path) -> io::Result<()> + 'a) -> Fake<'a> {
        self.copier = Some(Box::new(copier));
        self
    }
    fn mover(mut self, mover: impl Fn(&Path, &Path) -> io::Result<()> + 'a) -> Fake<'a> {
        self.mover = Some(Box::new(mover));
        self
    }
    fn running_again(mut self, check: impl Fn() -> io::Result<bool> + 'a) -> Fake<'a> {
        self.running_again = Some(Box::new(check));
        self
    }
}

impl Seams for Fake<'_> {
    fn wait_for_exit(&self, _plan: &InstallPlan) -> bool {
        self.wait
    }
    fn start_app(&self, executable: &Path) -> io::Result<()> {
        (self.start)(executable)
    }
    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        match &self.copier {
            Some(copier) => copier(from, to),
            None => replacement::copy_directory(from, to),
        }
    }
    fn move_dir(&self, from: &Path, to: &Path) -> io::Result<()> {
        match &self.mover {
            Some(mover) => mover(from, to),
            None => housekeeping::move_directory(from, to),
        }
    }
    fn running_again(&self, _plan: &InstallPlan) -> io::Result<bool> {
        self.running_again.as_ref().map_or(Ok(false), |check| check())
    }
    fn move_retry_delay(&self) -> Duration {
        Duration::from_millis(1)
    }
}

fn ok(_: &Path) -> io::Result<()> {
    Ok(())
}

fn kinds(result: &replacement::ReplacementResult) -> Vec<MoveKind> {
    result.moves.iter().map(|m| m.kind).collect()
}

// — Signature and manifest —

/// C#'s RFC 8032 §7.1 vectors: seed, public key, message, signature.
const RFC8032: [(&str, &str, &str, &str); 4] = [
    (
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    ),
    (
        "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
    (
        "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        "af82",
        "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    ),
    (
        "833fe62409237b9d62ec77587520911e9a759cec1d19755b7da901b96dca3d42",
        "ec172b93ad5e563bf4932c70e1245034c35467ef2efd4d64ebf819683467e2bf",
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        "dc2a4459e7369633a52b1bf277839a00201009a3efbf3ecb69bea2186c26b58909351fc9ac90b3ecfdfbc7c66431e0303dca179c138ac17ad9bef1177331a704",
    ),
];

/// Verification.cs "app update ed25519 matches the RFC 8032 vectors": the vectors verify, a flipped bit anywhere does not, and the
/// fixture signer the other checks use reproduces them. (C#'s verdict on every hard case: tests/app_update_parity.rs.)
#[test]
fn app_update_ed25519_matches_the_rfc_8032_vectors() {
    for (seed, key, message, sig) in RFC8032 {
        let (public, bytes, sig) = (hex(key), hex(message), hex(sig));
        assert!(signature::verify(&public, &bytes, &sig), "RFC 8032 vector {} did not verify", &key[..8]);
        let seed: [u8; 32] = hex(seed).try_into().unwrap();
        assert_eq!(public_key(&seed).to_vec(), public, "fixture signer derived another public key");
        assert_eq!(sign(&seed, &bytes).to_vec(), sig, "fixture signer produced another signature");

        let mut changed_message = if bytes.is_empty() { vec![1] } else { bytes.clone() };
        if !bytes.is_empty() {
            changed_message[0] ^= 1;
        }
        assert!(!signature::verify(&public, &changed_message, &sig), "a changed message must not verify");
        let mut changed_signature = sig.clone();
        changed_signature[0] ^= 1;
        assert!(!signature::verify(&public, &bytes, &changed_signature), "a changed signature must not verify");
        let mut changed_key = public.clone();
        changed_key[0] ^= 1;
        assert!(!signature::verify(&changed_key, &bytes, &sig), "another public key must not verify");
        assert!(!signature::verify(&public, &bytes, &sig[..63]), "a short signature must not verify");
        assert!(!signature::verify(&public[..31], &bytes, &sig), "a short public key must not verify");
    }
}

/// Verification.cs "app update manifest accepts a fixture signature and refuses everything else".
#[test]
fn app_update_manifest_accepts_a_fixture_signature_and_refuses_everything_else() {
    let plain = Plain::default().text();
    let manifest = manifest::parse(&envelope(&plain), &fixture_key()).unwrap();
    assert_eq!(manifest.version, "9.9.9", "the signed manifest version must be read");
    assert_eq!(manifest.windows.len(), 2, "both Windows architectures must be read");
    assert_eq!(manifest.notes.as_deref(), Some("픽스처 릴리스 노트"), "the release notes must survive the envelope");

    let refuse = |data: &[u8], key: &[u8; 32], what: &str| assert!(manifest::parse(data, key).is_err(), "{what} must be refused");
    refuse(plain.as_bytes(), &fixture_key(), "an unsigned manifest");
    refuse(&envelope(&plain), &other_key(), "a signature from another key");
    refuse(&envelope_with(&plain, &FIXTURE_SEED, "mightyclaude-update-v2"), &fixture_key(), "an unknown envelope format");

    // A tampered payload: the version is raised without re-signing.
    let signed: serde_json::Value = serde_json::from_slice(&envelope(&plain)).unwrap();
    let forged = serde_json::to_vec(&serde_json::json!({
        "format": ENVELOPE_FORMAT,
        "payload": STANDARD.encode(Plain { version: "99.0.0", ..Plain::default() }.text()),
        "signature": signed["signature"],
    }))
    .unwrap();
    refuse(&forged, &fixture_key(), "a tampered payload");
    refuse(
        br#"{"format":"mightyclaude-update-v1","payload":"not base64!!","signature":"AA=="}"#,
        &fixture_key(),
        "a payload that is not base64",
    );
}

/// Verification.cs "app update manifest refuses an asset without sha256 or size".
#[test]
fn app_update_manifest_refuses_an_asset_without_sha256_or_size() {
    let parse = |plain: Plain| manifest::parse(&envelope(&plain.text()), &fixture_key()).unwrap();
    let both = parse(Plain::default());
    assert!(both.windows.contains_key("x64") && both.windows.contains_key("arm64"), "a complete asset must be kept");
    let no_sha = parse(Plain { x64_sha: String::new(), ..Plain::default() });
    assert!(!no_sha.windows.contains_key("x64"), "an asset without sha256 must be refused");
    assert!(no_sha.windows.contains_key("arm64"), "the other architecture must be unaffected");
    assert!(!parse(Plain { arm64_size: None, ..Plain::default() }).windows.contains_key("arm64"), "an asset without size must be refused");
    assert!(
        !parse(Plain { arm64_size: Some(0), ..Plain::default() }).windows.contains_key("arm64"),
        "an asset with size 0 must be refused"
    );
    assert!(parse(Plain { scheme: "http", ..Plain::default() }).windows.is_empty(), "an http package address must be refused");

    assert!(!manifest::allowed("http://updates.example/latest.json"), "http must not be allowed");
    assert!(!manifest::allowed("file:///tmp/latest.json"), "a file url must not be allowed");
    assert!(!manifest::allowed("https:///latest.json"), "an https url without a host must not be allowed");
    assert!(manifest::allowed("https://updates.example/latest.json"), "an https url must be allowed");
}

/// Verification.cs "app update version comparison orders releases and pre-releases".
#[test]
fn app_update_version_comparison_orders_releases_and_pre_releases() {
    assert!(version::is_newer("1.2.0", "1.2.0-beta.2"), "a release must beat its pre-release");
    assert!(version::is_newer("1.2.0-beta.2", "1.1.9"), "a pre-release must beat the older release");
    assert!(version::is_newer("1.2", "1.1.9"), "a missing component must read as 0");
    assert!(!version::is_newer("1.2.0", "1.2"), "1.2 and 1.2.0 are the same version");
    assert_eq!(version::normalized("v1.2.3").as_deref(), Some("1.2.3"), "a leading v must be dropped");
    assert_eq!(version::normalized("1.2.3-beta.1").as_deref(), Some("1.2.3-beta.1"), "a pre-release tag must survive");
    assert_eq!(version::normalized("not-a-version"), None, "a non-version must be refused");
    assert_eq!(version::normalized(&"9".repeat(80)), None, "an over-long version must be refused");
}

// — Transport —

/// Verification.cs "app update without a public key there is no check at all".
#[tokio::test]
async fn app_update_without_a_public_key_there_is_no_check_at_all() {
    let directory = Temp::new();
    let transport = Arc::new(FakeUpdateTransport::default());
    transport.route(MANIFEST_URL, FakeRoute::ok(envelope(&Plain::default().text())));
    let keyless = service(&transport, directory.path(), None);
    assert!(!keyless.verifies_signatures(), "a build without a key must not claim to verify signatures");
    let refused = keyless.check(Some(MANIFEST_URL), "0.1.0").await.expect_err("a key-less build must refuse to check");
    let notice = text(&ko(), "NoPublicKeyNotice");
    assert!(
        matches!(&refused, ServiceError::Refused(error) if error.message(&ko()) == notice),
        "the refusal must be the section's own sentence"
    );
    assert!(transport.asked().is_empty(), "a key-less build must not even ask for the manifest");

    // A key that is not 32 raw bytes is the same as no key.
    assert_eq!(mighty_updates::app_update::build::public_key(Some("AQID")), None, "a malformed key must read as no key");

    let coordinator = Coordinator::new(keyless, options("x64", None));
    coordinator.check(None).await;
    let view = coordinator.describe(&ko(), |_| "00:00".into());
    assert!(!view.section_enabled, "the section must be disabled without a key");
    assert!(!view.button_enabled, "the check button must be disabled without a key");
    assert_eq!(view.status_text, notice, "the one status sentence must be shown");
    assert!(transport.asked().is_empty(), "the coordinator must not reach the network without a key");
}

/// Verification.cs "app update a built-in address ignores a user address".
#[tokio::test]
async fn app_update_a_built_in_address_ignores_a_user_address() {
    let directory = Temp::new();
    let transport = Arc::new(FakeUpdateTransport::default());
    transport.route(MANIFEST_URL, FakeRoute::ok(envelope(&Plain::default().text())));
    let service = service(&transport, directory.path(), Some(fixture_key()));

    let stamped = Coordinator::new(service.clone(), options("x64", None));
    assert_eq!(
        stamped.effective_manifest_url(Some("https://attacker.example/latest.json")).as_deref(),
        Some(MANIFEST_URL),
        "a build with an address must ignore a user address"
    );
    let view = stamped.describe(&ko(), |_| "00:00".into());
    assert_eq!(
        view.built_in_address_line,
        Some(text(&ko(), "BuiltInAddressTemplate").replace("{address}", MANIFEST_URL)),
        "the built-in address line must name the address the build carries"
    );
    assert!(!view.address_field_enabled, "the address field must be disabled when the build carries an address");

    stamped.check(Some("https://attacker.example/latest.json")).await;
    assert_eq!(transport.asked(), vec![MANIFEST_URL.to_string()], "only the built-in address may be requested");

    let open = Coordinator::new(service, Options { built_in_manifest_url: None, ..options("x64", None) });
    assert_eq!(
        open.effective_manifest_url(Some("https://typed.example/latest.json")).as_deref(),
        Some("https://typed.example/latest.json"),
        "without a built-in address a typed https address is used"
    );
    assert_eq!(open.effective_manifest_url(Some("http://typed.example/latest.json")), None, "a typed http address is refused");
    assert_eq!(open.effective_manifest_url(Some("  ")), None, "an empty address is refused");
}

/// Verification.cs "app update transport refuses a non-https hop".
#[tokio::test]
async fn app_update_transport_refuses_a_non_https_hop() {
    let directory = Temp::new();
    let signed = envelope(&Plain::default().text());

    let downgrade = Arc::new(FakeUpdateTransport::default());
    downgrade.route(MANIFEST_URL, FakeRoute::redirect(302, "http://updates.example/latest.json"));
    downgrade.route("http://updates.example/latest.json", FakeRoute::ok(signed.clone()));
    let refused = service(&downgrade, directory.path(), Some(fixture_key()))
        .check(Some(MANIFEST_URL), "0.1.0")
        .await
        .expect_err("an http hop must be refused");
    let ServiceError::Refused(error) = refused else { panic!("an http hop must be refused, not cancelled") };
    assert!(error.message(&ko()).contains("https"), "the refusal must name the transport rule: {}", error.message(&ko()));
    assert_eq!(downgrade.asked().len(), 1, "the http hop must not be taken");

    let mirrored = Arc::new(FakeUpdateTransport::default());
    mirrored.route(MANIFEST_URL, FakeRoute::redirect(302, "https://mirror.example/latest.json"));
    mirrored.route("https://mirror.example/latest.json", FakeRoute::ok(signed.clone()));
    let availability = service(&mirrored, directory.path(), Some(fixture_key())).check(Some(MANIFEST_URL), "0.1.0").await.unwrap();
    assert!(availability.is_newer(), "an https hop must be followed");
    assert_eq!(mirrored.asked().len(), 2, "the https hop must be taken exactly once");

    let plain = Arc::new(FakeUpdateTransport::default());
    plain.route("http://updates.example/latest.json", FakeRoute::ok(signed));
    let service = service(&plain, directory.path(), Some(fixture_key()));
    assert!(service.check(Some("http://updates.example/latest.json"), "0.1.0").await.is_err(), "an http manifest address must be refused");
    assert!(service.check(Some("file:///tmp/latest.json"), "0.1.0").await.is_err(), "a file manifest address must be refused");
}

// — Download —

/// Verification.cs "app update download verifies the bytes on disk and cleans up".
#[tokio::test]
async fn app_update_download_verifies_the_bytes_on_disk_and_cleans_up() {
    let directory = Temp::new();
    let package = "p".repeat(5000).into_bytes();
    let url = "https://updates.example/9.9.9/MightyCode-windows-x64.zip";
    let transport = Arc::new(FakeUpdateTransport::default());
    transport.route(url, FakeRoute::ok(package.clone()));
    let service = service(&transport, directory.path(), Some(fixture_key()));
    let never = CancellationToken::new();
    let silent: Arc<dyn Fn(f64) + Send + Sync> = Arc::new(|_| {});

    // A folder for an older version must be gone after the download.
    let stale = service.updates_directory().join("1.0.0");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("old.zip"), "stale").unwrap();

    let good = Asset { url: url.into(), sha256: sha256_of(&package), size: package.len() as u64 };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    let path = service.download(&good, "9.9.9", Arc::new(move |fraction| record.lock().unwrap().push(fraction)), &never).await.unwrap();
    assert!(path.is_file(), "the verified package must be on disk");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), package.len() as u64, "the package on disk must be the package");
    assert!(!stale.exists(), "another version folder must be removed");
    assert!(seen.lock().unwrap().last().is_some_and(|last| (last - 1.0).abs() < 0.0001), "progress must end at 1");

    // The same version's package already on disk and matching the signed asset is used again: nothing is downloaded twice.
    let requests = transport.asked().len();
    assert_eq!(service.download(&good, "9.9.9", silent.clone(), &never).await.unwrap(), path);
    assert_eq!(transport.asked().len(), requests, "a matching package already on disk must not be downloaded again");
    // One whose bytes changed since is downloaded afresh.
    std::fs::write(&path, "q".repeat(5000)).unwrap();
    assert_eq!(service.download(&good, "9.9.9", silent.clone(), &never).await.unwrap(), path);
    assert!(
        transport.asked().len() == requests + 1 && std::fs::read(&path).unwrap() == package,
        "a package whose bytes changed must be downloaded again"
    );

    let version_folder = service.updates_directory().join("9.9.9");
    let refusal = |result: Result<PathBuf, ServiceError>| match result {
        Err(ServiceError::Refused(error)) => error.message(&ko()),
        other => panic!("refused expected, got {other:?}"),
    };
    let wrong_digest = Asset { sha256: "c".repeat(64), ..good.clone() };
    let digest_message = refusal(service.download(&wrong_digest, "9.9.9", silent.clone(), &never).await);
    assert!(digest_message.contains("SHA-256"), "the refusal must name the digest: {digest_message}");
    assert!(!version_folder.exists(), "a mismatching download must be deleted");

    let wrong_size = Asset { size: package.len() as u64 - 1, ..good.clone() };
    refusal(service.download(&wrong_size, "9.9.9", silent.clone(), &never).await);
    assert!(!version_folder.exists(), "an oversized download must be deleted");

    let over_cap = Asset { size: MAX_PACKAGE_BYTES + 1, ..good.clone() };
    let cap_message = refusal(service.download(&over_cap, "9.9.9", silent.clone(), &never).await);
    assert!(cap_message.contains("너무 큽니다"), "the refusal must name the cap: {cap_message}");

    refusal(service.download(&Asset { url: "http://updates.example/p.zip".into(), ..good.clone() }, "9.9.9", silent.clone(), &never).await);

    // A cancelled download leaves nothing behind.
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        service.download(&good, "9.9.9", silent, &cancelled).await,
        Err(ServiceError::Cancelled),
        "a cancelled download must not finish"
    );
    assert!(!version_folder.exists(), "a cancelled download must leave nothing behind");
}

// — Staging —

/// Verification.cs "app update staging refuses an escaping entry or the wrong package".
#[test]
fn app_update_staging_refuses_an_escaping_entry_or_the_wrong_package() {
    let directory = Temp::new();
    let exe = fake_executable("x64");
    let stage_beside = |package: &Path, architecture: &str| stage::stage(package, &package.parent().unwrap().join("staged"), architecture);
    let good = make_zip(directory.path(), "good.zip", &[Entry::File("MightyCode.exe", &exe), Entry::File("Assets/mightycode.png", b"png")]);
    let staged = stage_beside(&good, "x64").unwrap();
    assert!(staged.join("MightyCode.exe").is_file(), "the executable must be staged at the root");
    assert!(staged.join("Assets").join("mightycode.png").is_file(), "a nested file must be staged");

    let refuse = |name: &str, entries: &[Entry], architecture: &str, what: &str| {
        let folder = directory.path().join(name.trim_end_matches(".zip"));
        std::fs::create_dir_all(&folder).unwrap();
        let path = make_zip(&folder, name, entries);
        assert!(stage_beside(&path, architecture).is_err(), "{what} must be refused");
    };
    refuse(
        "escape.zip",
        &[Entry::File("MightyCode.exe", &exe), Entry::File("../escaped.txt", b"nope")],
        "x64",
        "an entry with a .. segment",
    );
    refuse(
        "absolute.zip",
        &[Entry::File("MightyCode.exe", &exe), Entry::File("/etc/evil", b"nope")],
        "x64",
        "an entry with an absolute path",
    );
    // S_IFLNK (0xA000) in the high 16 bits marks a symbolic link.
    refuse("link.zip", &[Entry::File("MightyCode.exe", &exe), Entry::Link("shortcut", "/etc/passwd")], "x64", "a link entry");
    refuse("none.zip", &[Entry::File("Readme.txt", b"hello")], "x64", "a package with no MightyCode.exe");
    refuse(
        "two.zip",
        &[Entry::File("MightyCode.exe", &exe), Entry::File("tools/MightyCode.exe", &exe)],
        "x64",
        "a package with two executables",
    );
    refuse("deep.zip", &[Entry::File("app/bin/MightyCode.exe", &exe)], "x64", "an executable buried below the expected place");

    // The real Windows zip may pack the app one folder down; staging then returns that folder, not the extraction root.
    let nested_folder = directory.path().join("nested");
    std::fs::create_dir_all(&nested_folder).unwrap();
    let nested = make_zip(
        &nested_folder,
        "nested.zip",
        &[Entry::File("native-windows-x64/MightyCode.exe", &exe), Entry::File("native-windows-x64/Assets/mightycode.png", b"png")],
    );
    let app_folder = stage_beside(&nested, "x64").unwrap();
    assert_eq!(app_folder.file_name().unwrap(), "native-windows-x64", "staging must return the app folder");
    assert!(app_folder.join("MightyCode.exe").is_file(), "the app folder must hold the executable");

    refuse("arm.zip", &[Entry::File("MightyCode.exe", &fake_executable("arm64"))], "x64", "a package for another architecture");
    let arm_folder = directory.path().join("arm-ok");
    std::fs::create_dir_all(&arm_folder).unwrap();
    let arm_package = make_zip(&arm_folder, "arm-ok.zip", &[Entry::File("MightyCode.exe", &fake_executable("arm64"))]);
    assert!(stage_beside(&arm_package, "arm64").unwrap().is_dir(), "an arm64 package must stage on arm64");

    // This version's package carries MightyCode.exe: it stages, and it is the helper the running app starts.
    let renamed_folder = directory.path().join("renamed");
    std::fs::create_dir_all(&renamed_folder).unwrap();
    let renamed = make_zip(&renamed_folder, "renamed.zip", &[Entry::File("native-windows-x64/MightyCode.exe", &exe)]);
    let staged_renamed = stage_beside(&renamed, "x64").unwrap();
    assert!(staged_renamed.join("MightyCode.exe").is_file(), "a package of the renamed version must stage");
    let plan = create_plan(&staged_renamed, &directory.path().join("install"), &renamed, &"a".repeat(64), 1, 1);
    assert_eq!(
        plan.staged_helper_executable().file_name().unwrap(),
        "MightyCode.exe",
        "the renamed version's own executable must be the helper"
    );
    // A package of a version from before the rename (MightyClaude.exe only) is no update for this one.
    refuse("legacy.zip", &[Entry::File("MightyClaude.exe", &exe)], "x64", "a package with only the executable from before the rename");
}

// — Replacement —

/// Verification.cs "app update replacement verifies again and replaces the install".
#[test]
fn app_update_replacement_verifies_again_and_replaces_the_install() {
    let root = Temp::new();
    let install = root.path().join("MightyCode");
    let staged = root.path().join("staged");
    make_install(&install, "old");
    make_install(&staged, "new");
    let package = root.path().join("package.zip");
    std::fs::write(&package, b"package").unwrap();

    let plan = create_plan(&staged, &install, &package, &sha256_of(b"package"), 7, 4242);
    assert!(plan.is_valid(), "the plan must be valid");
    assert_eq!(plan.helper_arguments()[0], HELPER_FLAG, "the helper argument list must start with the flag");
    assert_eq!(plan::try_parse(&plan.helper_arguments()), Some(plan.clone()), "the helper must rebuild the same plan from its arguments");
    assert_eq!(
        replacement::planned_moves(&plan).iter().map(|m| m.kind).collect::<Vec<_>>(),
        [MoveKind::Backup, MoveKind::Install],
        "the plan of moves is backup then install"
    );
    assert_eq!(replacement::rollback(&plan).kind, MoveKind::Rollback, "the rollback move undoes the backup");
    assert!(replacement::should_rollback(true, false), "a taken backup with no install must roll back");
    assert!(!replacement::should_rollback(true, true), "a finished install must not roll back");
    assert!(
        !HELPER_ENVIRONMENT.iter().any(|(key, _)| ["TOKEN", "HOME", "USER"].iter().any(|word| key.to_ascii_uppercase().contains(word))),
        "the helper environment must carry nothing about the user"
    );

    let backup_present_when_started = RefCell::new(false);
    let started = RefCell::new(Vec::new());
    let result = replacement::run(
        &plan,
        &Fake::new(true, |path| {
            *backup_present_when_started.borrow_mut() = plan.backup_directory.is_dir();
            started.borrow_mut().push(path.to_path_buf());
            Ok(())
        }),
        &ko(),
    );
    assert!(result.replaced && !result.rolled_back, "the replacement must succeed: {:?}", result.error);
    assert_eq!(read(install.join("MightyCode.exe")), "new", "the new app must be installed");
    assert!(staged.is_dir(), "the staged folder must remain: the helper runs from it");
    assert_eq!(*started.borrow(), [install.join("MightyCode.exe")], "the new app must be started once");
    assert!(*backup_present_when_started.borrow(), "the backup must still exist when the new app starts");
    assert!(!plan.backup_directory.exists(), "the backup must be removed once the new app has started");
    assert_eq!(kinds(&result), [MoveKind::Backup, MoveKind::Install], "the moves must be the plan");

    // A backup the helper could not delete is cleared by the next start.
    make_install(&plan.backup_directory, "old");
    assert_eq!(housekeeping::prune_backups(&install, Utc::now()), 1, "a leftover backup must be pruned");
    assert!(!plan.backup_directory.exists(), "the leftover backup must be gone");
    assert!(install.is_dir(), "pruning must not touch the install");
    assert_eq!(housekeeping::prune_backups(&install, Utc::now()), 0, "pruning again must find nothing");
}

/// Verification.cs "app update replacement leaves the install untouched when it cannot proceed".
#[test]
fn app_update_replacement_leaves_the_install_untouched_when_it_cannot_proceed() {
    let root = Temp::new();
    let install = root.path().join("MightyCode");
    let staged = root.path().join("staged");
    make_install(&install, "old");
    make_install(&staged, "new");
    let package = root.path().join("package.zip");
    std::fs::write(&package, b"package").unwrap();
    let plan = create_plan(&staged, &install, &package, &sha256_of(b"package"), 7, 4242);

    // The package is swapped underneath after the download verified it.
    std::fs::write(&package, b"swapped").unwrap();
    let tampered = replacement::run(&plan, &Fake::new(true, ok), &ko());
    assert!(!tampered.replaced && !tampered.rolled_back, "a changed package must not be installed");
    assert!(
        tampered.error.as_deref().unwrap_or_default().contains("재검증"),
        "the refusal must name the re-verification: {:?}",
        tampered.error
    );
    assert_eq!(read(install.join("MightyCode.exe")), "old", "the install must be untouched");
    assert!(tampered.moves.is_empty(), "nothing may be moved when the digest differs");

    std::fs::write(&package, b"package").unwrap();
    let still_running = replacement::run(&plan, &Fake::new(false, ok), &ko());
    assert!(!still_running.replaced, "the swap must not start while the app runs");
    assert_eq!(read(install.join("MightyCode.exe")), "old", "the install must be untouched");

    // Starting the new app fails: the old one is put back and started.
    let restarted = RefCell::new(Vec::new());
    let rolled = replacement::run(
        &plan,
        &Fake::new(true, |path| {
            restarted.borrow_mut().push(path.to_path_buf());
            Err(io::Error::other("새 앱을 시작하지 못했습니다."))
        }),
        &ko(),
    );
    assert!(!rolled.replaced && rolled.rolled_back, "a failed start must roll back: {:?}", rolled.error);
    assert_eq!(read(install.join("MightyCode.exe")), "old", "the old app must be back");
    assert_eq!(kinds(&rolled), [MoveKind::Backup, MoveKind::Install, MoveKind::Rollback], "the rollback move must be recorded");
    assert_eq!(restarted.borrow().len(), 2, "the old app must be started after the rollback");

    let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        plan::try_parse(&args(&["--update-helper", "--staged", &staged.to_string_lossy()])),
        None,
        "a helper argument list missing values must not produce a plan"
    );
    assert_eq!(plan::try_parse(&args(&["--smoke-test"])), None, "another argument list must not produce a plan");
}

// — Automatic check and the section —

/// Verification.cs "app update automatic check happens every six hours".
#[tokio::test]
async fn app_update_automatic_check_happens_every_six_hours() {
    assert!(AppSnapshot::default().app_update_auto_check, "the macOS default is on");
    assert_eq!(AppSnapshot::default().version, 1, "the snapshot version must stay 1");
    let off = AppSnapshot { app_update_auto_check: false, ..AppSnapshot::default() };
    let round = AppSnapshot::read(&serde_json::from_str(&off.to_json()).unwrap()).unwrap();
    assert!(!round.app_update_auto_check, "the saved preference must survive a round trip");
    assert!(
        AppSnapshot::read(&serde_json::json!({"version": 1})).unwrap().app_update_auto_check,
        "a file written before the field existed must load with the macOS default"
    );

    let now = Utc::now();
    let hours = |h: i64| chrono::Duration::hours(h);
    assert!(Coordinator::is_due(true, true, None, now, None), "a first run is due");
    assert!(Coordinator::is_due(true, true, Some(now - hours(6)), now, None), "a check six hours old is due");
    assert!(!Coordinator::is_due(true, true, Some(now - hours(5)), now, None), "a check inside six hours is not due");
    assert!(
        !Coordinator::is_due(true, true, Some(now - hours(7)), now, Some(now - hours(1))),
        "a recent failed attempt holds the next check back"
    );
    assert!(!Coordinator::is_due(false, true, None, now, None), "the preference off means never");
    assert!(!Coordinator::is_due(true, false, None, now, None), "no public key means never");

    let directory = Temp::new();
    let transport = Arc::new(FakeUpdateTransport::default());
    transport.route(MANIFEST_URL, FakeRoute::ok(envelope(&Plain::default().text())));
    let coordinator = Coordinator::new(service(&transport, directory.path(), Some(fixture_key())), options("x64", None));
    assert!(coordinator.check_automatically_if_due(None, true, None, now).await, "the first automatic check runs");
    assert_eq!(coordinator.state().phase, Phase::Available, "the automatic check must find the new version");
    assert_eq!(coordinator.last_attempt(), Some(now), "the attempt is remembered");
    assert!(
        !coordinator.check_automatically_if_due(None, true, None, now + hours(1)).await,
        "an attempt inside six hours holds the next one back even with no saved check"
    );
    assert!(!coordinator.check_automatically_if_due(None, true, Some(now), now).await, "a second check inside six hours is skipped");
    assert_eq!(transport.asked().len(), 1, "only one manifest request may be made");
    assert_eq!(coordinator.state().phase, Phase::Available, "with the switch off a found version waits for the download button");
}

/// Verification.cs "app update section shows the macOS copy for every phase".
#[test]
fn app_update_section_shows_the_macos_copy_for_every_phase() {
    let ko = ko();
    let states = smoke::fixture_states(&ko);
    let view = |state: &UpdateState, automatic: bool| describe(&ko, &state.view(), true, None, |_| "00:00".into(), automatic);
    let statuses: Vec<String> = states.iter().map(|s| view(s, true).status_text).collect();
    let buttons: Vec<String> = states.iter().map(|s| view(s, true).button_label).collect();
    let t = |field: &str| text(&ko, field);

    assert_eq!(statuses[0], t("Checking"), "the checking sentence must be the macOS one");
    assert_eq!(statuses[1], "새 버전 9.9.9 이 있습니다.", "the available sentence must name the version");
    assert_eq!(statuses[2], "42% 받는 중…", "the download sentence must carry the percentage");
    assert_eq!(statuses[3], t("StagingProgress"), "the staging sentence must be the macOS one");
    assert_eq!(statuses[4], t("StagedTemplate").replace("{version}", "9.9.9"), "a staged update says it installs at the next launch");
    assert_eq!(
        view(&states[4], false).status_text,
        t("ReadyTemplate").replace("{version}", "9.9.9"),
        "with the switch off a staged update waits for restart now, as on macOS"
    );
    assert_eq!(statuses[5], t("Installing"), "the installing sentence must be the macOS one");
    assert_eq!(statuses[6], t("UpToDate"), "the up-to-date sentence must be the macOS one");
    assert_eq!(statuses[7], "업데이트 확인 중 오류가 발생했습니다.", "the failed state shows the error message");
    assert_eq!(statuses[8], statuses[4], "a failed check beside a staged update keeps the staged sentence");
    let kept = view(&states[8], true);
    assert!(
        kept.kept_error.as_deref() == Some("업데이트 확인 중 오류가 발생했습니다.") && kept.retry_notice == Some(t("RetryNotice")),
        "the failure and the retry notice sit beside it"
    );
    assert_eq!(
        buttons,
        [
            "InProgressButton",
            "DownloadButton",
            "CancelButton",
            "InProgressButton",
            "RestartNowButton",
            "InProgressButton",
            "CheckButton",
            "CheckButton",
            "RestartNowButton"
        ]
        .map(t),
        "the buttons must follow the macOS order"
    );

    let idle = view(&UpdateState::default(), true);
    assert_eq!(idle.status_text, t("NotCheckedYet"), "an unchecked build says so");
    assert!(idle.button_label == t("CheckButton") && idle.button_enabled, "the check button is offered when idle");
    assert_eq!(idle.signature_notice, t("SignatureVerified"), "a build with a key says it verifies signatures");
    let checked = describe(
        &ko,
        &UpdateState { checked_at: Some(DateTime::UNIX_EPOCH), ..UpdateState::default() }.view(),
        true,
        None,
        |_| "09:30".into(),
        true,
    );
    assert_eq!(checked.status_text, "마지막 확인 09:30", "the last check time is shown");
    let available = UpdateState { phase: Phase::Available, availability: states[1].availability.clone(), ..UpdateState::default() };
    assert_eq!(view(&available, true).notes, None, "fixture notes are absent, so no note line is shown");

    // The smoke decision accepts what the section renders and refuses less.
    let current = RefCell::new(true);
    let read_switch = || *current.borrow();
    let outcome = smoke::decide(&ko, &smoke::expected_statuses(&ko), &smoke::expected_buttons(&ko), true, &read_switch, &mut |value| {
        *current.borrow_mut() = value
    })
    .unwrap();
    assert!(outcome.restored && *current.borrow(), "the smoke run must put the switch back");
    assert!(
        smoke::decide(&ko, &[], &smoke::expected_buttons(&ko), true, &read_switch, &mut |value| *current.borrow_mut() = value).is_err(),
        "a section that rendered nothing must fail the smoke run"
    );
    assert!(
        smoke::decide(&ko, &smoke::expected_statuses(&ko), &smoke::expected_buttons(&ko), false, &read_switch, &mut |value| *current
            .borrow_mut() =
            value)
        .is_err(),
        "a key-less build that still checked must fail the smoke run"
    );
    assert_eq!(serde_json::to_value(&outcome).unwrap()["refusedWithoutPublicKey"], true, "the outcome keeps C#'s names");
}

// — Helper location and copy-based replacement —

/// Verification.cs "app update helper executable is inside the staged folder".
#[test]
fn app_update_helper_executable_is_inside_the_staged_folder() {
    let root = Temp::new();
    let staged = root.path().join("staged");
    let plan = create_plan(&staged, &root.path().join("install"), &root.path().join("pkg.zip"), &"a".repeat(64), 1, 1);
    assert!(plan.staged_helper_executable().starts_with(&staged), "the staged helper executable must be inside the staged directory");
    assert_eq!(plan.staged_helper_executable().file_name().unwrap(), EXECUTABLE_NAME, "it must name the application executable");
}

/// Verification.cs "app update helper copies the staged folder to the install path".
#[test]
fn app_update_helper_copies_the_staged_folder_to_the_install_path() {
    let root = Temp::new();
    let (install, _, plan) = swap_fixture(root.path(), true);
    std::fs::write(plan.staged_directory.join("extra.dll"), "extra").unwrap();
    std::fs::create_dir_all(plan.staged_directory.join("Assets")).unwrap();
    std::fs::write(plan.staged_directory.join("Assets").join("icon.png"), "icon").unwrap();

    let result = replacement::run(&plan, &Fake::new(true, ok), &ko());
    assert!(result.replaced && !result.rolled_back, "the copy-based replacement must succeed: {:?}", result.error);
    assert_eq!(read(install.join("MightyCode.exe")), "new", "the install must hold the new version");
    assert!(install.join("extra.dll").is_file(), "every file in the staged folder must be copied to the install");
    assert!(install.join("Assets").join("icon.png").is_file(), "nested files must be copied to the install");
    assert!(plan.staged_directory.is_dir(), "the staged folder must remain after the replacement: the helper runs from it");
}

/// Verification.cs "app update helper rolls back after a copy that fails half way".
#[test]
fn app_update_helper_rolls_back_after_a_copy_that_fails_half_way() {
    let root = Temp::new();
    let (install, _, plan) = swap_fixture(root.path(), true);
    let started = RefCell::new(Vec::new());
    let half_way = |source: &Path, destination: &Path| {
        std::fs::create_dir_all(destination)?;
        let exe = source.join(EXECUTABLE_NAME);
        if exe.is_file() {
            std::fs::copy(exe, destination.join(EXECUTABLE_NAME))?;
        }
        Err(io::Error::other("디스크 공간이 부족해 복사를 중단합니다."))
    };
    let result = replacement::run(
        &plan,
        &Fake::new(true, |path| {
            started.borrow_mut().push(path.to_path_buf());
            Ok(())
        })
        .copier(half_way),
        &ko(),
    );
    assert!(
        !result.replaced && !result.rolled_back && result.moves.is_empty(),
        "a half-way copy must fail before anything is moved: {:?}",
        result.error
    );
    assert_eq!(read(install.join("MightyCode.exe")), "old", "the old install must be untouched");
    assert!(!plan.new_directory().exists(), "the partial copy beside the install must be removed");
    assert!(!plan.backup_directory.exists(), "no backup may be taken");
    assert!(*started.borrow() == [install.join("MightyCode.exe")] && result.started, "the app the user had is started again");
    assert!(result.error.as_deref().unwrap_or_default().contains("디스크"), "the error must name the copy failure: {:?}", result.error);
}

/// Verification.cs "app update helper waits for the app to quit before it moves anything".
#[test]
fn app_update_helper_waits_for_the_app_to_quit_before_it_moves_anything() {
    let root = Temp::new();
    let (install, _, plan) = swap_fixture(root.path(), true);
    let started = RefCell::new(0);
    let result = replacement::run(
        &plan,
        &Fake::new(false, |_| {
            *started.borrow_mut() += 1;
            Ok(())
        }),
        &ko(),
    );
    assert!(!result.replaced && !result.rolled_back, "a still-running app must stop the replacement");
    assert!(result.moves.is_empty(), "nothing may be moved while the app is still running");
    assert_eq!(*started.borrow(), 0, "no app may be started while the old one still runs");
    assert_eq!(read(install.join("MightyCode.exe")), "old", "the install must be untouched while the app is still running");
    assert!(!plan.backup_directory.exists(), "no backup may be taken while the app is still running");
    assert!(
        result.error.as_deref().unwrap_or_default().contains("종료"),
        "the refusal must name the app that did not quit: {:?}",
        result.error
    );
}

/// Verification.cs "app update helper verifies the package again before the install is moved aside".
#[test]
fn app_update_helper_verifies_the_package_again_before_the_install_is_moved_aside() {
    let root = Temp::new();
    let (install, package, plan) = swap_fixture(root.path(), true);
    // The package is tampered with after the plan was built.
    std::fs::write(&package, b"tampered").unwrap();
    let result = replacement::run(&plan, &Fake::new(true, ok), &ko());
    assert!(!result.replaced && !result.rolled_back, "a changed package must stop the replacement");
    assert!(result.moves.is_empty(), "a changed package must be caught before the first move");
    assert_eq!(read(install.join("MightyCode.exe")), "old", "the install must be untouched when the re-verification fails");
    assert!(!plan.backup_directory.exists(), "no backup may be taken when the re-verification fails");
    assert!(
        result.error.as_deref().unwrap_or_default().contains("SHA-256"),
        "the refusal must name the failed re-verification: {:?}",
        result.error
    );

    // A package that vanished between the download and the swap is refused too.
    std::fs::remove_file(&package).unwrap();
    let gone = replacement::run(&plan, &Fake::new(true, ok), &ko());
    assert!(!gone.replaced && gone.moves.is_empty(), "a missing package must stop the replacement");
    assert_eq!(read(install.join("MightyCode.exe")), "old", "the install must be untouched when the package is gone");
}

/// Verification.cs "app update helper keeps the backup until the new app has started".
#[test]
fn app_update_helper_keeps_the_backup_until_the_new_app_has_started() {
    let root = Temp::new();
    let (_, package, plan) = swap_fixture(root.path(), true);
    let at_start = RefCell::new((false, false));
    let result = replacement::run(
        &plan,
        &Fake::new(true, |path| {
            *at_start.borrow_mut() = (plan.backup_directory.is_dir(), read(path) == "new");
            Ok(())
        }),
        &ko(),
    );
    assert!(result.replaced, "the replacement must succeed: {:?}", result.error);
    let (backup_at_start, new_version_at_start) = *at_start.borrow();
    assert!(backup_at_start, "the backup must still exist when the new app is started");
    assert!(new_version_at_start, "the new app must already be in the install path when it is started");
    assert!(result.started && result.backup_removed, "the backup is removed only after the new app started");
    assert!(!plan.backup_directory.exists(), "the backup is cleared once the new app runs");

    // Now the new app refuses to start: the old one must come back.
    let second = root.path().join("second");
    let (second_install, second_staged) = (second.join("MightyCode"), second.join("staged"));
    make_install(&second_install, "old");
    make_install(&second_staged, "new");
    let second_plan = create_plan(&second_staged, &second_install, &package, &sha256_of(b"package"), 7, 4242);
    let attempts = RefCell::new(0);
    let restart = replacement::run(
        &second_plan,
        &Fake::new(true, |_| {
            *attempts.borrow_mut() += 1;
            if *attempts.borrow() == 1 { Err(io::Error::other("새 앱을 시작하지 못했습니다.")) } else { Ok(()) }
        }),
        &ko(),
    );
    assert!(!restart.replaced && restart.rolled_back, "a new app that will not start must roll back: {:?}", restart.error);
    assert!(restart.started, "the old app must be started again after the rollback");
    assert_eq!(read(second_install.join("MightyCode.exe")), "old", "the old install must be back in place");
    assert!(!second_plan.backup_directory.exists(), "the backup is renamed back, not left behind");
    assert!(second_staged.is_dir(), "the staged folder the helper runs from must survive the rollback");
}

/// Verification.cs "app update pipeline runs from a fixture-signed manifest to a ready install plan".
#[tokio::test]
async fn app_update_pipeline_runs_from_a_fixture_signed_manifest_to_a_ready_install_plan() {
    let root = Temp::new();
    let state = root.path().join("state");
    let install = root.path().join("MightyCode");
    make_install(&install, "old");
    let package_path = make_zip(root.path(), "MightyCode-windows-x64.zip", &[Entry::File("MightyCode.exe", &fake_executable("x64"))]);
    let package = std::fs::read(&package_path).unwrap();
    let manifest_text = format!(
        r#"{{"version":"9.9.9","windows":{{"x64":{{"url":"https://updates.example/9.9.9/MightyCode-windows-x64.zip","sha256":"{}","size":{}}}}}}}"#,
        sha256_of(&package),
        package.len()
    );
    let transport = Arc::new(FakeUpdateTransport::default());
    transport.route(MANIFEST_URL, FakeRoute::ok(envelope(&manifest_text)));
    transport.route("https://updates.example/9.9.9/MightyCode-windows-x64.zip", FakeRoute::ok(package));
    let service = service(&transport, &state, Some(fixture_key()));
    let (launched, launch) = launches();
    let coordinator = Coordinator::new(service.clone(), Options { launch_helper: Some(launch), ..options("x64", Some(&install)) });

    coordinator.press(None).await;
    assert_eq!(
        coordinator.state().phase,
        Phase::Available,
        "the check must offer the new version: {:?}",
        coordinator.state().error_message
    );
    coordinator.press(None).await;
    assert_eq!(coordinator.state().phase, Phase::Ready, "the download must end ready: {:?}", coordinator.state().error_message);
    assert!(coordinator.state().install_plan.is_some(), "a ready update must carry an install plan");
    assert!(coordinator.state().staged_directory.unwrap().join("MightyCode.exe").is_file(), "the package must be staged");

    coordinator.press(None).await;
    assert_eq!(coordinator.state().phase, Phase::Installing, "the install must hand over to the helper");
    let plan = {
        let launched = launched.lock().unwrap();
        assert!(launched.len() == 1 && launched[0].install_directory == install, "the helper must be given the install folder");
        launched[0].clone()
    };

    // The helper re-verifies and replaces, using the plan the app built.
    let result = replacement::run(&plan, &Fake::new(true, ok), &ko());
    assert!(result.replaced, "the helper must replace the install: {:?}", result.error);
    assert!(install.join("MightyCode.exe").is_file(), "the new app must be in place");

    // A manifest with no asset for this machine is refused.
    let arm_only = Coordinator::new(service, options("arm64", Some(&install)));
    arm_only.press(None).await;
    arm_only.press(None).await;
    assert_eq!(arm_only.state().phase, Phase::Failed, "a manifest without this architecture must fail");
    assert!(arm_only.state().error_message.unwrap_or_default().contains("arm64"), "the refusal must name the architecture");
}

/// Verification.cs "app update keeps the setup program's uninstall entry at the app's version" (the decision; B writes it).
#[test]
fn app_update_keeps_the_setup_programs_uninstall_entry_at_the_apps_version() {
    let install = r"C:\Users\me\AppData\Local\Programs\MightyCode";
    let write = |location: Option<&str>, display: &str, app: &str| {
        installer_entry::version_to_write(location, Some(display), Path::new(app), "0.4.35")
    };
    assert_eq!(
        write(Some(&format!(r"{install}\")), "0.4.34", install).as_deref(),
        Some("0.4.35"),
        "an entry for this folder with an older version takes the app's version"
    );
    assert_eq!(
        write(Some(&install.to_uppercase()), "0.4.34", &format!(r"{install}\")).as_deref(),
        Some("0.4.35"),
        "folders compare without case and trailing separator"
    );
    assert_eq!(write(Some(&format!(r"{install}\")), "0.4.35", install), None, "the same version writes nothing");
    assert_eq!(
        write(Some(&format!(r"{install}\")), "0.4.34", r"D:\Tools\MightyCode"),
        None,
        "a copy unpacked elsewhere leaves the entry alone"
    );
    assert_eq!(write(None, "0.4.34", install), None, "no entry writes nothing");
    assert_eq!(
        write(Some(&format!("{install}Old")), "0.4.34", install),
        None,
        "a folder whose name only starts the same is another folder"
    );

    // The setup script and the app name the same entry, and setup installs where the app can replace itself.
    let script = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../windows/installer/MightyCode.iss"))
        .expect("the setup script");
    let key = installer_entry::UNINSTALL_KEY;
    let guid = &key[key.rfind('{').unwrap() + 1..key.rfind('}').unwrap()];
    assert!(script.contains(&format!("AppId={{{{{guid}}}")), "MightyCode.iss AppId must match UNINSTALL_KEY");
    assert!(
        script.contains("PrivilegesRequired=lowest") && script.contains("DisableDirPage=yes"),
        "setup installs for this user only, into its fixed folder"
    );
    assert_eq!(installer_entry::display_version("0", "5", "0"), "0.5.0", "the helper's version is major.minor.patch");
}

// — Fully automatic update (M/AppUpdateAuto.swift, M/AppUpdateAutoSteps.swift) —

/// Verification.cs "app update automatic policy follows the macOS rules".
#[test]
fn app_update_automatic_policy_follows_the_macos_rules() {
    let secs = Duration::from_secs;
    assert!(
        auto::CHECK_INTERVAL == secs(6 * 3600)
            && auto::TICK == secs(600)
            && auto::USER_IDLE_DELAY == secs(300)
            && auto::AGENT_IDLE_DELAY == secs(180)
            && auto::COUNTDOWN == secs(60)
            && auto::COUNTDOWN_TICK == secs(1)
            && auto::COUNTDOWN_GRACE == secs(5)
            && auto::SNOOZE == secs(4 * 3600),
        "the constants must be the macOS ones"
    );

    // The macOS source names the same blockers in the same order and the same constants.
    let swift =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../macos/Sources/MightyCore/AppUpdateAuto.swift"))
            .expect("AppUpdateAuto.swift")
            .replace("\r\n", "\n");
    let cases = IdleBlocker::ALL
        .iter()
        .map(|blocker| {
            let name = format!("{blocker:?}");
            format!("{}{}", name[..1].to_lowercase(), &name[1..])
        })
        .collect::<Vec<_>>()
        .join(", ");
    assert!(swift.contains(&format!("case {cases}\n")), "AppIdleBlocker must list the macOS cases in the macOS order: {cases}");
    for constant in [
        "checkInterval: TimeInterval = 6 * 60 * 60",
        "tick: TimeInterval = 10 * 60",
        "userIdleDelay: TimeInterval = 5 * 60",
        "agentIdleDelay: TimeInterval = 3 * 60",
        "countdown: TimeInterval = 60",
        "countdownTick: TimeInterval = 1",
        "countdownGrace: TimeInterval = 5",
        "snooze: TimeInterval = 4 * 60 * 60",
    ] {
        assert!(swift.contains(constant), "the macOS constant changed: {constant}");
    }

    let now = Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap();
    let (hours, minutes, seconds, days) =
        (chrono::Duration::hours, chrono::Duration::minutes, chrono::Duration::seconds, chrono::Duration::days);
    assert!(auto::check_due(true, true, None, None, now), "a first run is due");
    assert!(auto::check_due(true, true, Some(now - hours(6)), None, now), "six hours after the last check is due");
    assert!(!auto::check_due(true, true, Some(now - hours(6) + seconds(1)), None, now), "inside six hours is not due");
    assert!(!auto::check_due(true, true, Some(now - days(2)), Some(now - hours(1)), now), "a failed attempt waits for the next cycle");
    assert!(auto::check_due(true, true, Some(now - days(2)), Some(now - hours(6)), now), "the next cycle after a failed attempt is due");
    assert!(!auto::check_due(false, true, None, None, now), "the switch off means never");
    assert!(!auto::check_due(true, false, None, None, now), "no key means never");
    assert!(auto::check_due(true, true, Some(now + days(3)), None, now), "a check dated in the future (clock set back) is ignored");
    assert!(!auto::check_due(true, true, Some(now + days(3)), Some(now - hours(1)), now), "a past attempt still holds the next try back");

    assert!(
        auto::idle_blockers(&IdleFacts::default(), now).is_empty() && auto::is_idle(&IdleFacts::default(), now),
        "nothing going on is idle"
    );
    let alone: [(IdleFacts, IdleBlocker); 14] = [
        (IdleFacts { agent_runs: 1, ..IdleFacts::default() }, IdleBlocker::AgentRun),
        (IdleFacts { pending_cards: 1, ..IdleFacts::default() }, IdleBlocker::PendingCard),
        (IdleFacts { queued_inputs: 1, ..IdleFacts::default() }, IdleBlocker::QueuedInput),
        (IdleFacts { drafts: 1, ..IdleFacts::default() }, IdleBlocker::Draft),
        (IdleFacts { terminals: 1, ..IdleFacts::default() }, IdleBlocker::Terminal),
        (IdleFacts { cli_work: true, ..IdleFacts::default() }, IdleBlocker::CliWork),
        (IdleFacts { screen_sharing: true, ..IdleFacts::default() }, IdleBlocker::Remote),
        (IdleFacts { phone_uploads: 1, ..IdleFacts::default() }, IdleBlocker::Remote),
        (IdleFacts { remote_host_runs: 1, ..IdleFacts::default() }, IdleBlocker::Remote),
        (IdleFacts { sharing_workspaces: true, ..IdleFacts::default() }, IdleBlocker::Remote),
        (IdleFacts { blocking_modal: true, ..IdleFacts::default() }, IdleBlocker::Modal),
        (IdleFacts { last_user_input: Some(now - minutes(4)), ..IdleFacts::default() }, IdleBlocker::UserActive),
        (IdleFacts { last_user_input: Some(now + minutes(1)), ..IdleFacts::default() }, IdleBlocker::UserActive),
        (IdleFacts { last_agent_activity: Some(now - minutes(2)), ..IdleFacts::default() }, IdleBlocker::RecentAgentActivity),
    ];
    for (facts, blocker) in alone {
        assert_eq!(auto::idle_blockers(&facts, now), [blocker], "{blocker:?} must block on its own");
    }
    assert!(
        auto::is_idle(
            &IdleFacts { last_user_input: Some(now - minutes(5)), last_agent_activity: Some(now - minutes(3)), ..IdleFacts::default() },
            now
        ),
        "input and runs past their delays are idle"
    );
    assert_eq!(
        auto::idle_blockers(&IdleFacts { agent_runs: 1, drafts: 2, last_user_input: Some(now), ..IdleFacts::default() }, now),
        [IdleBlocker::AgentRun, IdleBlocker::Draft, IdleBlocker::UserActive],
        "the blockers come in the macOS order"
    );

    // The countdown: starts when idle, survives input, cancels on anything else, restarts when it runs out.
    let idle = RestartState::Idle;
    let decision = |state, action| RestartDecision { state, action };
    assert_eq!(auto::restart_offer(idle, false, true, &[], now), decision(idle, RestartAction::None), "nothing staged offers nothing");
    assert_eq!(auto::restart_offer(idle, true, false, &[], now).action, RestartAction::None, "the switch off offers nothing");
    assert_eq!(
        auto::restart_offer(idle, true, true, &[IdleBlocker::UserActive], now).action,
        RestartAction::None,
        "a busy app is not offered a restart"
    );
    let started = auto::restart_offer(idle, true, true, &[], now);
    assert_eq!(
        started,
        decision(RestartState::CountingDown(now + seconds(60)), RestartAction::StartCountdown),
        "an idle app starts the countdown"
    );
    assert_eq!(
        auto::restart_offer(started.state, true, true, &[IdleBlocker::UserActive], now + seconds(1)),
        decision(started.state, RestartAction::None),
        "input alone does not cancel a running countdown"
    );
    assert_eq!(
        auto::restart_offer(started.state, true, true, &[IdleBlocker::UserActive, IdleBlocker::Draft], now + seconds(2)),
        decision(idle, RestartAction::Cancel),
        "any other blocker cancels quietly"
    );
    assert_eq!(
        auto::restart_offer(started.state, true, false, &[], now + seconds(2)).action,
        RestartAction::Cancel,
        "the switch going off cancels"
    );
    assert_eq!(
        auto::restart_offer(started.state, false, true, &[], now + seconds(2)).action,
        RestartAction::Cancel,
        "an update no longer ready cancels"
    );
    assert_eq!(
        auto::restart_offer(started.state, true, true, &[], now + seconds(59)).action,
        RestartAction::None,
        "the countdown runs its sixty seconds"
    );
    assert_eq!(
        auto::restart_offer(started.state, true, true, &[], now + seconds(60)),
        decision(idle, RestartAction::Restart),
        "the end of the countdown restarts"
    );
    assert_eq!(
        auto::restart_offer(started.state, true, true, &[], now + seconds(65)).action,
        RestartAction::Restart,
        "a tick within the grace still restarts"
    );
    assert_eq!(
        auto::restart_offer(started.state, true, true, &[], now + seconds(66)),
        decision(idle, RestartAction::Cancel),
        "a countdown found past its grace (the computer slept) is cancelled, never acted on"
    );

    let later = auto::later(now);
    assert_eq!(later, RestartState::Snoozed(now + hours(4)), "later snoozes four hours");
    assert_eq!(
        auto::restart_offer(later, true, true, &[], now + hours(4) - seconds(1)),
        decision(later, RestartAction::None),
        "a snooze holds until it ends"
    );
    assert_eq!(
        auto::restart_offer(later, true, true, &[], now + hours(4)).action,
        RestartAction::StartCountdown,
        "after the snooze an idle app is asked again"
    );

    assert!(auto::installs_on_quit(true, true, true, false), "a staged update installs on quit");
    assert!(
        !auto::installs_on_quit(false, true, true, false)
            && !auto::installs_on_quit(true, false, true, false)
            && !auto::installs_on_quit(true, true, false, false)
            && !auto::installs_on_quit(true, true, true, true),
        "nothing staged, the switch off, no key or a shutdown installs nothing on quit"
    );
}

/// Verification.cs "app update helper --relaunch round trips".
#[test]
fn app_update_helper_relaunch_round_trips() {
    let temp = std::env::temp_dir();
    let plan = create_plan(&temp.join("staged"), &temp.join("MightyCode"), &temp.join("package.zip"), &"c".repeat(64), 10, 4242);
    assert!(plan.relaunch, "a plan relaunches by default");
    let quiet = InstallPlan { relaunch: false, ..plan.clone() };
    let arguments = quiet.helper_arguments();
    assert!(
        arguments.len() >= 2 && arguments[arguments.len() - 2] == RELAUNCH_FLAG && arguments[arguments.len() - 1] == "0",
        "a plan without relaunch says --relaunch 0"
    );
    assert_eq!(plan.helper_arguments().last().unwrap(), "1", "a plan with relaunch says --relaunch 1");
    assert_eq!(plan::try_parse(&arguments), Some(quiet), "--relaunch 0 must round trip");
    assert_eq!(plan::try_parse(&plan.helper_arguments()), Some(plan.clone()), "--relaunch 1 must round trip");
    let all = plan.helper_arguments();
    assert_eq!(
        plan::try_parse(&all[..all.len() - 2]),
        Some(plan.clone()),
        "an argument list from an app before --relaunch existed must relaunch"
    );
    let mut yes = all[..all.len() - 1].to_vec();
    yes.push("yes".into());
    assert_eq!(plan::try_parse(&yes), None, "a --relaunch value other than 1 or 0 must not produce a plan");
}

/// Verification.cs "app update replacement without relaunch never starts the app".
#[test]
fn app_update_replacement_without_relaunch_never_starts_the_app() {
    let root = Temp::new();
    let (install, package, plan) = swap_fixture(root.path(), false);
    let started = RefCell::new(Vec::new());
    let start = |path: &Path| {
        started.borrow_mut().push(path.to_path_buf());
        Ok(())
    };
    let result = replacement::run(&plan, &Fake::new(true, start), &ko());
    assert!(result.replaced && !result.rolled_back, "the install on quit must replace the install: {:?}", result.error);
    assert_eq!(read(install.join("MightyCode.exe")), "new", "the new app must be in place");
    assert!(started.borrow().is_empty() && !result.started, "the install on quit must not start the app");
    assert!(result.backup_removed && !plan.backup_directory.exists(), "the backup must be removed after a swap without relaunch");

    // A failed copy leaves the install as it was and starts nothing either.
    make_install(&plan.staged_directory, "newer");
    let failing =
        InstallPlan { relaunch: false, ..create_plan(&plan.staged_directory, &install, &package, &sha256_of(b"package"), 7, 4242) };
    let rolled = replacement::run(
        &failing,
        &Fake::new(true, start).copier(|_, destination| {
            std::fs::create_dir_all(destination)?;
            Err(io::Error::other("복사 실패"))
        }),
        &ko(),
    );
    assert!(
        !rolled.replaced && !rolled.rolled_back && rolled.moves.is_empty(),
        "a failed copy must stop before anything is moved: {:?}",
        rolled.error
    );
    assert_eq!(read(install.join("MightyCode.exe")), "new", "the install must be the one before the swap");
    assert!(started.borrow().is_empty() && !rolled.started, "a failed swap without relaunch must not start the app");
}

/// Verification.cs "app update a staged update stays ready and installs on quit".
#[tokio::test]
async fn app_update_a_staged_update_stays_ready_and_installs_on_quit() {
    let root = Temp::new();
    let install = root.path().join("MightyCode");
    make_install(&install, "old");
    let package_path = make_zip(root.path(), "MightyCode-windows-x64.zip", &[Entry::File("MightyCode.exe", &fake_executable("x64"))]);
    let package = std::fs::read(&package_path).unwrap();
    let transport = Arc::new(FakeUpdateTransport::default());
    transport.route(MANIFEST_URL, FakeRoute::ok(envelope(&windows_manifest("9.9.9", &package, &["x64"]))));
    transport.route("https://updates.example/9.9.9/MightyCode-windows-x64.zip", FakeRoute::ok(package.clone()));
    let service = service(&transport, &root.path().join("state"), Some(fixture_key()));
    let (launched, launch) = launches();
    let automatic = Arc::new(Mutex::new(true));
    let coordinator = Coordinator::new(
        service.clone(),
        Options { launch_helper: Some(launch), automatic: shared_flag(&automatic), ..options("x64", Some(&install)) },
    );

    assert!(coordinator.check_automatically_if_due(None, true, None, Utc::now()).await, "the first automatic check runs");
    assert_eq!(
        coordinator.state().phase,
        Phase::Ready,
        "with the switch on a found version is downloaded and staged: {:?}",
        coordinator.state().error_message
    );
    let first_downloads = downloads(&transport);
    assert_eq!(first_downloads, 1, "one package download");
    let t = |field: &str| text(&ko(), field);
    let view = coordinator.describe(&ko(), |_| "00:00".into());
    assert!(
        view.status_text == t("StagedTemplate").replace("{version}", "9.9.9") && view.button_label == t("RestartNowButton"),
        "a staged update says it installs at the next launch and offers restart now"
    );

    coordinator.check(None).await;
    assert_eq!(coordinator.state().phase, Phase::Ready, "the same version keeps the staged update ready");
    assert_eq!(downloads(&transport), first_downloads, "the same version is not downloaded twice");

    transport.route(MANIFEST_URL, FakeRoute { status: 500, location: None, body: Vec::new() });
    coordinator.check(None).await;
    assert!(
        coordinator.state().phase == Phase::Ready && coordinator.state().install_plan.is_some(),
        "a failed check never drops a staged update"
    );
    let last_error = coordinator.state().last_error;
    assert!(last_error.as_deref().is_some_and(|error| !error.is_empty()), "the failure is kept in LastError");
    let view = coordinator.describe(&ko(), |_| "00:00".into());
    assert!(
        view.kept_error == last_error && view.retry_notice == Some(t("RetryNotice")),
        "the section shows the failure and the retry notice"
    );
    assert_eq!(view.status_text, t("StagedTemplate").replace("{version}", "9.9.9"), "the status still says the update is staged");
    transport.route(MANIFEST_URL, FakeRoute::ok(envelope(&windows_manifest("9.9.9", &package, &["x64"]))));

    *automatic.lock().unwrap() = false;
    assert!(!coordinator.install_on_quit(false), "with the switch off quitting installs nothing");
    assert!(coordinator.state().phase == Phase::Ready && launched.lock().unwrap().is_empty(), "the staged update waits for restart now");
    *automatic.lock().unwrap() = true;
    assert!(!coordinator.install_on_quit(true), "a shutdown leaves the update for the next quit");
    assert!(coordinator.install_on_quit(false), "quitting installs the staged update");
    assert_eq!(coordinator.state().phase, Phase::Installing, "the install hands over to the helper");
    {
        let launched = launched.lock().unwrap();
        assert!(
            launched.len() == 1 && !launched[0].relaunch && launched[0].helper_arguments().last().unwrap() == "0",
            "the helper is started with --relaunch 0"
        );
    }
    assert!(!coordinator.install_on_quit(false), "an install under way is not started twice");

    // A failed check with nothing staged is the section's state, with the retry notice while the switch is on.
    let fresh = Coordinator::new(service, Options { automatic: flag(true), ..options("x64", Some(&install)) });
    transport.route(MANIFEST_URL, FakeRoute { status: 500, location: None, body: Vec::new() });
    fresh.check(None).await;
    assert_eq!(fresh.state().phase, Phase::Failed, "a failed check with nothing staged fails");
    let view = fresh.describe(&ko(), |_| "00:00".into());
    assert!(
        view.retry_notice == Some(t("RetryNotice")) && view.kept_error.is_none(),
        "the retry notice shows, the error is the status itself"
    );
}

/// C#'s `WindowsManifest(version, package, architectures)`.
fn windows_manifest(version: &str, package: &[u8], architectures: &[&str]) -> String {
    let assets: Vec<String> = architectures
        .iter()
        .map(|architecture| {
            format!(
                r#""{architecture}":{{"url":"https://updates.example/{version}/MightyCode-windows-{architecture}.zip","sha256":"{}","size":{}}}"#,
                sha256_of(package),
                package.len()
            )
        })
        .collect();
    format!(r#"{{"version":"{version}","windows":{{{}}}}}"#, assets.join(","))
}

/// Verification.cs "app update helper copies beside the install and swaps by renaming".
#[test]
fn app_update_helper_copies_beside_the_install_and_swaps_by_renaming() {
    let root = Temp::new();
    let (install, _, plan) = swap_fixture(root.path(), true);
    let expected_new =
        mighty_updates::app_update::dotnet::with_suffix(&mighty_updates::app_update::dotnet::full_path(&install).unwrap(), ".update-new");
    assert_eq!(plan.new_directory(), expected_new, "the copy lands beside the install");
    let whole_during_copy = RefCell::new(false);
    let result = replacement::run(
        &plan,
        &Fake::new(true, ok).copier(|source, destination| {
            *whole_during_copy.borrow_mut() = destination == plan.new_directory() && read(install.join("MightyCode.exe")) == "old";
            replacement::copy_directory(source, destination)
        }),
        &ko(),
    );
    assert!(result.replaced && *whole_during_copy.borrow(), "the install is whole while the copy runs: {:?}", result.error);
    assert_eq!(
        result.moves.iter().map(|m| (m.kind, m.from.clone())).collect::<Vec<_>>(),
        [(MoveKind::Backup, plan.install_directory.clone()), (MoveKind::Install, plan.new_directory())],
        "the swap is two renames, the install aside and the copy in"
    );
    assert!(!plan.new_directory().exists() && plan.staged_directory.is_dir(), "the copy was renamed in; the staged folder stays");

    // An install on quit that finds the app running again keeps everything for the next quit.
    let again = root.path().join("again");
    let (again_install, _, quiet) = swap_fixture(&again, false);
    let started_again = RefCell::new(0);
    let asked = RefCell::new(false);
    let skipped = replacement::run(
        &quiet,
        &Fake::new(true, |_| {
            *started_again.borrow_mut() += 1;
            Ok(())
        })
        .running_again(|| {
            *asked.borrow_mut() = true;
            Ok(true)
        }),
        &ko(),
    );
    assert!(
        *asked.borrow() && skipped.skipped && !skipped.replaced && !skipped.rolled_back && skipped.error.is_none(),
        "a running app skips the swap without an error"
    );
    assert!(skipped.moves.is_empty() && *started_again.borrow() == 0 && !skipped.started, "nothing is moved or started");
    assert_eq!(read(again_install.join("MightyCode.exe")), "old", "the install is untouched");
    assert!(!quiet.new_directory().exists() && quiet.staged_directory.is_dir(), "the copy is removed and the staged update kept");
    assert!(!quiet.backup_directory.exists(), "no backup is taken");
    let not_asked = RefCell::new(true);
    let relaunching = replacement::run(
        &InstallPlan { relaunch: true, ..quiet.clone() },
        &Fake::new(true, ok).running_again(|| {
            *not_asked.borrow_mut() = false;
            Ok(true)
        }),
        &ko(),
    );
    assert!(*not_asked.borrow() && relaunching.replaced, "a restart asked for by the user does not look for the app running again");

    // The copy cannot be renamed in: the old install is put back (and started only with a relaunch).
    for relaunch in [true, false] {
        let failing = root.path().join(format!("failing-{relaunch}"));
        let (failing_install, _, failing_plan) = swap_fixture(&failing, relaunch);
        let starts = RefCell::new(Vec::new());
        let rolled = replacement::run(
            &failing_plan,
            &Fake::new(true, |path| {
                starts.borrow_mut().push(path.to_path_buf());
                Ok(())
            })
            .mover(|from, to| {
                if from == failing_plan.new_directory() {
                    return Err(io::Error::other("이름을 바꾸지 못했습니다."));
                }
                housekeeping::move_directory(from, to)
            }),
            &ko(),
        );
        assert!(!rolled.replaced && rolled.rolled_back, "a failed rename must roll back: {:?}", rolled.error);
        assert_eq!(read(failing_install.join("MightyCode.exe")), "old", "the old install is back");
        assert!(
            !failing_plan.backup_directory.exists() && !failing_plan.new_directory().exists(),
            "neither the backup nor the copy is left behind"
        );
        assert!(
            starts.borrow().len() == usize::from(relaunch) && rolled.started == relaunch,
            "the old app is started again only with a relaunch"
        );
    }

    // The install cannot be moved aside (the app runs from it): nothing changes and the app the user had comes back.
    let locked = root.path().join("locked");
    let (locked_install, _, locked_plan) = swap_fixture(&locked, true);
    let locked_starts = RefCell::new(0);
    let refused = replacement::run(
        &locked_plan,
        &Fake::new(true, |_| {
            *locked_starts.borrow_mut() += 1;
            Ok(())
        })
        .mover(|_, _| Err(io::Error::other("다른 프로세스가 사용 중입니다."))),
        &ko(),
    );
    assert!(!refused.replaced && !refused.rolled_back && refused.moves.is_empty(), "a refused first rename moves nothing");
    assert!(
        read(locked_install.join("MightyCode.exe")) == "old" && !locked_plan.new_directory().exists(),
        "the install stays and the copy is removed"
    );
    assert_eq!(*locked_starts.borrow(), 1, "the app the user had is started again");
}

/// Verification.cs "app update helper retries a rename and reads an unsure running-again check as running".
#[test]
fn app_update_helper_retries_a_rename_and_reads_an_unsure_running_again_check_as_running() {
    let root = Temp::new();
    // The install is refused three times, then moves; the copy goes in after it.
    let (install, _, plan) = swap_fixture(&root.path().join("busy"), true);
    let tries = RefCell::new(Vec::new());
    let refusals = RefCell::new(0);
    let result = replacement::run(
        &plan,
        &Fake::new(true, ok).mover(|from, to| {
            tries.borrow_mut().push(from.to_path_buf());
            *refusals.borrow_mut() += 1;
            if *refusals.borrow() <= 3 {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "잠시 사용 중입니다."));
            }
            housekeeping::move_directory(from, to)
        }),
        &ko(),
    );
    assert!(
        result.replaced && !result.rolled_back && read(install.join("MightyCode.exe")) == "new",
        "a rename refused for a moment goes through: {:?}",
        result.error
    );
    let tries = tries.borrow();
    assert!(
        tries.len() == 5 && tries[..4].iter().all(|from| *from == plan.install_directory) && tries[4] == plan.new_directory(),
        "the install is tried four times, then the copy is renamed in; got {tries:?}"
    );

    // The copy is refused three times, then goes in.
    let (new_install, _, new_plan) = swap_fixture(&root.path().join("busy-new"), false);
    let new_refusals = RefCell::new(0);
    let second = replacement::run(
        &new_plan,
        &Fake::new(true, ok).mover(|from, to| {
            if from == new_plan.new_directory() {
                *new_refusals.borrow_mut() += 1;
                if *new_refusals.borrow() <= 3 {
                    return Err(io::Error::other("잠시 사용 중입니다."));
                }
            }
            housekeeping::move_directory(from, to)
        }),
        &ko(),
    );
    assert!(
        second.replaced && *new_refusals.borrow() == 4 && read(new_install.join("MightyCode.exe")) == "new",
        "the copy's rename is tried again too: {:?}",
        second.error
    );

    // Refused every time, the copy is given up after MOVE_ATTEMPTS tries, and the rollback's own rename is tried again.
    let (stuck_install, _, stuck_plan) = swap_fixture(&root.path().join("stuck"), false);
    let new_tries = RefCell::new(0u32);
    let rollback_refusals = RefCell::new(0);
    let stuck = replacement::run(
        &stuck_plan,
        &Fake::new(true, ok).mover(|from, to| {
            if from == stuck_plan.new_directory() {
                *new_tries.borrow_mut() += 1;
                return Err(io::Error::other("다른 프로세스가 사용 중입니다."));
            }
            if from == stuck_plan.backup_directory {
                *rollback_refusals.borrow_mut() += 1;
                if *rollback_refusals.borrow() <= 2 {
                    return Err(io::Error::other("다른 프로세스가 사용 중입니다."));
                }
            }
            housekeeping::move_directory(from, to)
        }),
        &ko(),
    );
    assert!(
        !stuck.replaced && stuck.rolled_back && *new_tries.borrow() == MOVE_ATTEMPTS && *rollback_refusals.borrow() == 3,
        "the copy is tried {MOVE_ATTEMPTS} times and the rollback goes through on its third try; got {} and {}: {:?}",
        new_tries.borrow(),
        rollback_refusals.borrow(),
        stuck.error
    );
    assert!(
        read(stuck_install.join("MightyCode.exe")) == "old"
            && !stuck_plan.backup_directory.exists()
            && !stuck_plan.new_directory().exists(),
        "the old install is back and nothing is left beside it"
    );

    // A copy that is not the staged tree (a file missing, or one of another size) is never swapped in: the install stays, the copy is
    // removed, the staged update kept, and the reason goes to install.log.
    let spoilers: [(&str, Spoil); 2] = [
        ("missing", |copy| std::fs::remove_file(copy.join("lib.dll")).unwrap()),
        ("short", |copy| std::fs::write(copy.join("lib.dll"), "x").unwrap()),
    ];
    for (name, spoil) in spoilers {
        let (copy_install, _, copy_plan) = swap_fixture(&root.path().join(format!("copy-{name}")), true);
        std::fs::write(copy_plan.staged_directory.join("lib.dll"), "library").unwrap();
        let copy_starts = RefCell::new(0);
        let spoiled = replacement::run(
            &copy_plan,
            &Fake::new(true, |_| {
                *copy_starts.borrow_mut() += 1;
                Ok(())
            })
            .copier(|source, destination| {
                replacement::copy_directory(source, destination)?;
                spoil(destination);
                Ok(())
            }),
            &ko(),
        );
        assert!(
            !spoiled.replaced
                && !spoiled.rolled_back
                && spoiled.moves.is_empty()
                && spoiled.error.as_deref().is_some_and(|e| e.contains("does not match the staged update")),
            "a {name} copy is refused before the renames; got replaced {}, error {:?}",
            spoiled.replaced,
            spoiled.error
        );
        assert!(
            read(copy_install.join("MightyCode.exe")) == "old"
                && !copy_plan.new_directory().exists()
                && copy_plan.staged_directory.join("lib.dll").is_file()
                && *copy_starts.borrow() == 1
                && spoiled.started,
            "the install stays, the copy is removed, the staged update kept, and the app the user had starts again"
        );
    }
    assert_eq!(housekeeping::tree_mismatch(&plan.staged_directory, &plan.staged_directory).unwrap(), None, "a tree matches itself");

    // One helper per install: the mutex name follows the folder, however it is written, and a helper that cannot hold it leaves at once,
    // skipped, with nothing moved or started and the reason for install.log.
    let name = guard::mutex_name(&install);
    let mut with_separator = install.clone().into_os_string();
    with_separator.push(std::path::MAIN_SEPARATOR.to_string());
    let mut other = install.clone().into_os_string();
    other.push("-other");
    assert!(
        name.starts_with(guard::MUTEX_PREFIX)
            && name.len() == guard::MUTEX_PREFIX.len() + 32
            && name == guard::mutex_name(Path::new(&with_separator))
            && name == guard::mutex_name(Path::new(&install.to_string_lossy().to_uppercase()))
            && name != guard::mutex_name(Path::new(&other)),
        "the helper's mutex is named for its install folder: {name}"
    );
    assert_eq!(guard::refusal(true), None, "the helper holding the mutex goes on");
    let refused = guard::refusal(false).expect("a refusal");
    assert!(
        refused.skipped
            && !refused.replaced
            && !refused.rolled_back
            && !refused.started
            && refused.moves.is_empty()
            && refused.error.as_deref().is_some_and(|why| why.contains("another update helper")),
        "a second helper leaves at once, skipped, moving and starting nothing"
    );

    // A check for the app running again that fails, at once or after a moment, keeps everything and says why.
    let (unsure_install, _, unsure_plan) = swap_fixture(&root.path().join("unsure"), false);
    let starts = RefCell::new(0);
    let checks: [fn() -> io::Result<bool>; 2] = [
        || Err(io::Error::other("프로세스 목록을 읽지 못했습니다.")),
        || {
            std::thread::sleep(Duration::from_millis(1));
            Err(io::Error::other("프로세스 목록을 읽지 못했습니다."))
        },
    ];
    for check in checks {
        let unsure = replacement::run(
            &unsure_plan,
            &Fake::new(true, |_| {
                *starts.borrow_mut() += 1;
                Ok(())
            })
            .running_again(check),
            &ko(),
        );
        assert!(
            unsure.skipped
                && !unsure.replaced
                && !unsure.rolled_back
                && unsure.moves.is_empty()
                && unsure.error.as_deref() == Some("프로세스 목록을 읽지 못했습니다.")
                && *starts.borrow() == 0,
            "a check that cannot tell reads as the app running, with its reason; got skipped {}, error {:?}",
            unsure.skipped,
            unsure.error
        );
        assert!(
            read(unsure_install.join("MightyCode.exe")) == "old"
                && !unsure_plan.new_directory().exists()
                && unsure_plan.staged_directory.is_dir(),
            "the install is untouched, the copy removed and the staged update kept"
        );
    }
}

/// Verification.cs "app update a kept update is staged again without a download".
#[tokio::test]
async fn app_update_a_kept_update_is_staged_again_without_a_download() {
    let root = Temp::new();
    let state = root.path().join("state");
    let install = root.path().join("MightyCode");
    make_install(&install, "old");
    let package_path = make_zip(root.path(), "MightyCode-windows-x64.zip", &[Entry::File("MightyCode.exe", &fake_executable("x64"))]);
    let package = std::fs::read(&package_path).unwrap();
    let transport = Arc::new(FakeUpdateTransport::default());
    transport.route(MANIFEST_URL, FakeRoute::ok(envelope(&windows_manifest("9.9.9", &package, &["x64"]))));
    transport.route("https://updates.example/9.9.9/MightyCode-windows-x64.zip", FakeRoute::ok(package.clone()));
    let service = service(&transport, &state, Some(fixture_key()));
    let with = |automatic: bool, busy: Arc<dyn Fn() -> bool + Send + Sync>| Options {
        launch_helper: Some(Arc::new(|_: &InstallPlan| Ok(()))),
        automatic: flag(automatic),
        staging_busy: Some(busy),
        ..options("x64", Some(&install))
    };

    // The run before staged it; its install on quit found the app open again and kept updates/ as it was.
    let before = Coordinator::new(service.clone(), with(true, flag(false)));
    assert_eq!(before.check_kept_update(None, Utc::now()).await, KeptCheck::None, "with nothing kept the start checks nothing");
    before.check(None).await;
    assert_eq!(before.state().phase, Phase::Ready, "the run before ends ready: {:?}", before.state().error_message);
    assert!(
        service.kept_version("0.1.0").as_deref() == Some("9.9.9")
            && service.kept_version("9.9.9").is_none()
            && service.kept_version("10.0.0").is_none(),
        "the kept version is found only by a version older than it"
    );
    // The helper's staged folder, as it runs from it.
    let helper_file = before.state().staged_directory.unwrap().join("helper-running.txt");
    std::fs::write(&helper_file, "in use").unwrap();

    let off = Coordinator::new(service.clone(), with(false, flag(false)));
    assert!(
        off.check_kept_update(None, Utc::now()).await == KeptCheck::None && off.state().phase == Phase::Idle,
        "with the switch off the start checks nothing"
    );

    // While the helper still runs from the staged folder (or its copy is fresh) the kept update waits, tick after tick: nothing is
    // fetched, unpacked or counted as an attempt, and nothing fails.
    let busy = Arc::new(Mutex::new(true));
    let requests = transport.asked().len();
    let next = Coordinator::new(service.clone(), with(true, shared_flag(&busy)));
    for _ in 0..2 {
        assert!(
            next.check_kept_update(None, Utc::now()).await == KeptCheck::Waiting
                && next.state().phase == Phase::Idle
                && next.last_attempt().is_none()
                && next.state().last_error.is_none()
                && transport.asked().len() == requests
                && helper_file.is_file(),
            "a kept update waits while staging is busy, with nothing fetched, failed or touched"
        );
    }

    // Once the helper is gone the next tick checks it and stages it again from the package on disk.
    *busy.lock().unwrap() = false;
    let now = Utc::now();
    assert!(
        next.check_kept_update(None, now).await == KeptCheck::Checked
            && next.state().phase == Phase::Ready
            && next.state().install_plan.is_some()
            && next.last_attempt() == Some(now),
        "the kept update is checked and ready again: {:?}",
        next.state().error_message
    );
    assert_eq!(
        transport.asked()[requests..],
        [MANIFEST_URL.to_string()],
        "only the manifest is fetched; the package on disk is used again"
    );
    assert!(
        next.state().staged_directory.unwrap().join("MightyCode.exe").is_file() && !helper_file.exists(),
        "the package is staged afresh"
    );

    // Any download waits the same way: before it starts nothing under updates/ is touched, and a download that finds staging busy once
    // it is done leaves the update Available, with no error, until a later tick stages it.
    std::fs::write(&helper_file, "in use").unwrap();
    let answers = Arc::new(Mutex::new(VecDeque::from([true])));
    let queue = answers.clone();
    let later = Coordinator::new(service.clone(), with(true, Arc::new(move || queue.lock().unwrap().pop_front().unwrap_or(false))));
    later.check(None).await;
    assert!(
        later.state().phase == Phase::Available
            && later.staging_deferred()
            && later.state().last_error.is_none()
            && later.state().error_message.is_none()
            && helper_file.is_file(),
        "a download while staging is busy waits without touching updates/; got {:?}, deferred {}",
        later.state().phase,
        later.staging_deferred()
    );
    *answers.lock().unwrap() = VecDeque::from([false, true]);
    later.retry_deferred_staging().await;
    assert!(
        later.state().phase == Phase::Available && later.staging_deferred() && later.state().last_error.is_none() && helper_file.is_file(),
        "busy again before unpacking, it stays Available and the staged folder is not touched; got {:?}",
        later.state().phase
    );
    let requests = transport.asked().len();
    later.retry_deferred_staging().await;
    assert!(
        later.state().phase == Phase::Ready && !later.staging_deferred() && transport.asked().len() == requests && !helper_file.exists(),
        "a later tick stages it from the package on disk: {:?}",
        later.state().error_message
    );

    // A deferral belongs to the Available it left: a check that fails meanwhile clears it, so the loop's due check runs again instead of
    // waiting on a retry that has nothing to do.
    let flaky_transport = Arc::new(FakeUpdateTransport::default());
    flaky_transport.route(MANIFEST_URL, FakeRoute::ok(envelope(&windows_manifest("9.9.9", &package, &["x64"]))));
    flaky_transport.route("https://updates.example/9.9.9/MightyCode-windows-x64.zip", FakeRoute::ok(package.clone()));
    let flaky = Arc::new(Service::new(flaky_transport.clone(), &state, Some(fixture_key())));
    let stuck = Coordinator::new(flaky, with(true, flag(true)));
    stuck.check(None).await;
    assert!(stuck.state().phase == Phase::Available && stuck.staging_deferred(), "deferred while staging is busy");
    flaky_transport.route(MANIFEST_URL, FakeRoute { status: 500, location: None, body: Vec::new() });
    stuck.check(None).await;
    assert!(
        stuck.state().phase == Phase::Failed && !stuck.staging_deferred(),
        "a failed check clears the deferral; got {:?}, deferred {}",
        stuck.state().phase,
        stuck.staging_deferred()
    );
    flaky_transport.route(MANIFEST_URL, FakeRoute::ok(envelope(&windows_manifest("9.9.9", &package, &["x64"]))));
    assert!(stuck.check_automatically_if_due(None, true, None, Utc::now()).await, "the next tick's due check runs");

    // What counts as busy for real: a fresh copy beside the install, not an old one or none (the process check is the platform's).
    let nothing_runs = |_: &Path| Ok(false);
    let fresh =
        mighty_updates::app_update::dotnet::with_suffix(&mighty_updates::app_update::dotnet::full_path(&install).unwrap(), ".update-new");
    assert!(
        !housekeeping::staging_busy(service.updates_directory(), Some(&install), Utc::now(), &nothing_runs),
        "no copy and no helper: not busy"
    );
    make_install(&fresh, "copying");
    assert!(
        housekeeping::staging_busy(service.updates_directory(), Some(&install), Utc::now(), &nothing_runs),
        "a copy written just now is busy"
    );
    let much_later = Utc::now() + chrono::Duration::from_std(NEW_COPY_GRACE).unwrap() + chrono::Duration::minutes(1);
    assert!(!housekeeping::staging_busy(service.updates_directory(), Some(&install), much_later, &nothing_runs), "an old copy is not");
    assert!(
        housekeeping::staging_busy(service.updates_directory(), Some(&install), much_later, &|_| Err(io::Error::other("no list"))),
        "a process check that cannot tell is busy"
    );
    std::fs::remove_dir_all(&fresh).unwrap();
}

/// Verification.cs "app update an interrupted swap is put back".
#[test]
fn app_update_an_interrupted_swap_is_put_back() {
    let root = Temp::new();
    let install = root.path().join("MightyCode");
    let backup = |stamp: &str| root.path().join(format!("MightyCode.backup-{stamp}"));
    let (older, newer) = (backup("20261001T000000000"), backup("20261020T000000000"));
    assert!(!housekeeping::restore_interrupted_install(&install), "with no backup there is nothing to put back");

    make_install(&older, "older");
    make_install(&newer, "newer");
    assert!(housekeeping::restore_interrupted_install(&install), "a missing install is put back");
    assert!(read(install.join("MightyCode.exe")) == "newer" && !newer.exists() && older.is_dir(), "the newest backup is renamed back");
    assert!(!housekeeping::restore_interrupted_install(&install), "a whole install is left alone");
    assert!(older.is_dir(), "a whole install leaves the backups to the pruning");

    std::fs::remove_file(install.join("MightyCode.exe")).unwrap();
    std::fs::write(install.join("half.dll"), "half").unwrap();
    assert!(housekeeping::restore_interrupted_install(&install), "an install without its executable is put back");
    assert!(
        read(install.join("MightyCode.exe")) == "older" && !install.join("half.dll").exists(),
        "the half install is replaced by the backup"
    );
    std::fs::remove_file(install.join("MightyCode.exe")).unwrap();
    std::fs::create_dir_all(backup("20261021T000000000")).unwrap();
    assert!(!housekeeping::restore_interrupted_install(&install), "a backup without the executable is not put back");
    std::fs::remove_dir_all(&install).unwrap();
    std::fs::remove_dir_all(backup("20261021T000000000")).unwrap();

    // The helper puts the install back before it starts, then swaps as usual.
    let (helper_install, _, plan) = swap_fixture(&root.path().join("helper"), false);
    let aside = mighty_updates::app_update::dotnet::with_suffix(&helper_install, ".backup-20261001T000000000");
    std::fs::rename(&helper_install, &aside).unwrap();
    let result = replacement::run(&plan, &Fake::new(true, ok), &ko());
    assert!(
        result.replaced && result.moves[0].kind == MoveKind::Backup,
        "the helper puts the install back, then backs it up and swaps: {:?}",
        result.error
    );
    assert_eq!(read(helper_install.join("MightyCode.exe")), "new", "the new version is installed");
    assert!(!aside.exists(), "the put-back backup is no longer beside the install");

    // A copy a killed helper left beside the install is cleared with the backups at a later start; one written in the last minutes may
    // still be filling (an install on quit's helper while the app was opened again) and is kept.
    make_install(&plan.new_directory(), "partial");
    housekeeping::prune_backups(&helper_install, Utc::now());
    assert!(plan.new_directory().join("MightyCode.exe").is_file(), "a copy written just now is left to the helper writing it");
    let much_later = Utc::now() + chrono::Duration::from_std(NEW_COPY_GRACE).unwrap() + chrono::Duration::minutes(1);
    housekeeping::prune_backups(&helper_install, much_later);
    assert!(!plan.new_directory().exists() && helper_install.is_dir(), "a leftover copy is pruned, the install stays");
}

/// Verification.cs "app update a check beside a staged update decides as on macOS".
#[tokio::test]
async fn app_update_a_check_beside_a_staged_update_decides_as_on_macos() {
    let root = Temp::new();
    let install = root.path().join("MightyCode");
    make_install(&install, "old");
    let package_path = make_zip(root.path(), "MightyCode-windows-x64.zip", &[Entry::File("MightyCode.exe", &fake_executable("x64"))]);
    let package = std::fs::read(&package_path).unwrap();
    let fake = Arc::new(FakeUpdateTransport::default());
    for version in ["9.9.9", "10.0.0", "10.0.1", "10.0.2"] {
        fake.route(&format!("https://updates.example/{version}/MightyCode-windows-x64.zip"), FakeRoute::ok(package.clone()));
    }
    let manifest = |version: &str, architecture: &str| FakeRoute::ok(envelope(&windows_manifest(version, &package, &[architecture])));
    fake.route(MANIFEST_URL, manifest("9.9.9", "x64"));
    let gated = Arc::new(Gated { inner: fake.clone(), gate: Mutex::new(None) });
    let service = Arc::new(Service::new(gated.clone(), &root.path().join("state"), Some(fixture_key())));
    let (launched, launch) = launches();
    let automatic = Arc::new(Mutex::new(true));
    let coordinator = Coordinator::new(
        service.clone(),
        Options { launch_helper: Some(launch.clone()), automatic: shared_flag(&automatic), ..options("x64", Some(&install)) },
    );

    coordinator.check(None).await;
    assert!(
        coordinator.state().phase == Phase::Ready && downloads(&fake) == 1,
        "the first version is staged: {:?}",
        coordinator.state().error_message
    );
    let first_plan = coordinator.state().install_plan;

    // A newer version only for arm64 keeps the staged x64 update, without a download.
    fake.route(MANIFEST_URL, manifest("10.0.0", "arm64"));
    coordinator.check(None).await;
    assert!(
        coordinator.state().phase == Phase::Ready && coordinator.state().install_plan == first_plan && downloads(&fake) == 1,
        "a newer version this machine cannot install keeps the staged update ready"
    );
    assert_eq!(coordinator.state().availability.unwrap().manifest.version, "9.9.9", "the staged version is still the one shown");

    // A newer version for this machine replaces the staged plan and downloads again.
    fake.route(MANIFEST_URL, manifest("10.0.1", "x64"));
    coordinator.check(None).await;
    assert!(
        coordinator.state().phase == Phase::Ready && downloads(&fake) == 2,
        "a newer version is downloaded and staged: {:?}",
        coordinator.state().error_message
    );
    let state = coordinator.state();
    assert!(
        state.availability.unwrap().manifest.version == "10.0.1" && state.install_plan.is_some() && state.install_plan != first_plan,
        "the newer version's plan replaces the staged one"
    );

    // The switch off shows through the coordinator's own describe.
    let t = |field: &str| text(&ko(), field);
    *automatic.lock().unwrap() = false;
    assert_eq!(
        coordinator.describe(&ko(), |_| "00:00".into()).status_text,
        t("ReadyTemplate").replace("{version}", "10.0.1"),
        "with the switch off a staged update waits for restart now"
    );
    *automatic.lock().unwrap() = true;
    assert_eq!(
        coordinator.describe(&ko(), |_| "00:00".into()).status_text,
        t("StagedTemplate").replace("{version}", "10.0.1"),
        "with the switch on it installs at the next launch"
    );

    // "Restart now" while a check from Ready is still running: the late answer leaves the install alone.
    let release = Arc::new(tokio::sync::Notify::new());
    *gated.gate.lock().unwrap() = Some(release.clone());
    fake.route(MANIFEST_URL, manifest("10.0.2", "x64"));
    let running = coordinator.check(None);
    tokio::pin!(running);
    tokio::select! {
        biased;
        () = &mut running => panic!("the check must wait at the gate"),
        () = tokio::task::yield_now() => {}
    }
    assert_eq!(coordinator.state().phase, Phase::Ready, "a staged update stays ready while the check runs");
    coordinator.install(true);
    assert!(coordinator.state().phase == Phase::Installing && launched.lock().unwrap().len() == 1, "restart now installs during the check");
    release.notify_one();
    running.await;
    assert!(
        coordinator.state().phase == Phase::Installing && downloads(&fake) == 2,
        "the late check result changes nothing and downloads nothing"
    );
    *gated.gate.lock().unwrap() = None;

    // Nothing staged and the newer version is only for arm64: blocked, shown, never downloaded, no retry notice.
    fake.route(MANIFEST_URL, manifest("10.0.0", "arm64"));
    let fresh = Coordinator::new(service.clone(), Options { automatic: flag(true), ..options("x64", Some(&install)) });
    fresh.check(None).await;
    assert!(
        fresh.state().phase == Phase::Failed && fresh.state().error_message.unwrap_or_default().contains("x64"),
        "a newer version for another machine is blocked with the reason"
    );
    assert_eq!(downloads(&fake), 2, "a blocked version is never downloaded");
    let blocked = fresh.describe(&ko(), |_| "00:00".into());
    assert!(
        Some(blocked.status_text) == fresh.state().error_message && blocked.retry_notice.is_none() && blocked.kept_error.is_none(),
        "a blocked version is no failure to retry"
    );

    // A run without an install plan (dev or portable) installs nothing on quit and says nothing.
    let no_plan = Coordinator::new(service, Options { launch_helper: Some(launch), automatic: flag(true), ..options("x64", None) });
    fake.route(MANIFEST_URL, manifest("9.9.9", "x64"));
    no_plan.check(None).await;
    assert!(
        no_plan.state().phase == Phase::Ready && no_plan.state().install_plan.is_none(),
        "a run without an install folder stages without a plan"
    );
    let changes = Arc::new(Mutex::new(0));
    let count = changes.clone();
    no_plan.set_on_change(Arc::new(move || *count.lock().unwrap() += 1));
    assert!(
        !no_plan.install_on_quit(false) && *changes.lock().unwrap() == 0 && no_plan.state().phase == Phase::Ready,
        "quitting it changes nothing"
    );
}

// — The bare environment —

/// Verification.cs "an app started from a bare environment takes the user's own variables back" (the pure half; the real block is
/// W9-B's, mighty-platform-windows' tests/user_environment.rs).
#[test]
fn an_app_started_from_a_bare_environment_takes_the_users_own_variables_back() {
    // The helper's environment is SystemRoot alone: bare.
    let helper: BTreeMap<String, String> = BTreeMap::from([("SystemRoot".into(), r"C:\Windows".into())]);
    let read_from = |env: &BTreeMap<String, String>, name: &str| environment::get(env, name).map(String::from);
    assert!(environment::is_bare(&|name| read_from(&helper, name)), "an environment with only SystemRoot is bare");
    let whole = |name: &str| match name {
        "USERPROFILE" => Some(r"C:\Users\u".to_string()),
        "APPDATA" => Some("a".into()),
        "LOCALAPPDATA" => Some("l".into()),
        _ => None,
    };
    assert!(!environment::is_bare(&whole), "a whole environment is not bare");
    assert!(
        environment::is_bare(&|name| Some(if name == "USERPROFILE" { String::new() } else { "x".into() })),
        "an empty USERPROFILE is bare"
    );

    let fresh = environment::parse(
        "=C:=C:\\\0USERPROFILE=C:\\Users\\u\0APPDATA=C:\\Users\\u\\AppData\\Roaming\0PATHEXT=.COM;.EXE;.BAT;.CMD\0SystemRoot=D:\\Other\0Path=C:\\Windows\\System32\0EMPTY=\0\0",
    );
    assert!(!fresh.contains_key("") && !fresh.keys().any(|name| name.starts_with('=')), "drive-letter entries are left out");
    assert!(
        environment::get(&fresh, "userprofile") == Some(r"C:\Users\u") && environment::get(&fresh, "PATH") == Some(r"C:\Windows\System32"),
        "the parsed block is case-blind"
    );
    let duplicate = environment::parse("Path=a\0PATH=b\0");
    assert!(
        duplicate.len() == 1 && duplicate.get("Path").map(String::as_str) == Some("b"),
        "a later duplicate wins and the first name is kept"
    );

    let missing: BTreeMap<String, String> = environment::missing(&|name| read_from(&helper, name), &fresh).into_iter().collect();
    let has = |name: &str| missing.keys().any(|key| key.eq_ignore_ascii_case(name));
    assert!(
        has("USERPROFILE")
            && has("APPDATA")
            && missing.get("PATHEXT").map(String::as_str) == Some(".COM;.EXE;.BAT;.CMD")
            && missing.contains_key("Path"),
        "the missing user variables are added"
    );
    assert!(!has("SystemRoot"), "a variable the process already has keeps its value");
    assert!(!has("EMPTY"), "an empty fresh value adds nothing");
}

// — Beyond C#'s checks: the port's own seams —

/// A shutdown while a check waits for the network cancels it as C#'s `ShutdownAsync` does: back to Idle without an error, and nothing
/// starts afterwards.
#[tokio::test]
async fn a_shutdown_cancels_a_check_under_way() {
    let root = Temp::new();
    let fake = Arc::new(FakeUpdateTransport::default());
    fake.route(MANIFEST_URL, FakeRoute::ok(envelope(&Plain::default().text())));
    let gate = Arc::new(tokio::sync::Notify::new());
    let gated = Arc::new(Gated { inner: fake.clone(), gate: Mutex::new(Some(gate)) });
    let coordinator = Coordinator::new(Arc::new(Service::new(gated, root.path(), Some(fixture_key()))), options("x64", None));
    let running = coordinator.check(None);
    tokio::pin!(running);
    tokio::select! {
        biased;
        () = &mut running => panic!("the check must wait at the gate"),
        () = tokio::task::yield_now() => {}
    }
    assert_eq!(coordinator.state().phase, Phase::Checking, "the check is under way");
    coordinator.shutdown();
    running.await;
    let state = coordinator.state();
    assert!(
        state.phase == Phase::Idle && state.error_message.is_none() && state.last_error.is_none(),
        "a cancelled check goes back quietly: {state:?}"
    );
    coordinator.check(None).await;
    assert_eq!(coordinator.state().phase, Phase::Idle, "nothing starts after the shutdown");
}

/// install.log goes beside the package, a second helper's line after the first.
#[test]
fn the_install_log_is_appended_beside_the_package() {
    let root = Temp::new();
    let (_, package, plan) = swap_fixture(root.path(), false);
    let path = mighty_updates::app_update::log::path(&plan, Path::new("unused"));
    assert_eq!(path, package.parent().unwrap().join("install.log"));
    let now = Utc.with_ymd_and_hms(2026, 11, 15, 3, 4, 5).unwrap();
    let first = mighty_updates::app_update::log::line(&guard::refusal(false).unwrap(), Some(&plan), now);
    let second =
        mighty_updates::app_update::log::line(&mighty_updates::app_update::log::unexpected("IOException", "denied"), Some(&plan), now);
    mighty_updates::app_update::log::append(&path, &first).unwrap();
    mighty_updates::app_update::log::append(&path, &second).unwrap();
    assert_eq!(
        read(&path),
        "2026-11-15T03:04:05.0000000+00:00 replaced=False rolledBack=False relaunch=False skipped=True started=False backupRemoved=False another update helper is already running for this install\n\
         2026-11-15T03:04:05.0000000+00:00 replaced=False rolledBack=False relaunch=False skipped=False started=False backupRemoved=False IOException: denied\n"
    );
}

// — The review's fixes (w9-review-fixes.md A) —

/// A transport whose package never sends a byte until its cancel flag is set; it records that the flag was set.
struct Stalled {
    inner: Arc<FakeUpdateTransport>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl UpdateTransport for Stalled {
    fn get_capped(&self, url: &str, cap: usize, timeout: Duration) -> BoxFuture<'_, mighty_platform::Result<UpdateResponse>> {
        self.inner.get_capped(url, cap, timeout)
    }
    fn download(&self, request: DownloadRequest) -> BoxFuture<'_, mighty_platform::Result<Downloaded>> {
        let cancelled = self.cancelled.clone();
        Box::pin(async move {
            std::fs::File::create(&request.destination)?;
            while !request.cancel.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            cancelled.store(true, Ordering::SeqCst);
            Err(mighty_platform::PlatformError::Refused("cancelled".into()))
        })
    }
}

/// M1: two presses on Available from two threads at once download once — the download is claimed in the lock that tested the phase,
/// and the host's staging check (slow here) runs outside it. Two checks at once ask for the manifest once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_presses_at_once_download_once() {
    let root = Temp::new();
    let install = root.path().join("MightyCode");
    make_install(&install, "old");
    let package_path = make_zip(root.path(), "MightyCode-windows-x64.zip", &[Entry::File("MightyCode.exe", &fake_executable("x64"))]);
    let package = std::fs::read(&package_path).unwrap();
    let fake = Arc::new(FakeUpdateTransport::default());
    fake.route(MANIFEST_URL, FakeRoute::ok(envelope(&windows_manifest("9.9.9", &package, &["x64"]))));
    fake.route("https://updates.example/9.9.9/MightyCode-windows-x64.zip", FakeRoute::ok(package));
    let gate = Arc::new(tokio::sync::Notify::new());
    let gated = Arc::new(Gated { inner: fake.clone(), gate: Mutex::new(Some(gate.clone())) });
    // Asked once as a download starts and once before it unpacks; a second download would ask again. The first caller waits a
    // moment for a second one, so two downloads that both got past the phase test would be inside it together.
    let arrivals = Arc::new((Mutex::new(0u32), std::sync::Condvar::new()));
    let slow_staging_check: Arc<dyn Fn() -> bool + Send + Sync> = {
        let arrivals = arrivals.clone();
        Arc::new(move || {
            let (count, arrived) = &*arrivals;
            let mut count = count.lock().unwrap();
            *count += 1;
            arrived.notify_all();
            let _ = arrived.wait_timeout_while(count, Duration::from_millis(200), |count| *count < 2).unwrap();
            false
        })
    };
    let coordinator = Arc::new(Coordinator::new(
        Arc::new(Service::new(gated.clone(), &root.path().join("state"), Some(fixture_key()))),
        Options { staging_busy: Some(slow_staging_check), ..options("x64", Some(&install)) },
    ));

    let checks: Vec<_> = (0..2)
        .map(|_| {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.check(None).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(50)).await;
    gate.notify_waiters();
    for check in checks {
        check.await.unwrap();
    }
    assert_eq!(fake.asked().iter().filter(|url| url.as_str() == MANIFEST_URL).count(), 1, "two checks at once ask once");
    assert_eq!(coordinator.state().phase, Phase::Available, "{:?}", coordinator.state());
    *gated.gate.lock().unwrap() = None;

    // Two presses on two threads of their own.
    let runtime = tokio::runtime::Handle::current();
    let presses: Vec<_> = (0..2)
        .map(|_| {
            let (coordinator, runtime) = (coordinator.clone(), runtime.clone());
            std::thread::spawn(move || runtime.block_on(coordinator.press(None)))
        })
        .collect();
    for press in presses {
        tokio::task::spawn_blocking(move || press.join().unwrap()).await.unwrap();
    }
    assert_eq!(downloads(&fake), 1, "exactly one download");
    assert_eq!(*arrivals.0.lock().unwrap(), 2, "one download ran: its start and its unpacking asked whether staging must wait");
    assert_eq!(coordinator.state().phase, Phase::Ready, "{:?}", coordinator.state());
}

/// L1: a download or a check whose future is dropped half way stops its transfer, deletes its version folder, gives its claim back and
/// puts its phase back, so the next press works.
#[tokio::test]
async fn a_dropped_download_or_check_leaves_nothing_stuck() {
    let root = Temp::new();
    let fake = Arc::new(FakeUpdateTransport::default());
    fake.route(MANIFEST_URL, FakeRoute::ok(envelope(&Plain::default().text())));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let detached = Arc::new(Detached { inner: fake.clone(), cancelled: cancelled.clone() });
    let service = Arc::new(Service::new(detached, root.path(), Some(fixture_key())));
    let coordinator = Coordinator::new(service.clone(), options("x64", None));
    coordinator.check(None).await;
    assert_eq!(coordinator.state().phase, Phase::Available);

    tokio::select! {
        () = coordinator.download() => panic!("the stalled download must not end"),
        () = tokio::time::sleep(Duration::from_millis(100)) => {}
    }
    let state = coordinator.state();
    assert!(state.phase == Phase::Available && state.download_fraction == 0.0, "a dropped download goes back to Available: {state:?}");
    assert!(eventually(&cancelled), "the transfer was told to stop");
    assert!(!service.updates_directory().join("9.9.9").exists(), "its version folder is gone");

    // The claim was given back: the next download starts, reaches the transport and is stopped again when dropped.
    cancelled.store(false, Ordering::SeqCst);
    tokio::select! {
        () = coordinator.download() => panic!("the stalled download must not end"),
        () = tokio::time::sleep(Duration::from_millis(50)) => {}
    }
    assert!(eventually(&cancelled) && coordinator.state().phase == Phase::Available, "a second download ran and was stopped");

    // A dropped check goes back to Idle and lets the next check run.
    let gate = Arc::new(tokio::sync::Notify::new());
    let gated = Arc::new(Gated { inner: fake.clone(), gate: Mutex::new(Some(gate)) });
    let checker =
        Coordinator::new(Arc::new(Service::new(gated.clone(), &root.path().join("other"), Some(fixture_key()))), options("x64", None));
    tokio::select! {
        () = checker.check(None) => panic!("the check must wait at the gate"),
        () = tokio::time::sleep(Duration::from_millis(20)) => {}
    }
    assert_eq!(checker.state().phase, Phase::Idle, "a dropped check goes back to Idle");
    *gated.gate.lock().unwrap() = None;
    checker.check(None).await;
    assert_eq!(checker.state().phase, Phase::Available, "the next check runs");
}

/// A transport that, like one copying on a thread of its own, keeps transferring after its future is gone until its cancel flag is
/// set; it records that the flag was set.
struct Detached {
    inner: Arc<FakeUpdateTransport>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl UpdateTransport for Detached {
    fn get_capped(&self, url: &str, cap: usize, timeout: Duration) -> BoxFuture<'_, mighty_platform::Result<UpdateResponse>> {
        self.inner.get_capped(url, cap, timeout)
    }
    fn download(&self, request: DownloadRequest) -> BoxFuture<'_, mighty_platform::Result<Downloaded>> {
        let cancelled = self.cancelled.clone();
        std::thread::spawn(move || {
            while !request.cancel.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
            cancelled.store(true, Ordering::SeqCst);
        });
        Box::pin(std::future::pending())
    }
}

fn eventually(flag: &std::sync::atomic::AtomicBool) -> bool {
    (0..500).any(|_| {
        std::thread::sleep(Duration::from_millis(2));
        flag.load(Ordering::SeqCst)
    })
}

/// L1: dropping the service's download future mid-transfer tells the transfer to stop and deletes the version folder.
#[tokio::test]
async fn a_dropped_service_download_stops_its_transfer() {
    let root = Temp::new();
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let service = Service::new(
        Arc::new(Detached { inner: Arc::new(FakeUpdateTransport::default()), cancelled: cancelled.clone() }),
        root.path(),
        Some(fixture_key()),
    );
    let asset = Asset { url: "https://updates.example/9.9.9/MightyCode-windows-x64.zip".into(), sha256: "a".repeat(64), size: 10 };
    let never = CancellationToken::new();
    tokio::select! {
        _ = service.download(&asset, "9.9.9", Arc::new(|_| {}), &never) => panic!("the transfer must not end"),
        () = tokio::time::sleep(Duration::from_millis(30)) => {}
    }
    assert!(eventually(&cancelled), "the transfer was told to stop");
    assert!(!service.updates_directory().join("9.9.9").exists(), "the version folder is gone");
}

/// L2 (stricter than C#): a signed manifest older than the staged version, or than the kept one on its check, is a failed check that
/// keeps the newer update.
#[tokio::test]
async fn an_older_manifest_never_replaces_a_newer_update() {
    let root = Temp::new();
    let install = root.path().join("MightyCode");
    make_install(&install, "old");
    let package_path = make_zip(root.path(), "MightyCode-windows-x64.zip", &[Entry::File("MightyCode.exe", &fake_executable("x64"))]);
    let package = std::fs::read(&package_path).unwrap();
    let fake = Arc::new(FakeUpdateTransport::default());
    for version in ["9.9.9", "10.0.1"] {
        fake.route(&format!("https://updates.example/{version}/MightyCode-windows-x64.zip"), FakeRoute::ok(package.clone()));
    }
    fake.route(MANIFEST_URL, FakeRoute::ok(envelope(&windows_manifest("10.0.1", &package, &["x64"]))));
    let service = service(&fake, &root.path().join("state"), Some(fixture_key()));
    let with = || Options {
        launch_helper: Some(Arc::new(|_: &InstallPlan| Ok(()))),
        automatic: flag(true),
        staging_busy: Some(flag(false)),
        ..options("x64", Some(&install))
    };
    let coordinator = Coordinator::new(service.clone(), with());
    coordinator.check(None).await;
    assert_eq!(coordinator.state().phase, Phase::Ready, "{:?}", coordinator.state());
    let plan = coordinator.state().install_plan;

    // The old 9.9.9 manifest is served again.
    fake.route(MANIFEST_URL, FakeRoute::ok(envelope(&windows_manifest("9.9.9", &package, &["x64"]))));
    coordinator.check(None).await;
    let state = coordinator.state();
    assert!(
        state.phase == Phase::Ready && state.install_plan == plan && state.availability.as_ref().unwrap().manifest.version == "10.0.1",
        "the staged update stays"
    );
    assert_eq!(state.last_error.as_deref(), Some("업데이트 확인 중 오류가 발생했습니다."), "the check failed");
    assert_eq!(downloads(&fake), 1, "nothing older is downloaded");

    // A later start finds 10.0.1 kept; an older manifest on its check is refused and the kept update stays on disk.
    let next = Coordinator::new(service.clone(), with());
    assert_eq!(next.check_kept_update(None, Utc::now()).await, KeptCheck::Checked);
    assert_eq!(next.state().phase, Phase::Failed, "the kept update's check failed: {:?}", next.state());
    assert_eq!(service.kept_version("0.1.0").as_deref(), Some("10.0.1"), "the kept update is still there");
    assert_eq!(downloads(&fake), 1, "nothing older is downloaded");
}

/// L3: a package download with no byte for the stall time stops with the size it reached and leaves nothing behind.
#[tokio::test]
async fn a_stalled_download_stops_with_what_arrived() {
    let root = Temp::new();
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stalled = Arc::new(Stalled { inner: Arc::new(FakeUpdateTransport::default()), cancelled: cancelled.clone() });
    let service = Service::new(stalled, root.path(), Some(fixture_key())).with_stall_timeout(Duration::from_millis(100));
    let asset = Asset { url: "https://updates.example/9.9.9/MightyCode-windows-x64.zip".into(), sha256: "a".repeat(64), size: 10 };
    let refused = service.download(&asset, "9.9.9", Arc::new(|_| {}), &CancellationToken::new()).await;
    let Err(ServiceError::Refused(error)) = refused else { panic!("a stall is refused, not cancelled: {refused:?}") };
    assert_eq!(error.message(&ko()), "패키지 크기가 업데이트 정보와 다릅니다 (0 ≠ 10).");
    assert!(cancelled.load(Ordering::SeqCst), "the transfer was told to stop");
    assert!(!service.updates_directory().join("9.9.9").exists(), "its folder is gone");
}

/// L4: no address or folder reaches Debug output.
#[test]
fn debug_output_leaves_addresses_and_folders_out() {
    let asset = Asset { url: "https://updates.example/x.zip?token=secret".into(), sha256: "a".repeat(64), size: 1 };
    assert!(!format!("{asset:?}").contains("updates.example"), "{asset:?}");
    let plan = create_plan(
        Path::new("C:/Users/name/staged"),
        Path::new("C:/Users/name/MightyCode"),
        Path::new("C:/Users/name/p.zip"),
        &"a".repeat(64),
        1,
        1,
    );
    assert!(!format!("{plan:?}").contains("name"), "{plan:?}");
    let state = UpdateState { install_plan: Some(plan), package_path: Some("C:/Users/name/p.zip".into()), ..UpdateState::default() };
    assert!(!format!("{state:?}").contains("name"), "{state:?}");
    let service = Service::new(Arc::new(FakeUpdateTransport::default()), Path::new("C:/Users/name/state"), None);
    assert!(!format!("{service:?}").contains("name"), "{service:?}");
}
