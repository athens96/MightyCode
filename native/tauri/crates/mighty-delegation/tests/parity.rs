//! Parity with C# itself: the nine delegation-record files and generated files round-tripped through the delegation file store, request
//! keys, `ChildStates.After` over the whole table, pruning on generated files, the sidebar's rows and the copies' cut, each run through
//! MightyCode.Core by `tools/parity` (`delegation`, Delegation.cs) and through this crate, compared line by line. Without the .NET 10
//! SDK the check says so and passes; `MIGHTY_PARITY=require` makes it fail instead.

#[path = "../../../tools/parity/build_lock.rs"]
mod build_lock;
use std::path::{Path, PathBuf};
use std::process::Command;

use mighty_delegation::copy::{DelegationCopy, DelegationCopyKind};
use mighty_delegation::model::{
    ChildRecord, ChildRecordOptions, ChildState, ChildStateEvent, ChildStates, DeliveryLane, DeliveryReceipt, DeliveryRoute, FollowUp,
    MergeKind, MergeRecord, Notice, NoticeKind, request_key,
};
use mighty_delegation::pruning::DelegationPruneContext;
use mighty_delegation::request::DelegationName;
use mighty_delegation::sidebar;
use mighty_delegation::store::{self, DelegationFile};

/// Whether a .NET 10 SDK is installed (`dotnet --list-sdks` lists a `10.x` one): the parity tool's own build needs it. Without it the
/// check says so and passes, or fails under `MIGHTY_PARITY=require`; with it a build that fails is the check's failure.
#[allow(clippy::disallowed_methods, reason = "the check's own .NET probe, no child of the app")]
fn dotnet_10() -> bool {
    let listed = std::process::Command::new("dotnet").arg("--list-sdks").output();
    let found = matches!(&listed, Ok(output) if output.status.success() && String::from_utf8_lossy(&output.stdout).lines().any(|line| line.starts_with("10.")));
    if !found {
        let why = "no .NET 10 SDK (dotnet --list-sdks)";
        assert!(std::env::var("MIGHTY_PARITY").as_deref() != Ok("require"), "{why}");
        eprintln!("skipped: {why}");
    }
    found
}

