//! C# RemoteClientVerification (Verification.cs L856-L886): the Windows client of remote workspaces — saved connections, keys,
//! addresses, the transport, the connection's races and the runs — against C#'s scripted fake host on loopback port 0. The two checks
//! that hold the connections file open (another program's share mode) and the vault's DPAPI half run on Windows only.

mod support;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::model::Workspace;
use mighty_platform::SecretVault;
use mighty_platform::fake::MemoryVault;
use mighty_remote::remote::address::{self, RemoteAddress};
use mighty_remote::remote::client::{ClientError, ClientOptions, RemoteClient};
use mighty_remote::remote::saved::{self, SavedConnection};
use mighty_remote::remote::strings;
use mighty_remote::remote::text;
use mighty_remote::remote::transport::{self, Method, RemoteTarget, Request, TransportError};
use mighty_remote::remote::wire::{RemoteConnectionStatus, RemoteWireStart, protocol};
use mighty_run::model::StartRunRequest;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::strings::{get, get_with, set_language};
use serde_json::{Value, json};
use support::fake_host::*;

fn same(a: &Value, b: &Value) -> bool {
    fn plain(value: &Value) -> Value {
        match value {
            Value::Object(o) => Value::Object(o.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), plain(v))).collect()),
            Value::Array(items) => Value::Array(items.iter().map(plain).collect()),
            other => other.clone(),
        }
    }
    plain(a) == plain(b)
}

fn file_text(directory: &std::path::Path) -> String {
    std::fs::read_to_string(directory.join(saved::FILE_NAME)).unwrap_or_default()
}

fn write_file(directory: &std::path::Path, text: &str) {
    std::fs::write(directory.join(saved::FILE_NAME), text).unwrap();
}

