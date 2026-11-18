//! Parity with C# itself (W8-A): the push message's hand-written JSON and the planner's summary over a corpus of strings (scalars, every
//! C0 control, `/`, DEL, U+2028, non-BMP, the 2048-byte rule), and the pairing URL of identities written by this crate's codec (names
//! with controls and non-ASCII, direct endpoints, damaged files that both refuse), each run through MightyCode.Core by `tools/parity`
//! (`push`, `pairing-url`; Phone.cs) and through this crate, compared line by line. Without the .NET 10 SDK the check says so and passes;
//! `MIGHTY_PARITY=require` makes it fail instead.

#[path = "../../../tools/parity/build_lock.rs"]
mod build_lock;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use mighty_platform::pairing::identity_file;
use mighty_platform::{PairedDevice, PairingIdentity, PairingStore, PushRegistration};
use mighty_remote::identity::Identity;
use mighty_remote::push::message::{PushMessage, PushPermission, PushRequest};
use mighty_remote::push::planner::{Planner, PushPane};

#[allow(clippy::disallowed_methods, reason = "the check's own .NET probe, no child of the app")]
fn dotnet_10() -> bool {
    let listed = Command::new("dotnet").arg("--list-sdks").output();
    let found = matches!(&listed, Ok(output) if output.status.success() && String::from_utf8_lossy(&output.stdout).lines().any(|line| line.starts_with("10.")));
    if !found {
        let why = "no .NET 10 SDK (dotnet --list-sdks)";
        assert!(std::env::var("MIGHTY_PARITY").as_deref() != Ok("require"), "{why}");
        eprintln!("skipped: {why}");
    }
    found
}