#[allow(clippy::disallowed_methods, reason = "the check's own .NET build and run, no child of the app")]
fn csharp(input: &Path, output: &Path) -> bool {
    let tauri = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = tauri.join("target/parity-cs-delegation");
    if !dotnet_10() {
        return false;
    }
    let build = build_lock::hold(&tauri.join("target"));
    let built = Command::new("dotnet")
        .arg("build")
        .arg(tauri.join("tools/parity/Parity.csproj"))
        .args(["-c", "Release", "-o"])
        .arg(&out)
        .args(["--nologo", "-v", "quiet"])
        .status();
    drop(build);
    assert!(matches!(built, Ok(status) if status.success()), "the C# parity tool did not build ({built:?})");
    let ran = Command::new("dotnet").arg(out.join("parity.dll")).arg("delegation").arg(input).arg(output).status().unwrap();
    assert!(ran.success(), "the C# parity tool failed");
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

/// What this crate answers for one vector (Delegation.cs's commands).
fn answer(line: &str) -> String {
    let part: Vec<&str> = line.split('\t').collect();
    match part[0] {
        "roundtrip" => match store::decode(&unhex(part[1])) {
            Some(file) => hex(&store::encode(&file)),
            None => "unreadable".into(),
        },
        "key" => {
            request_key(&text(part[1]), &text(part[2]), &text(part[3]), &text(part[4]), (part[5] != "-").then(|| text(part[5])).as_deref())
        }
        "after" => {
            let state = ChildState::parse(part[1]).unwrap();
            let change = ChildStateEvent::ALL.into_iter().find(|e| format!("{e:?}") == part[2]).unwrap();
            state.after(change).map_or("null", |next| next.name()).to_string()
        }
        "prune" => {
            let file = store::decode(&unhex(part[1])).unwrap();
            let open = part[2].split(',').filter(|p| !p.is_empty()).map(String::from).collect();
            let undoable: Vec<String> = part[3].split(',').filter(|p| !p.is_empty()).map(String::from).collect();
            let context = DelegationPruneContext::new(open, move |merge| undoable.contains(&merge.merged_commit));
            file.pruned(&context, part[4].parse().unwrap()).map_or("null".into(), |pruned| hex(&store::encode(&pruned)))
        }
        "rows" => sidebar::rows(&store::decode(&unhex(part[1])).unwrap())
            .iter()
            .map(|row| {
                let optional = |value: &Option<String>| value.as_deref().map_or("-".to_string(), |v| hex(v.as_bytes()));
                format!(
                    "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
                    row.id,
                    row.parent_session_id,
                    row.state.name(),
                    optional(&row.task),
                    row.parent_branch,
                    optional(&row.parent_checkout),
                    u8::from(row.can_undo),
                    row.held_notices,
                    u8::from(row.started_by_human),
                    row.agent
                )
            })
            .collect::<Vec<_>>()
            .join(";"),
        "copy" => {
            let copy =
                DelegationCopy::of("c", DelegationCopyKind::Report, 1, &unhex(part[1]), (part[2] != "-").then(|| part[2].parse().unwrap()));
            format!("{} {} {}", u8::from(copy.truncated), copy.original_bytes, hex(copy.text.as_bytes()))
        }
        other => format!("unknown {other}"),
    }
}

/// Generated files: every state, lane, kind and optional member, text with escapes and non-ASCII, filled near the cap.
fn generated() -> Vec<DelegationFile> {
    let states = ChildState::ALL;
    let mut files = vec![];
    for (round, slash) in ["/", "\\", "é/\u{1}\"\t한"].iter().enumerate() {
        let mut children = vec![];
        let mut notices = vec![];
        let mut follow_ups = vec![];
        let mut merges = vec![];
        let mut copies = vec![];
        for (index, state) in states.iter().enumerate() {
            let id = format!("c{round}-{index}");
            let mut child = ChildRecord::make_with(
                &id,
                &format!("p{}", index % 3),
                &format!("C:{slash}worktrees{slash}{id}"),
                &format!("feature{slash}{index}"),
                "a1",
                "acceptEdits",
                &format!("k{index}"),
                ChildRecordOptions {
                    state: Some(*state),
                    follow_up_count: (index % 3) as i32,
                    report_revision: index as i32,
                    report_head: (index % 2 == 0).then(|| "b2".to_string()),
                    parent_checkout: (index % 3 != 1).then(|| format!("C:{slash}repo")),
                    started_by_human: (index % 4 == 0).then_some(true),
                    provider: Some(["claude", "codex", "gemini"][index % 3].to_string()),
                },
            );
            child.closed_head = (*state == ChildState::Closed).then(|| "m2".to_string());
            child.run_id = (index % 2 == 1).then(|| format!("run-{index}"));
            child.awaited_run_id = child.run_id.clone();
            child.report_digest = (index % 3 == 2).then(|| "d".repeat(64));
            child.report_run_id = child.run_id.clone();
            let lane = DeliveryLane::ALL[index % 3];
            let receipt = (lane == DeliveryLane::Delivered)
                .then(|| DeliveryReceipt::new("2026-10-10T00:00:00Z", DeliveryRoute::ALL[index % 2], &format!("run-{index}")));
            notices.push(Notice {
                receipt: receipt.clone(),
                ..Notice::new(&format!("n{index}"), &id, index as i32, NoticeKind::ALL[index % 3], lane)
            });
            follow_ups.push(FollowUp { receipt, ..FollowUp::new(&format!("f{index}"), &id, &format!("do {slash} it\n"), lane) });
            if matches!(state, ChildState::Merged | ChildState::Closed) {
                merges.push(MergeRecord::new(&id, MergeKind::ALL[index % 3], "main", "a1", &format!("m{index}"), "m2"));
            }
            copies.push(DelegationCopy::of(&id, DelegationCopyKind::Task, 0, format!("# Task {slash}\n").as_bytes(), None));
            copies.push(DelegationCopy::of(
                &id,
                DelegationCopyKind::Report,
                index as i64,
                "r".repeat(1_000 * (round + 1)).as_bytes(),
                None,
            ));
            children.push(child);
        }
        files.push(DelegationFile::of(children, notices, follow_ups, merges, copies));
    }
    files
}

#[test]
fn delegation_matches_csharp() {
    let mut lines = vec![];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../scripts/fixtures/delegation-record");
    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path().join("profile").join(store::FILE_NAME))
        .filter(|p| p.exists())
        .collect();
    fixtures.sort();
    for path in &fixtures {
        lines.push(format!("roundtrip\t{}", hex(&std::fs::read(path).unwrap())));
    }
    let files = generated();
    for file in &files {
        let bytes = store::encode(file);
        lines.push(format!("roundtrip\t{}", hex(&bytes)));
        lines.push(format!("rows\t{}", hex(&bytes)));
        let size = bytes.len() as i64;
        for (open, undoable, headroom) in [
            ("p0,p1,p2", "", 0),
            ("p0", "m6", store::MAXIMUM_FILE_BYTES as i64 - size + 2_000),
            ("", "", store::MAXIMUM_FILE_BYTES as i64 - size + 20_000),
            ("p1", "m8", store::MAXIMUM_FILE_BYTES as i64),
        ] {
            lines.push(format!("prune\t{}\t{open}\t{undoable}\t{headroom}", hex(&bytes)));
        }
    }
    for damaged in [
        b"{".as_slice(),
        br#"{"version":1,"children":[],"notices":[],"followUps":[],"merges":[],"copies":[],"Version":1}"#,
        br#"{"VERSION":"1","CHILDREN":[],"notices":[],"followUps":[],"merges":[],"copies":[]}"#,
        b"\xEF\xBB\xBF{\"version\":1,\"children\":[],\"notices\":[],\"followUps\":[],\"merges\":[],\"copies\":[]}",
        // Names match without regard to an ASCII letter's case, and of two that match the last wins; U+017F (the long s, whose upper
        // case is S) matches nothing.
        "{\"ver\u{17F}ion\":1,\"children\":[],\"notices\":[],\"followUps\":[],\"merges\":[],\"copies\":[]}".as_bytes(),
        br#"{"version":1,"children":[],"notices":[],"followUps":[],"merges":[],"copies":[],"VERSION":2}"#,
        br#"{"version":2,"children":[],"notices":[],"followUps":[],"merges":[],"copies":[],"Version":1}"#,
    ] {
        lines.push(format!("roundtrip\t{}", hex(damaged)));
    }
    for (session, run, task, mode, other) in [
        ("parent-1", "run-1", "Fix the login bug", "acceptEdits", None),
        ("p", "", "카페 → done", "plan", Some("codex")),
        ("", "", "", "", None),
        ("a\u{0}b", "r", "t\r\n", "auto", Some("gemini")),
    ] {
        let h = |s: &str| hex(s.as_bytes());
        lines.push(format!("key\t{}\t{}\t{}\t{}\t{}", h(session), h(run), h(task), h(mode), other.map_or("-".into(), h)));
    }
    for state in ChildState::ALL {
        for change in ChildStateEvent::ALL {
            lines.push(format!("after\t{}\t{change:?}", state.name()));
        }
    }
    let cap = mighty_delegation::copy::MAXIMUM_COPY_BYTES;
    let copies: Vec<(Vec<u8>, Option<i64>)> = vec![
        (b"short".to_vec(), None),
        (vec![b'a'; cap], None),
        (vec![b'a'; cap + 1], None),
        (vec![b'"'; cap], None),
        ("보고서".repeat(30_000).into_bytes(), None),
        ("\u{1F469}\u{200D}\u{1F4BB} ".repeat(8_000).into_bytes(), None),
        (vec![0xFF, 0xFE, b'h', 0, b'i', 0, 0x3D, 0xD8], None),
        (vec![0xFE, 0xFF, 0, b'h', 0xD8, 0x3D, 0xDE, 0x00, 1], None),
        (vec![0xEF, 0xBB, 0xBF, b'x', 0xC3], Some(10)),
        (vec![0x80, b'a', 0xE2, 0x82], Some(cap as i64 + 5)),
    ];
    for (bytes, total) in copies {
        lines.push(format!("copy\t{}\t{}", hex(&bytes), total.map_or("-".into(), |t| t.to_string())));
    }

    let work = std::env::temp_dir().join(format!("mighty-delegation-parity-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let (input, output) = (work.join("in.txt"), work.join("cs.txt"));
    std::fs::write(&input, lines.join("\n") + "\n").unwrap();
    if !csharp(&input, &output) {
        let _ = std::fs::remove_dir_all(&work);
        return;
    }
    let expected = std::fs::read_to_string(&output).unwrap();
    let _ = std::fs::remove_dir_all(&work);
    let mut differences = 0;
    for (line, cs) in lines.iter().zip(expected.lines()) {
        let rs = answer(line);
        if rs != cs {
            differences += 1;
            eprintln!("{}: C# {} / Rust {}", &line[..line.len().min(60)], &cs[..cs.len().min(300)], &rs[..rs.len().min(300)]);
        }
    }
    assert_eq!(expected.lines().count(), lines.len(), "one answer a vector");
    assert_eq!(differences, 0, "the Rust answers differ from C#'s");
}