/// "remote client: saved connections keep no key". The file's Unix mode (C#'s user-only check off Windows) is the state store's.
#[test]
fn saved_connections_keep_no_key() {
    let directory = temp("saved");
    saved::write(
        &directory,
        &[
            SavedConnection::new("conn-1", "studio", "100.101.102.103:43137"),
            SavedConnection::new("conn-2", "mac", "[fd7a:115c:a1e0::5]:43137"),
        ],
    )
    .unwrap();
    let text = file_text(&directory);
    assert!(text.contains("\"version\":2") && !text.to_lowercase().contains("key"), "the version and no key: {text}");
    assert_eq!(saved::read(&directory).iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["conn-1", "conn-2"], "read back in order");
    write_file(&directory, r#"{"version":1,"connections":[{"id":"old","name":"x","address":"100.1.2.3"}]}"#);
    assert!(saved::read(&directory).is_empty(), "a version 1 file is not read");
    write_file(
        &directory,
        r#"{"version":2,"connections":[{"id":"bad id","name":"x","address":"a"},{"id":"ok","name":"y","address":"b"},{"id":"ok","name":"z","address":"c"}]}"#,
    );
    assert_eq!(saved::read(&directory).iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["y"], "bad and repeated ids are skipped");
    write_file(
        &directory,
        &format!(
            r#"{{"version":2,"connections":[null,{{"id":"no-address","name":"x"}},{{"id":"ok","name":null,"address":"b","key":"{KEY}"}}]}}"#
        ),
    );
    let kept = saved::read(&directory);
    assert_eq!(kept, [SavedConnection::new("ok", "", "b:43137")], "a null entry or one without an address is skipped alone");
    saved::write(&directory, &kept).unwrap();
    assert!(!file_text(&directory).contains(KEY), "a key a hand edit left in the file is gone after the next write");
    write_file(&directory, r#"{"version":2,"connections":[{"id":"ok","name":"x","address":"b"}"#);
    assert!(saved::read(&directory).is_empty(), "a damaged file reads as no connections");
    std::fs::write(directory.join(saved::FILE_NAME), vec![0u8; 256 * 1024 + 1]).unwrap();
    assert!(saved::read(&directory).is_empty(), "an oversize file is not read");
    assert!(saved::read(&directory.join("missing")).is_empty(), "no file, no connections");
    let name = format!("{}\u{1F600}", "a".repeat(120));
    let mut many = vec![SavedConnection::new("emoji", &name, "b")];
    many.extend((0..20).map(|i| SavedConnection::new(&format!("c{i}"), "n", &format!("h{i}"))));
    many.push(SavedConnection::new("bad id", "x", "b"));
    saved::write(&directory, &many).unwrap();
    let back = saved::read(&directory);
    assert!(back.len() == protocol::MAXIMUM_CONNECTIONS && back[0].name == "a".repeat(120), "cut at 120 characters, at most 16 kept");
    assert!(!file_text(&directory).contains("bad id"), "an entry the next read would drop is never written");
    let _ = std::fs::remove_dir_all(&directory);
}

/// "remote client: keys stay in the vault". mighty-platform's `MemoryVault` and, on Windows, `DpapiVault` (file names, entropy and the
/// DPAPI round trip are that crate's own checks; here the remote rules on top of them).
#[test]
fn keys_stay_in_the_vault() {
    let memory = MemoryVault::default();
    memory.save("conn-1", KEY).unwrap();
    assert_eq!(memory.load("conn-1").as_deref(), Some(KEY), "the memory vault keeps a key");
    memory.delete("conn-1").unwrap();
    assert!(memory.load("conn-1").is_none(), "and forgets it");
    #[cfg(windows)]
    {
        use mighty_platform_windows::{DpapiVault, REMOTE_KEYS_FOLDER, remote_key_file_name as file_name};
        let directory = temp("vault");
        let hashed = file_name("conn:1");
        assert!(
            hashed.len() == 68
                && hashed.ends_with(".bin")
                && hashed[..64].bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        );
        assert_ne!(hashed, file_name("conn:2"), "a key file is named by the id's SHA-256 alone");
        let dpapi = DpapiVault::new(directory.clone());
        dpapi.save("conn:1", KEY).unwrap();
        let folder = directory.join(REMOTE_KEYS_FOLDER);
        let file = std::fs::read_dir(&folder).unwrap().next().unwrap().unwrap().path();
        assert!(
            !String::from_utf8_lossy(&std::fs::read(&file).unwrap()).contains(KEY) && !file.to_string_lossy().contains("conn"),
            "ciphertext under a hashed name"
        );
        assert_eq!(DpapiVault::new(directory.clone()).load("conn:1").as_deref(), Some(KEY), "a new vault reads it back");
        dpapi.delete("conn:1").unwrap();
        assert!(!file.exists(), "delete removes the file");
        assert!(!format!("{dpapi:?}").contains(&directory.to_string_lossy().into_owned()), "the vault prints only its type");
        let _ = std::fs::remove_dir_all(&directory);
    }
}

/// "remote client: addresses parse with the default port".
#[test]
fn addresses_parse_with_the_default_port() {
    set_language("ko");
    assert_eq!(RemoteAddress::parse("100.101.102.103").unwrap(), RemoteAddress { host: "100.101.102.103".into(), port: 43137 });
    assert_eq!(RemoteAddress::parse(" http://100.1.2.3:5000/ ").unwrap().display(), "100.1.2.3:5000");
    assert_eq!(RemoteAddress::parse("Studio-PC.tail1234.ts.net").unwrap().display(), "studio-pc.tail1234.ts.net:43137");
    assert_eq!(RemoteAddress::parse("studio-pc:43200").unwrap().display(), "studio-pc:43200");
    assert_eq!(RemoteAddress::parse("[fd7a:115c:a1e0::1]:43200").unwrap().display(), "[fd7a:115c:a1e0::1]:43200");
    assert_eq!(RemoteAddress::parse("fd7a:115c:a1e0:0::1").unwrap().display(), "[fd7a:115c:a1e0::1]:43137", "IPv6 is canonical");
    assert_eq!(RemoteAddress::parse("\t \u{a0}HTTP://studio-pc\r\n\u{85}\u{2028}").unwrap().display(), "studio-pc:43137", "trimmed");
    assert!(
        RemoteAddress::parse("studio-pc:+5000").unwrap().port == 5000 && RemoteAddress::parse("[fd7a::1]:+43200").unwrap().port == 43200
    );
    let long = "a".repeat(400);
    for bad in [
        "",
        "https://100.1.2.3",
        "ftp://x",
        "user@100.1.2.3",
        "100.1.2.3/path",
        "100.1.2.3?k=v",
        "100.1.2.3#x",
        "100.1.2.3:80",
        "100.1.2.3:99999",
        "100.1.2.3:abc",
        "[fd7a::1",
        "-host",
        "a..b",
        "host name",
        "fe80::1%en0",
        long.as_str(),
        "studio\u{7}pc",
        "studio-pc\u{0}",
        "100.1.2.3:",
        "100.1.2.3:+",
        "100.1.2.3: 5000",
        "[fd7a::1]x",
        "[studio:pc]",
        "studio_pc",
        "\u{7}\u{8}",
        "\u{7}studio-pc",
        "studio-pc\u{1b}",
        "\u{9b}studio-pc",
        "studio-pc:-1",
        "studio-pc:99999999999",
    ] {
        assert!(RemoteAddress::parse(bad).is_err(), "accepted {bad:?}");
    }
    for (input, words) in [
        ("100.1.2.3:80", "badPort"),
        ("studio-pc:-1", "badPort"),
        ("[fd7a::1]:-0", "badPort"),
        ("studio-pc:99999999999", "badAddress"),
        ("user@x", "badAddress"),
        ("\u{7}studio-pc", "badAddress"),
    ] {
        assert_eq!(
            RemoteAddress::parse(input).unwrap_err(),
            get(&format!("remote.connect.error.{words}")),
            "{input} is refused with {words}"
        );
    }
}

/// "remote client: saved addresses read as the Mac reads them".
#[test]
fn saved_addresses_read_as_the_mac_reads_them() {
    let directory = temp("addresses");
    write_file(
        &directory,
        r#"{"version":2,"connections":[{"id":"a","name":"a","address":"Studio-PC"},{"id":"b","name":"b","address":"http://studio-pc:43137/"},{"id":"c","name":"c","address":"user@100.1.2.3"},{"id":"d","name":"d","address":"fd7a:115c:a1e0:0::5"},{"id":"e","name":"e","address":"[fd7a:115c:a1e0::5]:43137"},{"id":"f","name":"f","address":"100.101.102.103:80"},{"id":"g","name":"g","address":"100.101.102.103"}]}"#,
    );
    let read: Vec<String> = saved::read(&directory).iter().map(|c| format!("{}={}", c.id, c.address)).collect();
    assert_eq!(read, ["a=studio-pc:43137", "d=[fd7a:115c:a1e0::5]:43137", "g=100.101.102.103:43137"]);
    let entries: Vec<String> = (0..16)
        .map(|i| format!(r#"{{"id":"{}","name":"n","address":"h{i}"}}"#, if i < 2 { "bad id".to_string() } else { format!("p{i}") }))
        .collect();
    write_file(
        &directory,
        &format!(r#"{{"version":2,"connections":[{},{{"id":"late","name":"n","address":"late"}}]}}"#, entries.join(",")),
    );
    let first = saved::read(&directory);
    assert!(first.len() == 14 && first[0].id == "p2" && first.iter().all(|c| c.id != "late"), "only the file's first 16 entries are read");
    let mut many = vec![SavedConnection::new("bad id", "x", "h0"), SavedConnection::new("bad id", "x", "h1")];
    many.extend((2..18).map(|i| SavedConnection::new(&format!("p{i}"), "n", &format!("h{i}"))));
    saved::write(&directory, &many).unwrap();
    assert_eq!(saved::read(&directory).len(), protocol::MAXIMUM_CONNECTIONS, "a write keeps 16 good entries, whatever it skipped first");
    saved::write(
        &directory,
        &[
            SavedConnection::new("x", "x", "100.101.102.103"),
            SavedConnection::new("y", "y", "100.101.102.103:43137"),
            SavedConnection::new("z", "z", "not an address/"),
        ],
    )
    .unwrap();
    let written: Vec<String> = saved::read(&directory).iter().map(|c| format!("{}={}", c.id, c.address)).collect();
    assert_eq!(written, ["x=100.101.102.103:43137"], "a repeated or unparsable address is never written");
    let _ = std::fs::remove_dir_all(&directory);
}

/// "remote client: only Tailscale peers, loopback only when injected".
#[test]
fn only_tailscale_peers_and_loopback_when_injected() {
    use address::{peer_allowed, peer_ip_allowed, source_address_toward, through_tailscale};
    assert!(!peer_allowed("127.0.0.1", false) && !peer_allowed("::1", false), "loopback is never a peer by default");
    assert!(
        peer_allowed("127.0.0.1", true) && peer_allowed("::1", true) && !peer_allowed("192.168.0.2", true),
        "loopback only when injected"
    );
    assert!(peer_allowed("100.90.1.1", false) && peer_allowed("fd7a:115c:a1e0::5%17", false) && !peer_allowed("fe80::1%17", false));
    assert!(peer_ip_allowed("::ffff:100.90.1.1".parse().unwrap(), false) && !peer_ip_allowed("10.0.0.1".parse().unwrap(), false));
    for outside in ["100.63.255.255", "100.128.0.1", "fd7a:115c:a1e1::1", "192.168.0.2", "studio-pc", ""] {
        assert!(!peer_allowed(outside, false), "{outside} is refused");
    }
    for loose in ["127.1", "2130706433", "0x7f.1", "0177.0.0.1", "::ffff:127.0.0.1", "0:0:0:0:0:0:0:1", "127.0.0.256", "127.0.0.1.1"] {
        assert!(!peer_allowed(loose, true), "only the Mac's loopback spellings count when injected: {loose}");
    }
    assert!(peer_allowed("127.9.8.7", true) && peer_allowed("[::1]", true) && peer_allowed("::1%1", true));
    assert!(
        !peer_ip_allowed("127.0.0.1".parse().unwrap(), false)
            && peer_ip_allowed("::1".parse().unwrap(), true)
            && peer_ip_allowed("::ffff:127.0.0.1".parse().unwrap(), true)
    );
    assert_eq!(
        source_address_toward("127.0.0.1").as_deref(),
        Some("127.0.0.1"),
        "the route to loopback leaves from loopback, nothing sent"
    );
    assert!(source_address_toward("not an ip").is_none(), "garbage has no route");
    assert!(!through_tailscale("127.0.0.1", &|| Err("adapters".into())), "adapters that cannot be read are no tunnel");
    assert!(!through_tailscale("127.0.0.1", &|| Ok(vec!["100.101.102.103".into(), "fd7a:115c:a1e0::5".into()])), "not one of this PC's");
    assert!(through_tailscale("127.0.0.1", &|| Ok(vec!["100.101.102.103".into(), "127.0.0.1".into()])), "among this PC's is the tunnel");
    assert!(!through_tailscale("not an ip", &|| Ok(vec!["127.0.0.1".into()])), "no route, no tunnel");
    assert!(source_address_toward("studio-pc").is_none() && source_address_toward("").is_none(), "a name is never resolved here");
}

fn request(path: &str) -> (&str, Request<'static>) {
    (path, Request::default())
}

/// "remote client: one authenticated request follows no redirect and reads at most 2 MiB".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_authenticated_request_follows_no_redirect_and_reads_at_most_two_mebibytes() {
    set_language("ko");
    let elsewhere = FakeHost::new(|_| Reply::fixture("info.json", 200)).await;
    let elsewhere_port = elsewhere.port;
    let huge = format!("\"{}\"", "x".repeat(protocol::MAXIMUM_RESPONSE_BYTES));
    let host = FakeHost::new(move |seen| match seen.target.as_str() {
        "/v2/info" => Reply::fixture("info.json", 200),
        "/v2/moved" => {
            Reply { headers: vec![("Location".into(), format!("http://127.0.0.1:{elsewhere_port}/v2/info"))], ..Reply::new(302, "{}") }
        }
        "/v2/plain" => Reply { no_version: true, ..Reply::new(200, "{}") },
        "/v2/huge" => Reply::new(200, &huge),
        "/v2/refused" => Reply::fixture("error.json", 401),
        "/v2/busy" => Reply::new(503, r#"{"protocol":2,"error":"busy"}"#),
        "/v2/slow" => Reply { delay: Some(Duration::from_secs(3)), ..Reply::new(200, "{}") },
        "/v2/huge-unsized" => Reply { omit_length: true, ..Reply::new(200, &huge) },
        "/v2/stalled" => Reply { body_delay: Some(Duration::from_secs(3)), ..Reply::new(200, "{}") },
        "/v2/twice" => Reply { headers: vec![("X-Mighty-Remote-Version".into(), "2".into())], ..Reply::new(200, "{}") },
        "/v2/lone" => Reply::new(401, r#"{"protocol":2,"error":"\uD83D"}"#),
        _ => Reply::new(404, r#"{"protocol":2,"error":"no"}"#),
    })
    .await;
    let target = loopback(host.port);
    let call = |path: &str, timeout: Option<Duration>| {
        let (target, path) = (target.clone(), path.to_string());
        async move { transport::request(&target, KEY, Method::Get, &path, Request { timeout, ..Request::default() }).await }
    };
    let info = call("/v2/info", None).await.unwrap();
    assert!(same(&serde_json::from_slice(&info).unwrap(), &fixture("info.json")), "the body arrives whole");
    let seen = host.seen().last().cloned().unwrap();
    assert!(
        seen.headers["authorization"] == format!("Bearer {KEY}")
            && seen.headers["x-mighty-remote-version"] == "2"
            && !seen.headers.contains_key("origin")
            && !seen.headers.contains_key("cookie")
    );
    assert!(!seen.target.contains(KEY), "the key is never in the URL");
    assert!(matches!(call("/v2/moved", None).await, Err(TransportError::InvalidData(_))), "a redirect is an error");
    assert!(elsewhere.seen().is_empty(), "a redirect is never followed");
    assert_eq!(call("/v2/plain", None).await.unwrap_err(), TransportError::InvalidData(get("remote.connect.error.notMighty")));
    assert_eq!(call("/v2/huge", None).await.unwrap_err().message(), get("remote.connect.error.responseTooLarge"), "over 2 MiB is refused");
    assert_eq!(
        call("/v2/huge-unsized", None).await.unwrap_err(),
        TransportError::InvalidData(get("remote.connect.error.responseTooLarge"))
    );
    assert_eq!(
        call("/v2/twice", None).await.unwrap_err(),
        TransportError::InvalidData(get("remote.connect.error.notMighty")),
        "a repeated version"
    );
    let refused = call("/v2/refused", None).await.unwrap_err();
    assert!(refused.status() == Some(401) && !refused.transient() && refused.message() == "The key does not match.", "the host's message");
    assert!(call("/v2/busy", None).await.unwrap_err().transient(), "503 is a passing failure");
    let lone = call("/v2/lone", None).await.unwrap_err();
    assert!(
        lone.status() == Some(401) && lone.message() == get_with("remote.connect.error.unexpectedStatus", &[("status", "401")]),
        "a lone surrogate reads as the status"
    );
    let slow = call("/v2/slow", Some(Duration::from_millis(400))).await.unwrap_err();
    assert!(matches!(&slow, TransportError::Timeout(m) if *m == strings::timeout()) && slow.transient(), "a slow host times out");
    assert!(
        matches!(call("/v2/stalled", Some(Duration::from_millis(400))).await, Err(TransportError::Timeout(_))),
        "a stalled body times out"
    );
    transport::request(
        &target,
        KEY,
        Method::Get,
        "/v2/info",
        Request { headers: &[(protocol::GRAPH_HEADER, protocol::GRAPH_RUNS)], ..Request::default() },
    )
    .await
    .unwrap();
    assert_eq!(host.seen().last().unwrap().headers[protocol::GRAPH_HEADER], protocol::GRAPH_RUNS, "the graph opt-in header goes out");
    let before = host.seen().len();
    let refused_header = transport::request(
        &target,
        KEY,
        Method::Get,
        "/v2/info",
        Request { headers: &[("Origin", "http://example.invalid")], ..Request::default() },
    )
    .await;
    assert!(matches!(refused_header, Err(TransportError::Argument(_))), "only allow-listed extra headers");
    let own = format!("Bearer {KEY}");
    let overridden = transport::request(
        &target,
        KEY,
        Method::Get,
        "/v2/info",
        Request { headers: &[("Authorization", own.as_str())], ..Request::default() },
    )
    .await;
    assert!(matches!(overridden, Err(TransportError::Argument(_))), "the transport's own headers are never overridden");
    let unflagged = RemoteTarget::new(RemoteAddress { host: "127.0.0.1".into(), port: host.port }, "127.0.0.1", host.port, false);
    assert_eq!(
        transport::request(&unflagged, KEY, Method::Get, "/v2/info", Request::default()).await.unwrap_err().message(),
        get("remote.connect.error.notTailscale")
    );
    let lan = RemoteTarget::new(RemoteAddress { host: "192.168.1.5".into(), port: 43137 }, "192.168.1.5", 43137, true);
    assert_eq!(
        transport::request(&lan, KEY, Method::Get, "/v2/info", Request::default()).await.unwrap_err().message(),
        get("remote.connect.error.notTailscale")
    );
    assert!(transport::request(&target, "short", Method::Get, "/v2/info", Request::default()).await.is_err(), "a malformed key");
    for (path, _) in [request("/v1/info"), request("/v2/info?x=<script>")] {
        assert!(transport::request(&target, KEY, Method::Get, path, Request::default()).await.is_err(), "{path} is refused");
    }
    assert_eq!(
        transport::request(&target, KEY, Method::Get, "/v2/../v1/info", Request::default()).await.unwrap_err().message(),
        get("remote.connect.error.badRequest")
    );
    assert_eq!(host.seen().len(), before, "nothing refused locally reaches the host");
    assert!(
        !TransportError::InvalidData("x".into()).transient()
            && !TransportError::Status(mighty_remote::remote::wire::RemoteHttpFailure { status: 400, message: "x".into() }).transient()
    );
}

/// "remote client: hosts resolve only to Tailscale".
#[tokio::test]
async fn hosts_resolve_only_to_tailscale() {
    set_language("ko");
    let address = |host: &str| RemoteAddress { host: host.into(), port: 43137 };
    let injected = transport::resolve(&address("127.0.0.1"), true).await.unwrap();
    assert!(injected.ip == "127.0.0.1" && injected.allow_loopback, "loopback when injected, and the target keeps the flag");
    assert!(!transport::resolve(&address("100.101.102.103"), false).await.unwrap().allow_loopback, "never gains the flag");
    assert_eq!(transport::resolve(&address(&"a".repeat(300)), false).await.unwrap_err().message(), get("remote.connect.error.unresolved"));
    assert_eq!(transport::resolve(&address("localhost"), true).await.unwrap().ip, "127.0.0.1", "a name resolves once, IPv4 first");
    assert_eq!(transport::resolve(&address("100.101.102.103"), false).await.unwrap().ip, "100.101.102.103", "a literal needs no lookup");
    assert_eq!(transport::resolve(&address("127.0.0.1"), false).await.unwrap_err().message(), get("remote.connect.error.notTailscale"));
    assert_eq!(transport::resolve(&address("192.168.1.5"), true).await.unwrap_err().message(), get("remote.connect.error.notTailscale"));
}

fn info_or_refused(seen: &Seen) -> Reply {
    if seen.target != "/v2/info" {
        return Reply::new(404, "{}");
    }
    if seen.headers.get("authorization").is_some_and(|a| *a == format!("Bearer {KEY}")) {
        Reply::fixture("info.json", 200)
    } else {
        Reply::fixture("error.json", 401)
    }
}

/// "remote client: connections are saved without their key and read it on first use".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_are_saved_without_their_key_and_read_it_on_first_use() {
    set_language("ko");
    let host = FakeHost::new(info_or_refused).await;
    let (store, directory) = client_store();
    let vault = Arc::new(MemoryVault::default());
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &directory, vault.clone(), &seen, loopback_options());
    let address = format!("127.0.0.1:{}", host.port);
    let info = client.connect("", &address, KEY).await.unwrap();
    assert!(info.status == RemoteConnectionStatus::Connected && info.name == "studio-pc" && info.host_name.as_deref() == Some("studio-pc"));
    assert!(
        info.runtime.as_ref().is_some_and(|r| r.platform == "win32") && info.shared().len() == 2,
        "the host's name, OS and shared workspaces"
    );
    assert!(info.detail == Some(strings::connected_saved()) && vault.load(&info.id).as_deref() == Some(KEY), "the key went to the vault");
    let file = file_text(&directory);
    assert!(!file.contains(KEY) && file.contains(&address), "the file names the address, never the key");
    assert!(
        client.connect("Studio", &address, KEY).await.unwrap().id == info.id && client.connections().len() == 1,
        "same address, same connection"
    );
    assert_eq!(client.connect("", &address, "short").await.unwrap_err().message(), get("remote.connect.error.badKey"));
    assert_eq!(client.connect("", &address, &"x".repeat(43)).await.unwrap_err().status(), Some(401), "a wrong key is the host's 401");
    assert_eq!(client.connections().len(), 1, "a refused connect saves nothing");
    let workspace = client.open_workspace(&info.id, "ws-project").unwrap();
    let reference = workspace.remote.clone().unwrap();
    assert!(
        reference.connection_id.as_deref() == Some(info.id.as_str())
            && reference.workspace_id.as_deref() == Some("ws-project")
            && reference.host_name.as_deref() == Some("studio-pc")
    );
    assert_eq!(workspace.path(), "C:\\work\\project", "the host's workspace is listed as remote");
    assert_eq!(client.open_workspace(&info.id, "ws-project").unwrap().id(), workspace.id(), "opening it again lists the same one");
    assert_eq!(client.open_workspace(&info.id, "not-shared").unwrap_err().message(), get("remote.workspace.error.notShared"));
    client.disconnect(&info.id).await;
    assert_eq!(client.connection(&info.id).unwrap().status, RemoteConnectionStatus::Disconnected, "disconnect keeps the connection");
    assert_eq!(client.ensure_connected(&info.id).await.unwrap().status, RemoteConnectionStatus::Connected, "a send reconnects");
    // After a restart the connection is saved, its key unread until first use.
    let later = client_with(&store, &directory, vault.clone(), &seen, loopback_options());
    let saved_entry = later.connections().into_iter().next().unwrap();
    assert!(saved_entry.status == RemoteConnectionStatus::Saved && saved_entry.detail == Some(get("remote.connect.detail.saved")));
    assert_eq!(later.refresh(&info.id).await.unwrap().status, RemoteConnectionStatus::Connected, "refresh reads the key and connects");
    later.remove(&info.id).await;
    assert!(
        later.connections().is_empty() && vault.load(&info.id).is_none() && saved::read(&directory).is_empty(),
        "remove forgets it and its key"
    );
    client.dispose().await;
    later.dispose().await;
    let _ = std::fs::remove_dir_all(&directory);
}

#[cfg(windows)]
fn hold_open(path: &std::path::Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(path).unwrap()
}

/// "remote client: a connections file held open is read later and never written over" (another program's share mode: Windows).
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connections_file_held_open_is_read_later_and_never_written_over() {
    let (store, directory) = client_store();
    let seen = Arc::new(Collected::default());
    saved::write(
        &directory,
        &[
            SavedConnection::new("conn-1", "studio", "100.101.102.103:43137"),
            SavedConnection::new("conn-2", "mac", "[fd7a:115c:a1e0::5]:43137"),
        ],
    )
    .unwrap();
    let path = directory.join(saved::FILE_NAME);
    let before = std::fs::read(&path).unwrap();
    let client = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, ClientOptions::default());
    let changed = Arc::new(AtomicUsize::new(0));
    let counter = changed.clone();
    client.on_changed(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    {
        let _held = hold_open(&path);
        assert!(saved::try_read(&directory).is_err(), "a file held open cannot be read now");
        assert!(client.connections().is_empty() && client.connection("conn-1").is_none(), "while it is held, nothing is listed");
        client.remove("conn-9").await;
    }
    assert_eq!(std::fs::read(&path).unwrap(), before, "no write replaced the file while it was unread");
    let held_changes = changed.load(Ordering::SeqCst);
    until(
        || client.connections().iter().map(|c| c.id.clone()).collect::<Vec<_>>() == ["conn-1", "conn-2"],
        5000,
        "the connections come back",
    )
    .await;
    assert_eq!(changed.load(Ordering::SeqCst), held_changes + 1, "the window is told once when the connections come back");
    client.remove("conn-9").await;
    assert_eq!(saved::read(&directory).iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["conn-1", "conn-2"], "a later write keeps them");
    let _ = std::fs::remove_dir_all(&directory);
}

/// "remote client: a connect waits for a held connections file and keeps the file's ids" (Windows: the share mode).
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connect_waits_for_a_held_connections_file() {
    set_language("ko");
    let host = FakeHost::new(info_or_refused).await;
    let address = format!("127.0.0.1:{}", host.port);
    let seen = Arc::new(Collected::default());
    let make = |saved_list: Vec<SavedConnection>, wait: Duration| {
        let (store, directory) = client_store();
        saved::write(&directory, &saved_list).unwrap();
        let options = ClientOptions { saved_read_wait: wait, ..loopback_options() };
        (client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, options), directory)
    };
    // The address the file holds keeps the file's id.
    let (known, known_dir) = make(
        vec![SavedConnection::new("conn-old", "studio", &address), SavedConnection::new("conn-2", "mac", "[fd7a:115c:a1e0::5]:43137")],
        Duration::from_secs(5),
    );
    let again = {
        let _held = hold_open(&known_dir.join(saved::FILE_NAME));
        let (client, address) = (known.clone(), address.clone());
        let again = tokio::spawn(async move { client.connect("", &address, KEY).await });
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!again.is_finished(), "a connect waits while the saved file is held");
        again
    };
    let info = again.await.unwrap().unwrap();
    assert!(info.id == "conn-old" && known.connections().iter().map(|c| c.id.clone()).collect::<Vec<_>>() == ["conn-old", "conn-2"]);
    assert_eq!(
        saved::read(&known_dir).iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["conn-old", "conn-2"],
        "the file keeps the old id"
    );
    // A new address is saved with the file's entries, in the same launch.
    let (fresh, fresh_dir) = make(vec![SavedConnection::new("conn-2", "mac", "[fd7a:115c:a1e0::5]:43137")], Duration::from_secs(5));
    let added = {
        let _held = hold_open(&fresh_dir.join(saved::FILE_NAME));
        let (client, address) = (fresh.clone(), address.clone());
        let added = tokio::spawn(async move { client.connect("", &address, KEY).await });
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!added.is_finished(), "a new connect waits while the saved file is held");
        added
    };
    let info = added.await.unwrap().unwrap();
    assert_eq!(
        saved::read(&fresh_dir).iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        ["conn-2".to_string(), info.id],
        "saved beside the file's"
    );
    // A file held past the wait refuses the connect and stays as it was.
    let (held, held_dir) = make(vec![SavedConnection::new("conn-2", "mac", "[fd7a:115c:a1e0::5]:43137")], Duration::from_millis(300));
    let path = held_dir.join(saved::FILE_NAME);
    let before = std::fs::read(&path).unwrap();
    {
        let _held = hold_open(&path);
        assert_eq!(held.connect("", &address, KEY).await.unwrap_err().message(), strings::saved_unreadable());
    }
    assert_eq!(std::fs::read(&path).unwrap(), before, "the refused connect left the file as it was");
    until(|| held.connections().iter().map(|c| c.id.clone()).collect::<Vec<_>>() == ["conn-2"], 5000, "read later").await;
    for directory in [known_dir, fresh_dir, held_dir] {
        let _ = std::fs::remove_dir_all(&directory);
    }
}

/// "remote client: no connection leaves this PC's tunnel".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_connection_leaves_this_pcs_tunnel() {
    set_language("ko");
    let host = FakeHost::new(|_| Reply::fixture("info.json", 200)).await;
    let (store, directory) = client_store();
    let seen = Arc::new(Collected::default());
    let tunnel = Arc::new(AtomicBool::new(true));
    let open = tunnel.clone();
    let options = ClientOptions { route_check: Some(Arc::new(move |_| Ok(open.load(Ordering::SeqCst)))), ..loopback_options() };
    let client = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, options);
    let info = client.connect("", &format!("127.0.0.1:{}", host.port), KEY).await.unwrap();
    tunnel.store(false, Ordering::SeqCst);
    let before = host.seen().len();
    let not_via = get("remote.connect.error.notViaTailscale");
    assert_eq!(client.ensure_connected(&info.id).await.unwrap_err().message(), not_via, "a route outside the tunnel ends the connection");
    assert!(
        client.connection(&info.id).unwrap().status == RemoteConnectionStatus::Disconnected && host.seen().len() == before,
        "nothing sent"
    );
    assert_eq!(client.refresh(&info.id).await.unwrap_err().message(), not_via, "refresh refuses the same route");
    assert_eq!(host.seen().len(), before, "still nothing sent");
    let offline = client_with(
        &store,
        &directory,
        Arc::new(MemoryVault::default()),
        &seen,
        ClientOptions { local_addresses: Some(Arc::new(|| Ok(vec![]))), ..ClientOptions::default() },
    );
    assert_eq!(
        offline.connect("", "100.101.102.103", KEY).await.unwrap_err().message(),
        strings::local_tailscale_off(),
        "no Tailscale here, no connect"
    );
    // A route check that fails is no tunnel either, never a passing failure, and nothing is sent.
    let broken = client_with(
        &store,
        &directory,
        Arc::new(MemoryVault::default()),
        &seen,
        ClientOptions { route_check: Some(Arc::new(|_| Err("x".into()))), ..loopback_options() },
    );
    let refused = broken.connect("", &format!("127.0.0.1:{}", host.port), KEY).await.unwrap_err();
    assert!(
        refused.message() == not_via && !matches!(&refused, ClientError::Transport(t) if t.transient()),
        "a failing route check refuses"
    );
    assert_eq!(host.seen().len(), before, "and nothing was sent");
    let _ = std::fs::remove_dir_all(&directory);
}