/// Runs `parity <mode> <input> <output>` from tools/parity (built once per run of this binary).
#[allow(clippy::disallowed_methods, reason = "the check's own .NET build and run, no child of the app")]
fn csharp(mode: &str, input: &Path, output: &Path) -> bool {
    let tauri = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = tauri.join("target/parity-cs-remote");
    if !dotnet_10() {
        return false;
    }
    static BUILT: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    let mut built = BUILT.lock().unwrap_or_else(|p| p.into_inner());
    if !*built {
        let build = build_lock::hold(&tauri.join("target"));
        let status = Command::new("dotnet")
            .arg("build")
            .arg(tauri.join("tools/parity/Parity.csproj"))
            .args(["-c", "Release", "-o"])
            .arg(&out)
            .args(["--nologo", "-v", "quiet"])
            .status();
        drop(build);
        assert!(matches!(status, Ok(status) if status.success()), "the C# parity tool did not build ({status:?})");
        *built = true;
    }
    let ran = Command::new("dotnet").arg(out.join("parity.dll")).arg(mode).arg(input).arg(output).status().unwrap();
    assert!(ran.success(), "the C# parity tool failed ({mode})");
    true
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

fn text(hex_text: &str) -> String {
    String::from_utf8(unhex(hex_text)).unwrap()
}

fn work(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("mighty-remote-parity-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// The strings the push corpus is made of.
fn corpus() -> Vec<String> {
    let mut all: Vec<String> = vec![
        String::new(),
        "plain".into(),
        "a/b\\c\"d".into(),
        "\u{7f}\u{2028}\u{2029}\u{85}".into(),
        "\u{1F600}\u{1F468}\u{200D}\u{1F469}가나다<&>'+`".into(),
        "line\nnext\r\nthird\rfourth".into(),
        " \t lead and trail \u{3000}\u{a0} ".into(),
        "\u{2003}\u{2003}x\u{2003}".into(),
        "\u{1F600}".repeat(300),
        "x".repeat(2100),
        "가".repeat(700),
    ];
    all.push((0u32..0x20).filter_map(char::from_u32).collect());
    all
}

/// What this crate answers for one push vector (Phone.cs's `push`).
fn push_answer(line: &str) -> String {
    let part: Vec<&str> = line.split('\t').collect();
    match part[0] {
        "encode" => {
            let permission = (part[9] != "-").then(|| {
                let p: Vec<&str> = part[9].split(',').collect();
                PushPermission { request_id: text(p[0]), run_id: text(p[1]), buttons: text(p[2]) }
            });
            let message = PushMessage {
                v: 1,
                id: text(part[1]),
                kind: text(part[2]),
                host_id: text(part[3]),
                host_name: text(part[4]),
                workspace: text(part[5]),
                title: text(part[6]),
                summary: text(part[7]),
                session_id: text(part[8]),
                permission,
                at: text(part[10]),
            };
            message.encoded_for_push().map(|bytes| hex(&bytes)).unwrap_or_else(|| "null".into())
        }
        "summary" => {
            let mut planner = Planner::new();
            let pane = PushPane::new("p", "w", "t", "running", None, vec![]);
            planner.observe(std::slice::from_ref(&pane), "h", "n", "a");
            let asking = PushPane { requests: vec![PushRequest::new("r", "u", "permission", &text(part[1]), "claude")], ..pane };
            hex(planner.observe(&[asking], "h", "n", "a")[0].summary.as_bytes())
        }
        other => panic!("unknown push vector {other}"),
    }
}

#[test]
fn push_messages_and_summaries_are_csharps_bytes() {
    let strings = corpus();
    let mut vectors = vec![];
    for (at, value) in strings.iter().enumerate() {
        let h = hex(value.as_bytes());
        let other = hex(strings[(at + 3) % strings.len()].as_bytes());
        let permission = if at % 2 == 0 { format!("{},{},{}", hex(b"req-1"), h, hex(b"claude")) } else { "-".into() };
        vectors.push(format!(
            "encode\t{h}\t{}\t{other}\t{h}\t{other}\t{h}\t{h}\t{}\t{permission}\t{}",
            hex(b"permission"),
            hex(b"pane-1"),
            hex(b"2026-10-23T01:02:03Z")
        ));
        vectors.push(format!(
            "encode\t{}\t{}\t{}\t{}\t{}\t{}\t{h}\t{}\t-\t{}",
            hex(b"i"),
            hex(b"completed"),
            hex(b"h"),
            hex(b"H"),
            hex(b"w"),
            hex(b"t"),
            hex(b"s"),
            hex(b"t")
        ));
        vectors.push(format!("summary\t{h}"));
    }
    let folder = work("push");
    let (input, output) = (folder.join("in.txt"), folder.join("out.txt"));
    std::fs::write(&input, vectors.join("\n") + "\n").unwrap();
    if csharp("push", &input, &output) {
        let theirs = std::fs::read_to_string(&output).unwrap();
        for ((vector, theirs), at) in vectors.iter().zip(theirs.lines()).zip(0..) {
            assert_eq!(push_answer(vector), theirs, "vector {at}: {vector}");
        }
        assert_eq!(theirs.lines().count(), vectors.len());
    }
    let _ = std::fs::remove_dir_all(&folder);
}

struct Memory(std::sync::Mutex<Option<PairingIdentity>>);

impl PairingStore for Memory {
    fn load(&self) -> mighty_platform::Result<Option<PairingIdentity>> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, _: &PairingIdentity) -> mighty_platform::Result<()> {
        Ok(())
    }
}

fn identity(index: u8, devices: Vec<PairedDevice>) -> PairingIdentity {
    PairingIdentity {
        host_id: format!("4e0c0a3f-8a59-4a0f-9a57-0a3c1b2d3e{index:02x}"),
        host_token: hex(&[index; 32]),
        pairing_key: format!("AQgPFh0kKzI5QEdOVVxjanF4f4aNlJuiqbC3vsXM0{index:02}"),
        secret_key: [index.wrapping_mul(7).wrapping_add(1); 32],
        devices,
    }
}

/// What this crate answers for one pairing vector: the file decoded as C#'s constructor decodes it, then the link.
fn pairing_answer(line: &str) -> String {
    let part: Vec<&str> = line.split('\t').collect();
    let Ok(saved) = std::str::from_utf8(&unhex(part[0])).map_err(|_| ()).and_then(|t| identity_file::decode(t).map_err(|_| ())) else {
        return "invalid".into();
    };
    let identity = Identity::load_or_create(Arc::new(Memory(std::sync::Mutex::new(Some(saved)))), false).unwrap();
    let direct: Vec<String> = if part[3] == "-" { vec![] } else { text(part[3]).split(',').map(str::to_string).collect() };
    identity.pairing_url(&text(part[1]), &text(part[2]), &direct)
}

#[test]
fn pairing_urls_and_the_identity_file_are_csharps() {
    let device = PairedDevice {
        id: "AAECAwQFBgcICQoLDA0ODw".into(),
        name: "민수의 \"폰\" <&>".into(),
        token_hash: Some("5723c1b7256eba050781d42f7d4fd96201c4b75d7673b1a82ffbcba6dc2e1c23".into()),
        first_seen: "2026-10-23T01:02:03.0000000+00:00".into(),
        last_seen: "2026-10-24T05:06:07.1234567+09:00".into(),
        push: Some(PushRegistration {
            handle: "__79_Pv6-fj39vX08_Lx8O_u7ezr6uno5-bl5OPi4eA".into(),
            kinds: vec!["completed".into(), "later".into()],
            updated_at: "2026-10-24T05:06:07Z".into(),
        }),
    };
    let files: Vec<String> = vec![
        identity_file::encode(&identity(1, vec![])).unwrap(),
        identity_file::encode(&identity(2, vec![device.clone()])).unwrap(),
        identity_file::encode(&identity(3, vec![PairedDevice { id: "legacy".into(), token_hash: None, push: None, ..device.clone() }]))
            .unwrap(),
        identity_file::encode(&identity(4, vec![])).unwrap().replace("\"version\":1", "\"version\":2"),
        identity_file::encode(&identity(5, vec![device.clone()]))
            .unwrap()
            .replace("__79_Pv6-fj39vX08_Lx8O_u7ezr6uno5-bl5OPi4eA", "not a handle"),
        identity_file::encode(&identity(6, vec![PairedDevice { id: "short".into(), ..device.clone() }])).unwrap(),
        "not json".into(),
    ];
    let names = ["Fixture", "DESKTOP-\u{1}\u{7}PC \u{1F600}", &"n".repeat(130), "이름 / name ?&="];
    let relays = ["wss://mighty-relay.athens96.workers.dev", "ws://127.0.0.1:8787", "wss://[fd7a:115c:a1e0::1]:443"];
    let directs = ["-", "100.101.102.103:43139", "100.101.102.103:43139,[fd7a:115c:a1e0::1]:43139"];
    let mut vectors = vec![];
    for (at, file) in files.iter().enumerate() {
        let direct = directs[at % directs.len()];
        let direct = if direct == "-" { "-".to_string() } else { hex(direct.as_bytes()) };
        vectors.push(format!(
            "{}\t{}\t{}\t{direct}",
            hex(file.as_bytes()),
            hex(relays[at % relays.len()].as_bytes()),
            hex(names[at % names.len()].as_bytes())
        ));
    }
    let folder = work("pairing");
    let (input, output) = (folder.join("in.txt"), folder.join("out.txt"));
    std::fs::write(&input, vectors.join("\n") + "\n").unwrap();
    if csharp("pairing-url", &input, &output) {
        let theirs = std::fs::read_to_string(&output).unwrap();
        for ((vector, theirs), at) in vectors.iter().zip(theirs.lines()).zip(0..) {
            assert_eq!(pairing_answer(vector), theirs, "vector {at}");
        }
        assert_eq!(theirs.lines().count(), vectors.len());
    }
    let _ = std::fs::remove_dir_all(&folder);
}
