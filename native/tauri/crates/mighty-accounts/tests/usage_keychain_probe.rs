//! Mac stage M-J: the app's Claude usage read with the Claude CLI's Keychain item (the Mac, `runtime_probe_with_keychain`) and
//! without one (Windows, `runtime_probe`), over the `ClaudeCredentials` fake and a recording HTTPS port: nothing real is read or sent.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mighty_accounts::usage::{self, AccountUsageFailureKind, ClaudeKeychain, RuntimeProbe, UsageError, copy};
use mighty_platform::accounts::FakeClaudeCredentials;
use mighty_platform::fake::{FakeProcessHost, RecordingHttps};
use mighty_platform::http::HttpsResponse;

const SIGN_IN: &str = r#"{"claudeAiOauth":{"accessToken":"sk-ant-keychain","subscriptionType":"max","scopes":["user:profile"]}}"#;

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

/// Without a Keychain (Windows) only the CLI's file is read: none here, so the read says to sign in and sends nothing. With one (the
/// Mac) a silent read where the Keychain would ask says Keychain access is needed, and sends nothing; the one read allowed to ask
/// after the switch was turned on gets the item, whose token alone goes in the request.
#[tokio::test]
async fn the_mac_reads_claudes_sign_in_from_the_keychain_first() {
    mighty_run::strings::set_language("ko");
    let home = std::env::temp_dir().join(format!("mighty-usage-keychain-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();

    let http = Arc::new(RecordingHttps::default());
    let windows = usage::runtime_probe(parts(&home, http.clone()));
    let read = windows("claude".into()).await;
    assert_eq!(failure(&read), Some((AccountUsageFailureKind::Authentication, copy::detail_authentication())), "{read:?}");
    assert!(http.requests().is_empty());

    let store = Arc::new(FakeClaudeCredentials::with(SIGN_IN, true));
    let ask = Arc::new(AtomicBool::new(false));
    let asked = ask.clone();
    let keychain = ClaudeKeychain { store: store.clone(), interactive: Arc::new(move || asked.swap(false, Ordering::SeqCst)) };
    let mac = usage::runtime_probe_with_keychain(parts(&home, http.clone()), Some(keychain));
    let read = mac("claude".into()).await;
    assert_eq!(
        failure(&read),
        Some((AccountUsageFailureKind::Authentication, mighty_run::strings::get("usage.keychain.needed"))),
        "{read:?}"
    );
    assert!(http.requests().is_empty(), "nothing is sent without a sign-in");

    ask.store(true, Ordering::SeqCst);
    http.answer(HttpsResponse { status: 401, body: String::new(), retry_after: None, redirect_location: None });
    let read = mac("claude".into()).await;
    assert_eq!(failure(&read).map(|(kind, _)| kind), Some(AccountUsageFailureKind::Authentication), "{read:?}");
    let requests = http.requests();
    assert_eq!(requests.len(), 1, "the Keychain's sign-in was used");
    assert!(requests[0].0.headers.iter().any(|(name, value)| name == "Authorization" && value == "Bearer sk-ant-keychain"));
    assert_eq!(store.reads().iter().map(|(_, interactive)| *interactive).collect::<Vec<_>>(), [false, true], "asked once");
    let _ = std::fs::remove_dir_all(&home);
}