/// A vault that cannot keep anything beyond this launch (DPAPI refused): every save fails and deletes are counted.
#[derive(Default)]
struct RefusingVault {
    deleted: Mutex<Vec<String>>,
}

impl SecretVault for RefusingVault {
    fn load(&self, _: &str) -> Option<String> {
        None
    }
    fn save(&self, _: &str, _: &str) -> mighty_platform::Result<()> {
        Err(mighty_platform::PlatformError::Refused("no vault".into()))
    }
    fn delete(&self, id: &str) -> mighty_platform::Result<()> {
        self.deleted.lock().unwrap().push(id.into());
        Ok(())
    }
}

/// "remote client: the host is read strictly and a key the vault refuses stays in memory".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_host_is_read_strictly_and_a_key_the_vault_refuses_stays_in_memory() {
    set_language("ko");
    let renamed = fixture("info.json").to_string().replace("\"hostName\"", "\"HostName\"");
    let loose = FakeHost::new(move |_| Reply::new(200, &renamed)).await;
    let host = FakeHost::new(|_| Reply::fixture("info.json", 200)).await;
    let (store, directory) = client_store();
    let seen = Arc::new(Collected::default());
    let refusing = Arc::new(RefusingVault::default());
    let client = client_with(&store, &directory, refusing.clone(), &seen, loopback_options());
    assert_eq!(
        client.connect("", &format!("127.0.0.1:{}", loose.port), KEY).await.unwrap_err().message(),
        get("remote.connect.error.notMighty")
    );
    assert!(client.connections().is_empty(), "and nothing is saved");
    let info = client.connect("", &format!("127.0.0.1:{}", host.port), KEY).await.unwrap();
    assert!(info.detail == Some(strings::connected_memory()) && refusing.deleted.lock().unwrap().is_empty(), "kept for this launch only");
    assert!(!file_text(&directory).contains(KEY), "the file never holds the key");
    client.disconnect(&info.id).await;
    assert_eq!(client.ensure_connected(&info.id).await.unwrap().status, RemoteConnectionStatus::Connected, "the key in memory reconnects");
    let later = client_with(&store, &directory, refusing.clone(), &seen, loopback_options());
    assert_eq!(
        later.refresh(&info.id).await.unwrap_err().message(),
        get("remote.connect.error.keyMissing"),
        "after a restart the key is gone"
    );
    assert_eq!(later.connection(&info.id).unwrap().status, RemoteConnectionStatus::Disconnected, "and the connection says so");
    write_file(
        &directory,
        r#"{"version":2,"connections":[{"id":"a","name":"a","address":"Studio-PC"},{"id":"b","name":"b","address":"studio-pc:43137"},{"id":"c","name":"c","address":"user@x"}]}"#,
    );
    let edited = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, loopback_options());
    assert_eq!(edited.connections().iter().map(|c| format!("{}={}", c.id, c.address)).collect::<Vec<_>>(), ["a=studio-pc:43137"]);
    let _ = std::fs::remove_dir_all(&directory);
}

