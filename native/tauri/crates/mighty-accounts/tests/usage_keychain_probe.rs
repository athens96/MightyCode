//! Mac stage M-J: the app's Claude usage read with the Claude CLI's Keychain item (the Mac, `runtime_probe_with_keychain`) and
//! without one (Windows, `runtime_probe`), over the `ClaudeCredentials` fake and a recording HTTPS port: nothing real is read or sent.
//! Swift `AccountUsageService.read(provider:force:interactive:)` and `failed`'s `keychainPermission` (AccountUsageService.swift:119-176).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use mighty_accounts::usage::{self, AccountUsageFailureKind, AccountUsageService, ClaudeKeychain, RuntimeProbe, UsageError, copy};
use mighty_platform::accounts::FakeClaudeCredentials;
use mighty_platform::fake::{FakeProcessHost, RecordingHttps};
use mighty_platform::http::HttpsResponse;
use mighty_platform::{ClaudeCredentials, PlatformError};

const SIGN_IN: &str = r#"{"claudeAiOauth":{"accessToken":"sk-ant-keychain","subscriptionType":"max","scopes":["user:profile"]}}"#;

fn home(name: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("mighty-usage-keychain-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    home
}

fn parts(home: &std::path::Path, http: Arc<RecordingHttps>) -> RuntimeProbe {
    let home_text = home.to_string_lossy().into_owned();
    RuntimeProbe {
        find: Arc::new(|_| Box::pin(async { None })),
        environment: Arc::new(BTreeMap::new),
        home: Arc::new(move || home_text.clone()),
        working_directory: home.to_path_buf(),
        processes: Arc::new(FakeProcessHost::default()),
        http,
    }
}

fn failure(read: &Result<usage::AccountUsageSnapshot, UsageError>) -> Option<(AccountUsageFailureKind, String)> {
    match read {
        Err(UsageError::Failure(failure)) => Some((failure.kind, failure.detail.clone())),
        _ => None,
    }
}

fn unauthorized() -> HttpsResponse {
    HttpsResponse { status: 401, body: String::new(), retry_after: None, redirect_location: None }
}

/// Without a Keychain (Windows) only the CLI's file is read: none here, so the read says to sign in and sends nothing. With one (the
/// Mac) a silent read where the Keychain would ask is `KeychainPermission` and sends nothing; a read the user asked for gets the
/// item, whose token alone goes in the request.
#[tokio::test]
async fn the_mac_reads_claudes_sign_in_from_the_keychain_first() {
    mighty_run::strings::set_language("ko");
    let home = home("probe");
    let http = Arc::new(RecordingHttps::default());
    let windows = usage::runtime_probe(parts(&home, http.clone()));
    let read = windows("claude".into()).await;
    assert_eq!(failure(&read), Some((AccountUsageFailureKind::Authentication, copy::detail_authentication())), "{read:?}");
    assert!(http.requests().is_empty());

    let store = Arc::new(FakeClaudeCredentials::with(SIGN_IN, true));
    let mac = usage::runtime_probe_with_keychain(parts(&home, http.clone()), Some(ClaudeKeychain::new(store.clone())));
    let read = mac("claude".into(), false).await;
    assert_eq!(
        failure(&read),
        Some((AccountUsageFailureKind::KeychainPermission, mighty_run::strings::get("usage.keychain.accessNeeded"))),
        "{read:?}"
    );
    assert!(http.requests().is_empty(), "nothing is sent without a sign-in");

    http.answer(unauthorized());
    let read = mac("claude".into(), true).await;
    assert_eq!(failure(&read).map(|(kind, _)| kind), Some(AccountUsageFailureKind::Authentication), "{read:?}");
    let requests = http.requests();
    assert_eq!(requests.len(), 1, "the Keychain's sign-in was used");
    assert!(requests[0].0.headers.iter().any(|(name, value)| name == "Authorization" && value == "Bearer sk-ant-keychain"));
    assert_eq!(store.reads().iter().map(|(_, interactive)| *interactive).collect::<Vec<_>>(), [false, true]);
    let _ = std::fs::remove_dir_all(&home);
}

/// A Keychain that never answers a silent read (a locked one waits for its unlock) is given up on after the bound and read as one
/// that would have asked; the probe's async worker is never the one that waits.
struct Stuck(Mutex<u32>);

impl ClaudeCredentials for Stuck {
    fn read(&self, _config_dir: Option<&std::path::Path>, _interactive: bool) -> mighty_platform::Result<Option<String>> {
        *self.0.lock().unwrap() += 1;
        std::thread::sleep(Duration::from_secs(2));
        Err(PlatformError::Refused("locked".into()))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_silent_read_that_never_answers_is_bounded() {
    mighty_run::strings::set_language("ko");
    let home = home("stuck");
    let http = Arc::new(RecordingHttps::default());
    let keychain = ClaudeKeychain { store: Arc::new(Stuck(Mutex::new(0))), silent_timeout: Duration::from_millis(100) };
    let mac = usage::runtime_probe_with_keychain(parts(&home, http.clone()), Some(keychain));
    let started = std::time::Instant::now();
    let read = mac("claude".into(), false).await;
    assert!(started.elapsed() < Duration::from_secs(1), "bounded, on a single-threaded runtime: {:?}", started.elapsed());
    assert_eq!(failure(&read).map(|(kind, _)| kind), Some(AccountUsageFailureKind::KeychainPermission), "{read:?}");
    let _ = std::fs::remove_dir_all(&home);
}

/// Swift `failed`'s `keychainPermission`: status `permission` with `usage.keychain.accessNeeded`, read again in an hour; a forced
/// automatic read keeps it, a read the user asks for lifts the back-off at once and passes `interactive` to the probe.
#[tokio::test]
async fn a_permission_waits_an_hour_unless_the_user_asks() {
    mighty_run::strings::set_language("ko");
    let asked = Arc::new(Mutex::new(Vec::<bool>::new()));
    let record = asked.clone();
    let probe: usage::InteractiveProbe = Arc::new(move |_provider, interactive| {
        record.lock().unwrap().push(interactive);
        Box::pin(async {
            Err(UsageError::Failure(usage::AccountUsageFailure::new(AccountUsageFailureKind::KeychainPermission, String::new())))
        })
    });
    let now = Utc.with_ymd_and_hms(2026, 11, 17, 0, 0, 0).unwrap();
    let service = AccountUsageService::with_interactive_probe(probe, Arc::new(move || now));
    let first = service.read("claude", true).await;
    assert_eq!((first.status.as_str(), first.detail.clone()), ("permission", mighty_run::strings::get("usage.keychain.accessNeeded")));
    assert_eq!(service.read("claude", true).await.status, "permission");
    assert_eq!(*asked.lock().unwrap(), [false], "the hour holds a forced automatic read back");
    assert_eq!(service.read_as("claude", true, true).await.status, "permission");
    assert_eq!(*asked.lock().unwrap(), [false, true], "the user's read goes through, allowed to ask");
}