/// "remote client: text is cut at whole characters". Rust text holds no lone surrogate, so C#'s lone-surrogate cases have no Rust form.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn text_is_cut_at_whole_characters() {
    set_language("ko");
    let (emoji, family) = ("\u{1F600}", "\u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}");
    assert_eq!(text::cut(&format!("{}{emoji}bbb", "a".repeat(299)), 300), format!("{}{emoji}", "a".repeat(299)));
    assert_eq!(text::cut(&format!("{}{family}b", "a".repeat(299)), 300), format!("{}{family}", "a".repeat(299)));
    assert_eq!(text::count(&format!("{}{family}", "a".repeat(119)), usize::MAX), 120, "a joined emoji is one character");
    assert!(text::cut("abc", 0).is_empty() && text::cut("e\u{301}x", 1) == "e\u{301}" && text::count(&"a".repeat(500), 121) == 121);
    let error = format!("{}{emoji}{}", "a".repeat(299), "b".repeat(20));
    let expected = format!("{}{emoji}", "a".repeat(299));
    let refuse = Arc::new(AtomicBool::new(false));
    let (refusing, message) = (refuse.clone(), error.clone());
    let host = FakeHost::new(move |_| {
        if refusing.load(Ordering::SeqCst) {
            Reply::new(401, &json!({"protocol": 2, "error": message}).to_string())
        } else {
            Reply::fixture("info.json", 200)
        }
    })
    .await;
    refuse.store(true, Ordering::SeqCst);
    let refused = transport::request(&loopback(host.port), KEY, Method::Get, "/v2/info", Request::default()).await.unwrap_err();
    assert!(refused.status() == Some(401) && refused.message() == expected, "the host's error is cut at 300 characters");
    refuse.store(false, Ordering::SeqCst);
    let (store, directory) = client_store();
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, loopback_options());
    let address = format!("127.0.0.1:{}", host.port);
    let info = client.connect(&format!("{}{family}", "a".repeat(119)), &address, KEY).await.unwrap();
    assert_eq!(info.name, format!("{}{family}", "a".repeat(119)), "a name of 120 characters is taken whole");
    assert_eq!(
        client.connect(&format!("{}{family}", "a".repeat(120)), &address, KEY).await.unwrap_err().message(),
        get("remote.connect.error.badKey")
    );
    assert_eq!(client.connect("Stu\u{7}dio", &address, KEY).await.unwrap().name, "Studio", "a name is cleaned as the file cleans it");
    let later = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, loopback_options());
    assert_eq!(later.connections()[0].name, "Studio", "and reads the same after a restart");
    refuse.store(true, Ordering::SeqCst);
    assert!(client.refresh(&info.id).await.unwrap_err().status().is_some(), "a refused refresh");
    let now = client.connection(&info.id).unwrap();
    assert!(
        now.status == RemoteConnectionStatus::Disconnected && now.detail.as_deref() == Some(expected.as_str()),
        "cut at whole characters too"
    );
    refuse.store(false, Ordering::SeqCst);
    let fail = Arc::new(AtomicBool::new(false));
    let (failing, long) = (fail.clone(), error.clone());
    let watched = ClientOptions {
        local_addresses: Some(Arc::new(move || {
            if failing.load(Ordering::SeqCst) { Err(long.clone()) } else { Ok(vec!["100.101.102.103".into()]) }
        })),
        ..loopback_options()
    };
    let uncut = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, watched);
    let second = uncut.connect("", &address, KEY).await.unwrap();
    fail.store(true, Ordering::SeqCst);
    assert_eq!(uncut.refresh(&second.id).await.unwrap_err().message(), error, "a refresh that fails with a long message");
    assert_eq!(
        uncut.connection(&second.id).unwrap().detail.as_deref(),
        Some(expected.as_str()),
        "the connection cuts it at whole characters"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// C#'s `Gate`: holds a host's /v2/info until released, once closed, and says when one arrived.
#[derive(Clone)]
struct InfoGate {
    closed: Arc<AtomicBool>,
    arrived: Arc<tokio::sync::Notify>,
    released: Arc<tokio::sync::Semaphore>,
}

impl Default for InfoGate {
    fn default() -> Self {
        InfoGate { closed: Arc::default(), arrived: Arc::default(), released: Arc::new(tokio::sync::Semaphore::new(0)) }
    }
}

impl InfoGate {
    fn closed() -> InfoGate {
        let gate = InfoGate::default();
        gate.closed.store(true, Ordering::SeqCst);
        gate
    }
    fn answer(&self) -> Answer {
        let gate = self.clone();
        Arc::new(move |seen: &Seen| {
            let (gate, target) = (gate.clone(), seen.target.clone());
            Box::pin(async move {
                if gate.closed.load(Ordering::SeqCst) && target == "/v2/info" {
                    gate.arrived.notify_one();
                    let _ = gate.released.acquire().await;
                }
                Reply::fixture("info.json", 200)
            })
        })
    }
    fn release(&self) {
        self.released.add_permits(1000);
    }
}

/// A memory vault that refuses keys of the wrong shape and lists the ids it holds; its delete, once held, waits until released.
#[derive(Default)]
struct ListingVault {
    keys: Mutex<std::collections::HashMap<String, String>>,
    hold: AtomicBool,
    entered: Arc<tokio::sync::Notify>,
    released: (Mutex<bool>, std::sync::Condvar),
}

impl ListingVault {
    fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.keys.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }
    fn release(&self) {
        *self.released.0.lock().unwrap() = true;
        self.released.1.notify_all();
    }
}

impl SecretVault for ListingVault {
    fn load(&self, id: &str) -> Option<String> {
        self.keys.lock().unwrap().get(id).cloned()
    }
    fn save(&self, id: &str, key: &str) -> mighty_platform::Result<()> {
        if !mighty_remote::remote::validation::key(Some(key)) {
            return Err(mighty_platform::PlatformError::Refused("shape".into()));
        }
        self.keys.lock().unwrap().insert(id.into(), key.into());
        Ok(())
    }
    fn delete(&self, id: &str) -> mighty_platform::Result<()> {
        if self.hold.load(Ordering::SeqCst) {
            self.entered.notify_one();
            let guard = self.released.0.lock().unwrap();
            let _ = self.released.1.wait_timeout_while(guard, Duration::from_secs(10), |r| !*r).unwrap();
        }
        self.keys.lock().unwrap().remove(id);
        Ok(())
    }
}

/// "remote client: overlapping changes keep the newer state".
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn overlapping_changes_keep_the_newer_state() {
    set_language("ko");
    let seen = Arc::new(Collected::default());
    // (a) Two connects that both passed the first count with 15 saved: one is the 16th, the other is refused and keeps no key.
    {
        let (store, directory) = client_store();
        saved::write(&directory, &(0..15).map(|i| SavedConnection::new(&format!("saved-{i}"), "n", &format!("h{i}"))).collect::<Vec<_>>())
            .unwrap();
        let (first, second) = (InfoGate::closed(), InfoGate::closed());
        let one = FakeHost::with(first.answer()).await;
        let two = FakeHost::with(second.answer()).await;
        let vault = Arc::new(ListingVault::default());
        let client = client_with(&store, &directory, vault.clone(), &seen, loopback_options());
        let (c1, c2) = (client.clone(), client.clone());
        let (p1, p2) = (one.port, two.port);
        let a = tokio::spawn(async move { c1.connect("", &format!("127.0.0.1:{p1}"), KEY).await });
        let b = tokio::spawn(async move { c2.connect("", &format!("127.0.0.1:{p2}"), KEY).await });
        tokio::time::timeout(Duration::from_secs(10), first.arrived.notified()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), second.arrived.notified()).await.unwrap();
        first.release();
        second.release();
        let outcomes = [a.await.unwrap(), b.await.unwrap()];
        let connected: Vec<_> = outcomes.iter().filter_map(|o| o.as_ref().ok()).collect();
        let refused: Vec<_> = outcomes.iter().filter_map(|o| o.as_ref().err()).collect();
        let too_many = get_with("remote.connect.error.tooMany", &[("count", "16")]);
        assert!(connected.len() == 1 && refused.len() == 1 && refused[0].message() == too_many, "exactly one is the 16th");
        assert!(client.connections().len() == 16 && saved::read(&directory).len() == 16, "16 listed and 16 saved");
        assert_eq!(vault.ids(), [connected[0].id.clone()], "the refused connect left no key in the vault");
        let _ = std::fs::remove_dir_all(&directory);
    }
    // (b) A disconnect while a refresh waits on the host: the refresh is refused as changed and the disconnect stands.
    {
        let (store, directory) = client_store();
        let gate = InfoGate::default();
        let host = FakeHost::with(gate.answer()).await;
        let client = client_with(&store, &directory, Arc::new(ListingVault::default()), &seen, loopback_options());
        let info = client.connect("", &format!("127.0.0.1:{}", host.port), KEY).await.unwrap();
        gate.closed.store(true, Ordering::SeqCst);
        let (refreshing, id) = (client.clone(), info.id.clone());
        let refresh = tokio::spawn(async move { refreshing.refresh(&id).await });
        tokio::time::timeout(Duration::from_secs(10), gate.arrived.notified()).await.unwrap();
        client.disconnect(&info.id).await;
        gate.release();
        assert_eq!(
            refresh.await.unwrap().unwrap_err().message(),
            get("remote.connect.error.changed"),
            "a refresh that outlived a disconnect"
        );
        let now = client.connection(&info.id).unwrap();
        assert!(
            now.status == RemoteConnectionStatus::Disconnected && now.detail == Some(get("remote.connect.detail.disconnected")),
            "never failed over"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
    // (c) A remove while a new key's connect waits on the host: the connect lands after it and lists the connection again, with its key.
    {
        let (store, directory) = client_store();
        let gate = InfoGate::default();
        let vault = Arc::new(ListingVault::default());
        let newer = "m".repeat(43);
        let host = FakeHost::with(gate.answer()).await;
        let client = client_with(&store, &directory, vault.clone(), &seen, loopback_options());
        let address = format!("127.0.0.1:{}", host.port);
        let info = client.connect("", &address, KEY).await.unwrap();
        gate.closed.store(true, Ordering::SeqCst);
        let (connecting, at, key) = (client.clone(), address.clone(), newer.clone());
        let again = tokio::spawn(async move { connecting.connect("", &at, &key).await });
        tokio::time::timeout(Duration::from_secs(10), gate.arrived.notified()).await.unwrap();
        client.remove(&info.id).await;
        assert!(client.connections().is_empty() && vault.ids().is_empty(), "removed, key and all");
        gate.release();
        let back = again.await.unwrap().unwrap();
        assert!(back.id == info.id && client.connections()[0].id == info.id && saved::read(&directory)[0].id == info.id, "listed again");
        assert!(vault.load(&info.id) == Some(newer) && vault.ids() == [info.id.clone()], "with its new key in the vault");
        let _ = std::fs::remove_dir_all(&directory);
    }
    // (e) A remove holding its key delete while a connect for the same address comes back: the connect's save waits for the delete.
    {
        let (store, directory) = client_store();
        let gate = InfoGate::default();
        let vault = Arc::new(ListingVault::default());
        let newer = "n".repeat(43);
        let host = FakeHost::with(gate.answer()).await;
        let client = client_with(&store, &directory, vault.clone(), &seen, loopback_options());
        let address = format!("127.0.0.1:{}", host.port);
        let info = client.connect("", &address, KEY).await.unwrap();
        gate.closed.store(true, Ordering::SeqCst);
        let (connecting, at, key) = (client.clone(), address.clone(), newer.clone());
        let again = tokio::spawn(async move { connecting.connect("", &at, &key).await });
        tokio::time::timeout(Duration::from_secs(10), gate.arrived.notified()).await.unwrap();
        vault.hold.store(true, Ordering::SeqCst);
        let (removing, id) = (client.clone(), info.id.clone());
        let remove = tokio::spawn(async move { removing.remove(&id).await });
        tokio::time::timeout(Duration::from_secs(10), vault.entered.notified()).await.unwrap();
        gate.release();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!again.is_finished(), "the connect does not finish while the remove's delete is held");
        vault.release();
        remove.await.unwrap();
        let back = again.await.unwrap().unwrap();
        assert!(back.id == info.id && client.connections()[0].id == info.id && saved::read(&directory)[0].id == info.id, "listed again");
        assert!(vault.load(&info.id) == Some(newer) && vault.ids() == [info.id.clone()], "and its new key is in the vault");
        let _ = std::fs::remove_dir_all(&directory);
    }
    // (d) A disconnect while a send's route check runs: the route failure is refused as changed and the disconnect stands.
    {
        let (store, directory) = client_store();
        let host = FakeHost::new(|_| Reply::fixture("info.json", 200)).await;
        let hold = Arc::new(AtomicBool::new(false));
        let arrived = Arc::new(tokio::sync::Notify::new());
        let released = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let (holding, arriving, releasing) = (hold.clone(), arrived.clone(), released.clone());
        let routed = ClientOptions {
            route_check: Some(Arc::new(move |_| {
                if !holding.load(Ordering::SeqCst) {
                    return Ok(true);
                }
                arriving.notify_one();
                let guard = releasing.0.lock().unwrap();
                let _ = releasing.1.wait_timeout_while(guard, Duration::from_secs(10), |r| !*r).unwrap();
                Ok(false)
            })),
            ..loopback_options()
        };
        let client = client_with(&store, &directory, Arc::new(ListingVault::default()), &seen, routed);
        let info = client.connect("", &format!("127.0.0.1:{}", host.port), KEY).await.unwrap();
        hold.store(true, Ordering::SeqCst);
        let (sending, id) = (client.clone(), info.id.clone());
        let send = tokio::spawn(async move { sending.ensure_connected(&id).await });
        tokio::time::timeout(Duration::from_secs(10), arrived.notified()).await.unwrap();
        client.disconnect(&info.id).await;
        *released.0.lock().unwrap() = true;
        released.1.notify_all();
        assert_eq!(
            send.await.unwrap().unwrap_err().message(),
            get("remote.connect.error.changed"),
            "a route failure that outlived a disconnect"
        );
        assert_eq!(
            client.connection(&info.id).unwrap().detail,
            Some(get("remote.connect.detail.disconnected")),
            "the disconnect's words stand"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}

// Runs.

/// C#'s `ScriptedHost`: /v2/info, then whatever the check queues for job-1's polls; `hook` answers first when it gives a reply.
struct Scripted {
    polls: Arc<Mutex<VecDeque<Reply>>>,
    start: Arc<Mutex<Option<Reply>>>,
    hook: Arc<Mutex<Option<HookFn>>>,
    host: FakeHost,
}

type HookFn = Arc<dyn Fn(&Seen) -> Option<mighty_platform::BoxFuture<'static, Reply>> + Send + Sync>;

fn empty_poll(cursor: &str) -> Reply {
    Reply::new(200, &format!(r#"{{"protocol":2,"cursor":{cursor},"lastCursor":{cursor},"gap":false,"done":false,"events":[]}}"#))
}

impl Scripted {
    async fn new() -> Scripted {
        let polls: Arc<Mutex<VecDeque<Reply>>> = Arc::default();
        let start: Arc<Mutex<Option<Reply>>> = Arc::default();
        let hook: Arc<Mutex<Option<HookFn>>> = Arc::default();
        let (p, s, h) = (polls.clone(), start.clone(), hook.clone());
        let host = FakeHost::with(Arc::new(move |seen: &Seen| {
            let hooked = h.lock().unwrap().clone().and_then(|hook| hook(seen));
            if let Some(hooked) = hooked {
                return hooked;
            }
            let reply = match seen.target.as_str() {
                "/v2/info" => Reply::fixture("info.json", 200),
                "/v2/runs" => s.lock().unwrap().clone().unwrap_or_else(|| Reply::fixture("start-accepted.json", 202)),
                t if t.starts_with("/v2/runs/job-1/events") => {
                    let cursor = t[t.find("cursor=").unwrap() + 7..].to_string();
                    p.lock().unwrap().pop_front().unwrap_or_else(|| empty_poll(&cursor))
                }
                _ => Reply::fixture("answer.json", 200),
            };
            Box::pin(async move { reply })
        }))
        .await;
        Scripted { polls, start, hook, host }
    }

    fn queue(&self, reply: Reply) {
        self.polls.lock().unwrap().push_back(reply);
    }

    fn seen(&self, prefix: &str) -> Vec<Seen> {
        self.host.seen().into_iter().filter(|r| r.target.starts_with(prefix)).collect()
    }

    fn poll_count(&self) -> usize {
        self.seen("/v2/runs/job-1/events").len()
    }

    fn posts(&self) -> usize {
        self.host.seen().iter().filter(|r| r.target == "/v2/runs").count()
    }
}

struct Connected {
    client: RemoteClient,
    workspace: Workspace,
    seen: Arc<Collected>,
    directory: std::path::PathBuf,
}

impl Connected {
    fn connection(&self) -> String {
        self.workspace.remote.as_ref().unwrap().connection_id.clone().unwrap()
    }
    async fn close(self) {
        self.client.dispose().await;
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

async fn connected(scripted: &Scripted, options: Option<ClientOptions>) -> Connected {
    let (store, directory) = client_store();
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, options.unwrap_or_else(loopback_options));
    let info = client.connect("", &format!("127.0.0.1:{}", scripted.host.port), KEY).await.unwrap();
    let workspace = client.open_workspace(&info.id, "ws-project").unwrap();
    Connected { client, workspace, seen, directory }
}

fn poll(cursor: i64, done: bool, events: &[String]) -> String {
    format!(r#"{{"protocol":2,"cursor":{cursor},"lastCursor":{cursor},"gap":false,"done":{done},"events":[{}]}}"#, events.join(","))
}

fn log_event(cursor: i64, text: &str) -> String {
    format!(
        r#"{{"cursor":{cursor},"event":{{"sessionId":"job-1","type":"log","entry":{{"id":"log-{cursor}","kind":"output","text":"{text}","timestamp":"2026-10-17T01:04:00.000Z"}}}}}}"#
    )
}

fn lost(detail: &str) -> String {
    get_with("remote.run.error.connectionLost", &[("detail", detail)])
}

fn shell(pane: &str, workspace: &Workspace) -> StartRunRequest {
    StartRunRequest::new(pane, workspace.id(), "shell", "dir")
}

/// "remote client: a run posts the Mac's request and applies the host's events as the pane's".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_posts_the_macs_request_and_applies_the_hosts_events_as_the_panes() {
    set_language("ko");
    let scripted = Scripted::new().await;
    scripted.queue(Reply::fixture("poll.json", 200));
    let c = connected(&scripted, None).await;
    let start = RemoteWireStart::decode(&serde_json::to_vec(&fixture("start.json")).unwrap()).unwrap();
    c.client.start(start.to_request("pane-1", c.workspace.id()), &c.workspace).await.unwrap();
    until(|| c.seen.events().iter().any(|e| e.kind == "status" && e.status.as_deref() == Some("completed")), 10_000, "completed").await;
    let posted = scripted.seen("/v2/runs").into_iter().find(|r| r.method == "POST").unwrap();
    assert!(same(&serde_json::from_slice(&posted.body).unwrap(), &fixture("start.json")), "the body is the Mac's start sample");
    assert_eq!(posted.headers.get("x-mighty-remote-graph").map(String::as_str), Some("run"), "a Windows client asks for graph runs");
    let kinds: Vec<String> = c.seen.events().iter().map(|e| e.kind.clone()).collect();
    assert!(c.seen.events().iter().all(|e| e.session_id == "pane-1"));
    assert_eq!(
        kinds,
        ["status", "resume", "log", "activity", "usage", "plan", "todos", "background", "status"],
        "the Mac graph node left out"
    );
    let permissions = c.seen.permissions();
    assert!(
        permissions.len() == 1
            && permissions[0].run_id == "pane-1"
            && permissions[0].id == "perm-1"
            && permissions[0].input_json == "{\"command\":\"ls\"}"
    );
    until(|| !c.client.is_running("pane-1"), 5000, "the run ends right after its last status").await;
    c.close().await;
}

/// "remote client: answers go to the host run and a stop reaches the host".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answers_go_to_the_host_run_and_a_stop_reaches_the_host() {
    set_language("ko");
    let scripted = Scripted::new().await;
    let permission = fixture("poll.json")["events"][5].clone();
    scripted
        .queue(Reply::new(200, &format!(r#"{{"protocol":2,"cursor":6,"lastCursor":6,"gap":false,"done":false,"events":[{permission}]}}"#)));
    let c = connected(&scripted, None).await;
    c.client.start(StartRunRequest::new("pane-1", c.workspace.id(), "claude", "hi"), &c.workspace).await.unwrap();
    until(|| c.seen.permissions().len() == 1, 10_000, "the request arrives").await;
    c.client.respond_to_permission("pane-1", "perm-1", true).await.unwrap();
    let answer = scripted.seen("/v2/runs/job-1/permission");
    assert!(answer.len() == 1 && same(&serde_json::from_slice(&answer[0].body).unwrap(), &fixture("permission.json")), "the Mac's sample");
    c.client.answer_plan("pane-1", "perm-3", &PlanDecision::revise("Keep the tests")).await.unwrap();
    assert!(same(&serde_json::from_slice(&scripted.seen("/v2/runs/job-1/plan")[0].body).unwrap(), &fixture("plan-revised.json")));
    c.client.stop("pane-1").await;
    let stop = scripted.seen("/v2/runs/job-1/stop");
    assert!(stop.len() == 1 && stop[0].body == b"{}", "a stop reaches the host, with a body");
    let last = c.seen.events().last().cloned().unwrap();
    assert!(last.kind == "status" && last.status.as_deref() == Some("stopped") && !c.client.is_running("pane-1"), "a stop ends the pane");
    c.close().await;
}

/// "remote client: one unusable event is skipped and a gap is said".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_unusable_event_is_skipped_and_a_gap_is_said() {
    set_language("ko");
    let scripted = Scripted::new().await;
    scripted.queue(Reply::fixture("poll-malformed.json", 200));
    let c = connected(&scripted, None).await;
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    until(|| !c.client.is_running("pane-1"), 10_000, "pane-1 ends").await;
    let skipped = get("remote.run.notice.skipped");
    let words = c.seen.words(None);
    assert!(
        words.iter().filter(|w| **w == skipped).count() == 1
            && words.contains(&"ok".to_string())
            && words.contains(&"completed".to_string())
    );
    assert_eq!(
        c.client.connection(&c.connection()).unwrap().status,
        RemoteConnectionStatus::Connected,
        "a skipped event keeps the connection"
    );
    scripted.queue(Reply::fixture("poll-gap.json", 200));
    c.client.start(StartRunRequest::new("pane-2", c.workspace.id(), "claude", "hi"), &c.workspace).await.unwrap();
    until(|| !c.client.is_running("pane-2"), 10_000, "pane-2 ends").await;
    let events = c.seen.events();
    let pane2: Vec<_> = events.iter().filter(|e| e.session_id == "pane-2").collect();
    assert!(
        pane2.iter().any(|e| e.entry.as_ref().and_then(|l| l.text.as_deref()) == Some(get("remote.run.notice.gap").as_str())),
        "a gap is said"
    );
    let last = pane2.last().unwrap();
    assert!(
        last.status.as_deref() == Some("error") && last.reason.as_deref() == Some("authentication"),
        "a lost sign-in reads as Windows' own"
    );
    c.close().await;
}

/// "remote client: a busy host is asked again for a while, then the connection is lost".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_busy_host_is_asked_again_for_ten_seconds_and_then_the_connection_is_lost() {
    set_language("ko");
    let scripted = Scripted::new().await;
    for _ in 0..2 {
        scripted.queue(Reply::new(503, r#"{"protocol":2,"error":"busy"}"#));
    }
    scripted.queue(Reply::fixture("poll-malformed.json", 200));
    let c = connected(&scripted, Some(ClientOptions { poll_retry_window: Duration::from_secs(5), ..loopback_options() })).await;
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    until(|| c.seen.events().iter().any(|e| e.status.as_deref() == Some("completed")), 10_000, "completed").await;
    assert!(!c.seen.events().iter().any(|e| e.status.as_deref() == Some("error")), "two busy answers are ridden out");
    c.close().await;
    let down = Scripted::new().await;
    for _ in 0..50 {
        down.queue(Reply::new(503, r#"{"protocol":2,"error":"busy"}"#));
    }
    let c = connected(&down, Some(ClientOptions { poll_retry_window: Duration::from_millis(1500), ..loopback_options() })).await;
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    until(|| c.seen.events().iter().any(|e| e.status.as_deref() == Some("error")), 10_000, "error").await;
    assert!(
        c.seen
            .events()
            .iter()
            .any(|e| e.entry.as_ref().is_some_and(|l| l.kind() == "error" && l.text.as_deref() == Some(lost("busy").as_str())))
    );
    assert_eq!(c.client.connection(&c.connection()).unwrap().status, RemoteConnectionStatus::Disconnected, "past the window it is lost");
    c.close().await;
}

/// "remote client: unsupported runs and a refused key never run".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_runs_and_a_refused_key_never_run() {
    set_language("ko");
    let scripted = Scripted::new().await;
    let c = connected(&scripted, None).await;
    let settings = mighty_core::model::RunSettings { permission_mode: "acceptEdits".into(), ..Default::default() };
    let codex = StartRunRequest::new("pane-1", c.workspace.id(), "claude", "hi").with_settings(settings).with_provider("codex");
    let missing = get_with("remote.host.error.cliMissing", &[("name", mighty_run::providers::name("codex"))]);
    assert_eq!(c.client.start(codex, &c.workspace).await.unwrap_err().message(), missing, "the host's missing Codex is refused locally");
    assert_eq!(scripted.posts(), 0, "nothing was posted");
    *scripted.start.lock().unwrap() = Some(Reply::fixture("error.json", 401));
    assert_eq!(c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap_err().status(), Some(401));
    assert!(c.client.connection(&c.connection()).unwrap().status == RemoteConnectionStatus::Disconnected && !c.client.is_running("pane-1"));
    assert_eq!(
        c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap_err().message(),
        get("remote.run.error.notConnected")
    );
    c.close().await;
}

/// "remote client: a malformed event is dropped on its own".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_malformed_event_is_dropped_on_its_own() {
    set_language("ko");
    let scripted = Scripted::new().await;
    scripted.queue(Reply::new(
        200,
        &poll(
            5,
            true,
            &[
                log_event(1, "ok"),
                r#"{"cursor":2,"event":"not an event"}"#.into(),
                r#"{"cursor":3,"event":{"sessionId":"job-1","type":"log","type":"status","status":"error"}}"#.into(),
                r#""not an item""#.into(),
                "null".into(),
                r#"{"event":{"sessionId":"job-1","type":"status","status":"error"}}"#.into(),
                r#"{"cursor":5,"event":{"sessionId":"job-1","type":"status","status":"completed"}}"#.into(),
            ],
        ),
    ));
    let c = connected(&scripted, None).await;
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    until(|| !c.client.is_running("pane-1"), 10_000, "pane-1 ends").await;
    assert_eq!(c.seen.words(None), ["ok".to_string(), "completed".into(), get("remote.run.notice.skipped")], "the bad ones are said once");
    assert_eq!(c.client.connection(&c.connection()).unwrap().status, RemoteConnectionStatus::Connected, "and the connection stays");
    scripted.queue(Reply::new(200, &poll(1, true, &[log_event(1, "partial")])));
    c.client.start(shell("pane-2", &c.workspace), &c.workspace).await.unwrap();
    until(|| !c.client.is_running("pane-2"), 10_000, "pane-2 ends").await;
    assert_eq!(c.seen.words(Some("pane-2")), ["partial", "error"], "a done poll without a last status ends the pane in error");
    c.close().await;
}

/// C#'s `HeldPoll`: holds job-1's next poll at the host until released, and says when it arrived.
#[derive(Clone, Default)]
struct HeldPoll {
    arrived: Arc<AtomicBool>,
    released: Arc<Mutex<Option<tokio::sync::oneshot::Sender<Reply>>>>,
}

impl HeldPoll {
    fn hook(&self) -> HookFn {
        let held = self.clone();
        Arc::new(move |seen: &Seen| {
            if !seen.target.starts_with("/v2/runs/job-1/events") || held.arrived.swap(true, Ordering::SeqCst) {
                return None;
            }
            let (send, receive) = tokio::sync::oneshot::channel();
            *held.released.lock().unwrap() = Some(send);
            Some(Box::pin(async move { receive.await.unwrap_or_else(|_| empty_poll("0")) }))
        })
    }
    fn release(&self, reply: Reply) {
        if let Some(send) = self.released.lock().unwrap().take() {
            let _ = send.send(reply);
        }
    }
}

/// "remote client: a route outside the tunnel ends the connection, and a stale run never fails a newer one".
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn a_route_outside_the_tunnel_ends_the_connection_and_a_stale_run_never_fails_a_newer_one() {
    set_language("ko");
    let not_via = get("remote.connect.error.notViaTailscale");
    let scripted = Scripted::new().await;
    let tunnel = Arc::new(AtomicBool::new(true));
    let block = Arc::new(AtomicBool::new(false));
    let arrived = Arc::new(AtomicBool::new(false));
    let released = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let (t, b, a, r) = (tunnel.clone(), block.clone(), arrived.clone(), released.clone());
    let options = ClientOptions {
        route_check: Some(Arc::new(move |_| {
            if b.swap(false, Ordering::SeqCst) {
                a.store(true, Ordering::SeqCst);
                let guard = r.0.lock().unwrap();
                let _ = r.1.wait_timeout_while(guard, Duration::from_secs(10), |released| !*released).unwrap();
                return Ok(false);
            }
            Ok(t.load(Ordering::SeqCst))
        })),
        ..loopback_options()
    };
    let c = connected(&scripted, Some(options)).await;
    let id = c.connection();
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    until(|| scripted.poll_count() > 0, 10_000, "a poll").await;
    tunnel.store(false, Ordering::SeqCst);
    until(|| !c.client.is_running("pane-1"), 10_000, "pane-1 ends").await;
    let now = c.client.connection(&id).unwrap();
    assert!(
        now.status == RemoteConnectionStatus::Disconnected && now.detail.as_deref() == Some(not_via.as_str()),
        "the poll's route check lost it"
    );
    let says_why =
        || c.seen.words(Some("pane-1")).iter().rev().take(2).rev().cloned().collect::<Vec<_>>() == [lost(&not_via), "error".to_string()];
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while !says_why() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(says_why(), "and the pane says why");
    // A start: nothing is posted.
    tunnel.store(true, Ordering::SeqCst);
    c.client.refresh(&id).await.unwrap();
    tunnel.store(false, Ordering::SeqCst);
    let posts = scripted.posts();
    assert_eq!(
        c.client.start(shell("pane-2", &c.workspace), &c.workspace).await.unwrap_err().message(),
        not_via,
        "a start outside the tunnel"
    );
    assert!(
        scripted.posts() == posts
            && c.client.connection(&id).unwrap().status == RemoteConnectionStatus::Disconnected
            && !c.client.is_running("pane-2")
    );
    // An answer and a stop, while the run's poll waits on the host: neither is sent.
    for action in ["permission", "stop"] {
        tunnel.store(true, Ordering::SeqCst);
        c.client.refresh(&id).await.unwrap();
        let held = HeldPoll::default();
        *scripted.hook.lock().unwrap() = Some(held.hook());
        c.client.start(shell("pane-3", &c.workspace), &c.workspace).await.unwrap();
        until(|| held.arrived.load(Ordering::SeqCst), 10_000, "the poll arrives").await;
        tunnel.store(false, Ordering::SeqCst);
        if action == "permission" {
            assert_eq!(
                c.client.respond_to_permission("pane-3", "perm-1", true).await.unwrap_err().message(),
                not_via,
                "an answer outside the tunnel"
            );
        } else {
            c.client.stop("pane-3").await;
        }
        held.release(empty_poll("0"));
        *scripted.hook.lock().unwrap() = None;
        assert!(
            scripted.seen(&format!("/v2/runs/job-1/{action}")).is_empty()
                && c.client.connection(&id).unwrap().detail.as_deref() == Some(not_via.as_str())
                && !c.client.is_running("pane-3"),
            "the {action} was not sent and the connection is lost"
        );
        c.client.follows_ended().await;
    }
    // A stale run: its route check fails after its connection was disconnected and connected again. The newer connection stands.
    tunnel.store(true, Ordering::SeqCst);
    c.client.refresh(&id).await.unwrap();
    c.client.start(shell("pane-4", &c.workspace), &c.workspace).await.unwrap();
    let polls = scripted.poll_count();
    until(|| scripted.poll_count() > polls && c.client.is_running("pane-4"), 10_000, "pane-4 polls").await;
    block.store(true, Ordering::SeqCst);
    until(|| arrived.load(Ordering::SeqCst), 10_000, "the run's route check was asked").await;
    c.client.disconnect(&id).await;
    c.client.refresh(&id).await.unwrap();
    *released.0.lock().unwrap() = true;
    released.1.notify_all();
    tokio::time::timeout(Duration::from_secs(10), c.client.follows_ended()).await.unwrap();
    assert_eq!(
        c.client.connection(&id).unwrap().status,
        RemoteConnectionStatus::Connected,
        "the stale run left the newer connection alone"
    );
    c.close().await;
}

/// "remote client: only a passing failure is asked again".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_passing_failure_is_asked_again() {
    set_language("ko");
    let scripted = Scripted::new().await;
    scripted.queue(Reply::new(500, r#"{"protocol":2,"error":"broken"}"#));
    scripted.queue(Reply::fixture("poll-malformed.json", 200));
    let c = connected(&scripted, None).await;
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    until(|| c.seen.events().iter().any(|e| matches!(e.status.as_deref(), Some("error" | "completed"))), 10_000, "an end").await;
    let words = c.seen.words(None);
    assert!(
        words.contains(&lost("broken")) && words.last().map(String::as_str) == Some("error") && scripted.poll_count() == 1,
        "not asked again"
    );
    assert_eq!(c.client.connection(&c.connection()).unwrap().status, RemoteConnectionStatus::Disconnected, "and is shown lost");
    c.close().await;
}

/// "remote client: a lost connection ends its runs with its reason cut".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_connection_ends_its_runs_with_its_reason_cut() {
    set_language("ko");
    let emoji = "\u{1F600}";
    let error = format!("{}{emoji}{}", "a".repeat(299), "b".repeat(20));
    let expected = format!("{}{emoji}", "a".repeat(299));
    let scripted = Scripted::new().await;
    let fail = Arc::new(AtomicBool::new(false));
    let (failing, long) = (fail.clone(), error.clone());
    let options = ClientOptions {
        local_addresses: Some(Arc::new(move || {
            if failing.load(Ordering::SeqCst) { Err(long.clone()) } else { Ok(vec!["100.101.102.103".into()]) }
        })),
        ..loopback_options()
    };
    let c = connected(&scripted, Some(options)).await;
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    c.client.start(shell("pane-2", &c.workspace), &c.workspace).await.unwrap();
    fail.store(true, Ordering::SeqCst);
    assert_eq!(c.client.refresh(&c.connection()).await.unwrap_err().message(), error, "a refresh that fails with a long message");
    for pane in ["pane-1", "pane-2"] {
        let words = c.seen.words(Some(pane));
        assert!(
            words[words.len() - 2..] == [lost(&expected), "error".to_string()] && !c.client.is_running(pane),
            "{pane} ended with the reason cut"
        );
    }
    until(|| scripted.seen("/v2/runs/job-1/stop").len() == 2, 10_000, "both stops reach the host").await;
    c.close().await;
}

/// "remote client: closing the app stops its runs on the host".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_the_app_stops_its_runs_on_the_host() {
    set_language("ko");
    let scripted = Scripted::new().await;
    let c = connected(&scripted, None).await;
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    c.client.dispose().await;
    let last = c.seen.events().last().cloned().unwrap();
    assert!(
        scripted.seen("/v2/runs/job-1/stop").len() == 1
            && last.session_id == "pane-1"
            && last.status.as_deref() == Some("stopped")
            && !c.client.is_running("pane-1")
    );
    assert_eq!(c.client.start(shell("pane-2", &c.workspace), &c.workspace).await.unwrap_err().message(), get("common.appClosing"));
    let _ = std::fs::remove_dir_all(&c.directory);
}

/// "remote client: a remote pane keeps the newest requests and answers with the host's run id".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_pane_keeps_the_newest_requests_and_answers_with_the_hosts_run_id() {
    set_language("ko");
    let scripted = Scripted::new().await;
    let requests: Vec<String> = (0..40)
        .map(|i| {
            format!(
                r#"{{"cursor":{},"event":{{"sessionId":"job-1","type":"permission","permission":{{"id":"p{i}","runId":"host-{i}","toolUseId":"t{i}","toolName":"Bash","inputJSON":"{{}}","summary":"ls","state":"pending","canAllow":true,"canAnswerQuestions":false,"canAnswerPlan":false}}}}}}"#,
                i + 1
            )
        })
        .collect();
    scripted.queue(Reply::new(200, &poll(40, false, &requests)));
    let c = connected(&scripted, None).await;
    c.client.start(StartRunRequest::new("pane-1", c.workspace.id(), "claude", "hi"), &c.workspace).await.unwrap();
    until(|| c.seen.permissions().iter().filter(|p| p.state == "pending").count() == 40, 10_000, "40 requests").await;
    let withdrawn: Vec<String> = c.seen.permissions().iter().filter(|p| p.state == "cancelled").map(|p| p.id.clone()).collect();
    assert_eq!(withdrawn, (0..8).map(|i| format!("p{i}")).collect::<Vec<_>>(), "the newest 32 wait and the 8 older ones are withdrawn");
    assert!(c.seen.permissions().iter().all(|p| p.run_id == "pane-1"));
    c.client.respond_to_permission("pane-1", "p39", false).await.unwrap();
    let body: Value = serde_json::from_slice(&scripted.seen("/v2/runs/job-1/permission")[0].body).unwrap();
    assert_eq!(body["runId"], "host-39", "an answer carries the host's run id of that request");
    c.close().await;
}

/// "remote client: a stop waits for the poll being applied and is the pane's last word".
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn a_stop_waits_for_the_poll_being_applied_and_is_the_panes_last_word() {
    set_language("ko");
    let scripted = Scripted::new().await;
    scripted.queue(Reply::new(200, &poll(3, false, &[log_event(1, "first"), log_event(2, "second"), log_event(3, "third")])));
    let c = connected(&scripted, None).await;
    *c.seen.hold.lock().unwrap() = Some(Arc::new(|e| e.entry.as_ref().and_then(|l| l.text.as_deref()) == Some("first")));
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    until(|| c.seen.held.load(Ordering::SeqCst), 10_000, "held").await;
    let stopping = c.client.clone();
    let stop = tokio::spawn(async move { stopping.stop("pane-1").await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!stop.is_finished(), "the stop waits while the poll is being applied");
    c.seen.release();
    tokio::time::timeout(Duration::from_secs(10), stop).await.unwrap().unwrap();
    assert_eq!(c.seen.words(None), ["first", "second", "third", "stopped"], "the poll's events, then the stop");
    c.close().await;
}

/// "remote client: no run starts or outlives its connection while its stops are on their way".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_run_starts_or_outlives_its_connection_while_its_stops_are_on_their_way() {
    set_language("ko");
    for leave in ["disconnect", "remove", "replace"] {
        let scripted = Scripted::new().await;
        let arrived = Arc::new(AtomicBool::new(false));
        let release: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>> = Arc::default();
        let (seen_stop, held) = (arrived.clone(), release.clone());
        *scripted.hook.lock().unwrap() = Some(Arc::new(move |seen: &Seen| {
            if seen.target != "/v2/runs/job-1/stop" || seen_stop.swap(true, Ordering::SeqCst) {
                return None;
            }
            let (send, receive) = tokio::sync::oneshot::channel();
            *held.lock().unwrap() = Some(send);
            Some(Box::pin(async move {
                let _ = receive.await;
                Reply::fixture("answer.json", 200)
            }))
        }));
        let c = connected(&scripted, None).await;
        let id = c.connection();
        c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
        let leaving = match leave {
            "disconnect" => c.client.disconnect(&id),
            "remove" => c.client.remove(&id),
            _ => {
                let (client, address) = (c.client.clone(), format!("127.0.0.1:{}", scripted.host.port));
                Box::pin(async move {
                    client.connect("", &address, &"n".repeat(43)).await.unwrap();
                })
            }
        };
        let leaving = tokio::spawn(leaving);
        until(|| arrived.load(Ordering::SeqCst), 10_000, "the stop arrives").await;
        assert_eq!(
            c.client.start(shell("pane-2", &c.workspace), &c.workspace).await.unwrap_err().message(),
            get("remote.run.error.notConnected"),
            "{leave}"
        );
        if let Some(send) = release.lock().unwrap().take() {
            let _ = send.send(());
        }
        leaving.await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), c.client.follows_ended()).await.unwrap();
        let polls = scripted.poll_count();
        assert!(
            scripted.posts() == 1 && !c.client.is_running("pane-1") && !c.client.is_running("pane-2"),
            "{leave}: only the first run was posted"
        );
        let last = c.seen.events().into_iter().rfind(|e| e.session_id == "pane-1").unwrap();
        assert!(
            last.status.as_deref() == Some("stopped") && !c.seen.events().iter().any(|e| e.session_id == "pane-2"),
            "{leave}: stopped, never began"
        );
        let state = c.client.connection(&id);
        match leave {
            "replace" => assert_eq!(state.unwrap().status, RemoteConnectionStatus::Connected),
            "remove" => assert!(state.is_none()),
            _ => assert_eq!(state.unwrap().status, RemoteConnectionStatus::Disconnected),
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(scripted.poll_count(), polls, "{leave}: nothing polls afterwards");
        c.close().await;
    }
}

/// "remote client: a callback that throws ends the run".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_callback_that_throws_ends_the_run() {
    set_language("ko");
    let scripted = Scripted::new().await;
    scripted.queue(Reply::new(200, &poll(2, false, &[log_event(1, "boom"), log_event(2, "after")])));
    let c = connected(&scripted, None).await;
    *c.seen.after.lock().unwrap() = Some(Arc::new(|e| {
        if e.entry.as_ref().and_then(|l| l.text.as_deref()) == Some("boom") {
            panic!("the pane broke");
        }
    }));
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    until(|| !c.client.is_running("pane-1"), 10_000, "pane-1 ends").await;
    tokio::time::timeout(Duration::from_secs(10), c.client.follows_ended()).await.unwrap();
    assert_eq!(c.seen.words(None), ["boom".to_string(), lost("the pane broke"), "error".into()], "the run ended at the failing callback");
    assert_eq!(c.client.connection(&c.connection()).unwrap().status, RemoteConnectionStatus::Connected, "and the connection stays");
    c.close().await;
}

/// "remote client: a disconnect from inside an event ends the poll there".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_disconnect_from_inside_an_event_ends_the_poll_there() {
    set_language("ko");
    let scripted = Scripted::new().await;
    scripted.queue(Reply::new(200, &poll(3, false, &[log_event(1, "first"), log_event(2, "second"), log_event(3, "third")])));
    let c = connected(&scripted, None).await;
    let disconnect: Arc<Mutex<Option<mighty_platform::BoxFuture<'static, ()>>>> = Arc::default();
    let (client, id, slot) = (c.client.clone(), c.connection(), disconnect.clone());
    *c.seen.after.lock().unwrap() = Some(Arc::new(move |e| {
        if e.entry.as_ref().and_then(|l| l.text.as_deref()) == Some("first") {
            *slot.lock().unwrap() = Some(client.disconnect(&id));
        }
    }));
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    until(|| disconnect.lock().unwrap().is_some() && !c.client.is_running("pane-1"), 10_000, "the disconnect").await;
    let pending = disconnect.lock().unwrap().take().unwrap();
    pending.await;
    tokio::time::timeout(Duration::from_secs(10), c.client.follows_ended()).await.unwrap();
    assert_eq!(c.seen.words(None), ["first", "stopped"], "the run ended at the disconnect");
    c.close().await;
}

/// "remote client: a disconnect while a poll is in flight ends the pane stopped".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_disconnect_while_a_poll_is_in_flight_ends_the_pane_stopped() {
    set_language("ko");
    let scripted = Scripted::new().await;
    let c = connected(&scripted, None).await;
    let held = HeldPoll::default();
    *scripted.hook.lock().unwrap() = Some(held.hook());
    c.client.start(shell("pane-1", &c.workspace), &c.workspace).await.unwrap();
    until(|| held.arrived.load(Ordering::SeqCst), 10_000, "the poll arrives").await;
    tokio::time::timeout(Duration::from_secs(10), c.client.disconnect(&c.connection())).await.unwrap();
    held.release(Reply::new(200, &poll(1, false, &[log_event(1, "late")])));
    tokio::time::timeout(Duration::from_secs(10), c.client.follows_ended()).await.unwrap();
    assert_eq!(c.seen.words(Some("pane-1")), ["stopped"], "the pane's only last word is stopped");
    assert!(!c.client.is_running("pane-1") && c.client.connection(&c.connection()).unwrap().status == RemoteConnectionStatus::Disconnected);
    *scripted.hook.lock().unwrap() = None;
    c.close().await;
}

/// "remote client: a pane that throws during a remove leaves the others their last word and the key goes".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pane_that_throws_during_a_remove_leaves_the_others_their_last_word_and_the_key_goes() {
    set_language("ko");
    let scripted = Scripted::new().await;
    let (store, directory) = client_store();
    let seen = Arc::new(Collected::default());
    let vault = Arc::new(MemoryVault::default());
    let client = client_with(&store, &directory, vault.clone(), &seen, loopback_options());
    let info = client.connect("", &format!("127.0.0.1:{}", scripted.host.port), KEY).await.unwrap();
    let workspace = client.open_workspace(&info.id, "ws-project").unwrap();
    client.start(shell("pane-1", &workspace), &workspace).await.unwrap();
    client.start(shell("pane-2", &workspace), &workspace).await.unwrap();
    assert_eq!(vault.load(&info.id).as_deref(), Some(KEY), "the key is in the vault before the remove");
    let thrown = Arc::new(AtomicBool::new(false));
    let once = thrown.clone();
    *seen.after.lock().unwrap() = Some(Arc::new(move |e| {
        if e.status.as_deref() == Some("stopped") && !once.swap(true, Ordering::SeqCst) {
            panic!("the pane broke");
        }
    }));
    client.remove(&info.id).await;
    tokio::time::timeout(Duration::from_secs(10), client.follows_ended()).await.unwrap();
    assert!(thrown.load(Ordering::SeqCst), "a pane's callback threw");
    for pane in ["pane-1", "pane-2"] {
        let last = seen.events().into_iter().rfind(|e| e.session_id == pane);
        assert!(last.is_some_and(|e| e.status.as_deref() == Some("stopped")) && !client.is_running(pane), "{pane} got its last word");
    }
    assert!(
        client.connection(&info.id).is_none() && client.connections().is_empty() && vault.load(&info.id).is_none(),
        "the connection and its key are gone"
    );
    client.dispose().await;
    let _ = std::fs::remove_dir_all(&directory);
}
