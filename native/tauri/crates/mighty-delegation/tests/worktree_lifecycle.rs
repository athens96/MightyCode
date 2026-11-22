//! Verification for worktree port blocks (`worktree_ports`), the non-concurrent run action rule, dev-server address detection and a
//! script run with a time limit (`run_actions`) (`MC-25`).

mod support;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use mighty_core::model::PaneWorktree;
use mighty_delegation::run_actions::{self, DevUrl, RunningAction, detect_dev_url, runs_to_stop};
use mighty_delegation::worktree_ports::{self, DEFAULT_START, HIGHEST_PORT, PortBlock, PortsFile, find_free_block};
use mighty_platform::SpawnRequest;

const REPO: &str = "/repos/app";

#[test]
fn blocks_never_overlap_and_skip_listening_ports() {
    let mut file = PortsFile::default();
    let free = |_: u16| false;
    let main = file.assign(REPO, REPO, 10, DEFAULT_START, free).unwrap();
    let first = file.assign(REPO, "/wt/a", 10, DEFAULT_START, free).unwrap();
    let second = file.assign(REPO, "/wt/b", 10, DEFAULT_START, free).unwrap();
    assert_eq!(main, PortBlock { base: 41_000, count: 10 });
    assert_eq!(first, PortBlock { base: 41_010, count: 10 });
    assert_eq!(second, PortBlock { base: 41_020, count: 10 });
    assert_eq!(first.label(), ":41010\u{2013}41019");

    // Another repository's worktrees share the computer's ports: no overlap across repositories either, whatever the size.
    let other = file.assign("/repos/other", "/wt/c", 4, DEFAULT_START, free).unwrap();
    assert_eq!(other, PortBlock { base: 41_030, count: 4 });
    let blocks = file.assigned();
    for (i, a) in blocks.iter().enumerate() {
        for b in &blocks[i + 1..] {
            assert!(!a.overlaps(b), "{a:?} overlaps {b:?}");
        }
    }

    // The same worktree asking again keeps its block; a new size moves it.
    assert_eq!(file.assign(REPO, "/wt/a", 10, DEFAULT_START, free), Some(first));
    let resized = file.assign(REPO, "/wt/a", 20, DEFAULT_START, free).unwrap();
    assert_eq!(resized.count, 20);
    assert!(!resized.overlaps(&main) && !resized.overlaps(&second) && !resized.overlaps(&other));

    // A port something listens on is skipped with its block: 41040..41049 holds 41043, so the block starts after it.
    let busy: HashSet<u16> = [41_043, 41_044].into();
    let mut fresh = PortsFile::default();
    fresh.assign(REPO, REPO, 10, DEFAULT_START, |_| false);
    fresh.assign(REPO, "/wt/x", 10, DEFAULT_START, |_| false);
    fresh.assign(REPO, "/wt/y", 10, DEFAULT_START, |_| false);
    fresh.assign(REPO, "/wt/z", 10, DEFAULT_START, |_| false);
    let skipped = fresh.assign(REPO, "/wt/busy", 10, DEFAULT_START, |port| busy.contains(&port)).unwrap();
    assert_eq!(skipped, PortBlock { base: 41_045, count: 10 });

    // `count` 0 turns the assignment off and gives back what the worktree held; nothing fits past the top.
    assert_eq!(file.assign(REPO, "/wt/b", 0, DEFAULT_START, free), None);
    assert_eq!(file.get(REPO, "/wt/b"), None);
    assert_eq!(find_free_block(&[], 10, HIGHEST_PORT - 5, free), None);
    assert_eq!(find_free_block(&[], 10, DEFAULT_START, |_| true), None, "every port busy: no block");
}

#[test]
fn a_released_block_is_reused_and_vanished_worktrees_are_pruned() {
    let mut file = PortsFile::default();
    let free = |_: u16| false;
    file.assign(REPO, REPO, 10, DEFAULT_START, free);
    let a = file.assign(REPO, "/wt/a", 10, DEFAULT_START, free).unwrap();
    file.assign(REPO, "/wt/b", 10, DEFAULT_START, free);
    assert_eq!(file.release(REPO, "/wt/a"), Some(a));
    assert_eq!(file.release(REPO, "/wt/a"), None, "released once");
    let reused = file.assign(REPO, "/wt/c", 10, DEFAULT_START, free).unwrap();
    assert_eq!(reused, a, "the lowest free block is the one given back");

    // The app was closed while /wt/b was removed, and another repository's checkout went away: both go at the start.
    file.assign("/repos/gone", "/repos/gone", 10, DEFAULT_START, free);
    file.assign("/repos/gone", "/wt/g", 10, DEFAULT_START, free);
    let exists = |path: &str| path != "/wt/b" && path != "/repos/gone";
    let mut released = file.prune(exists);
    released.sort();
    assert_eq!(
        released,
        vec![
            ("/repos/app".to_string(), "/wt/b".to_string()),
            ("/repos/gone".to_string(), "/repos/gone".to_string()),
            ("/repos/gone".to_string(), "/wt/g".to_string()),
        ]
    );
    assert!(!file.repos.contains_key("/repos/gone"));

    // A block an open pane's record still holds is adopted back when the file lost it, never over another worktree's block.
    let held = file.get(REPO, "/wt/c").unwrap();
    assert!(!file.adopt(REPO, "/wt/d", held), "overlaps /wt/c");
    assert!(file.adopt(REPO, "/wt/d", PortBlock { base: 50_000, count: 10 }));
    assert!(file.adopt(REPO, "/wt/d", PortBlock { base: 50_000, count: 10 }), "again: already recorded");
    assert!(file.get(REPO, REPO).is_some() && file.get(REPO, "/wt/c").is_some());
}

#[test]
fn the_ports_file_round_trips_and_a_bad_one_reads_empty() {
    let folder = support::place::temp("worktree-ports-file");
    let mut file = PortsFile::default();
    file.assign(REPO, REPO, 10, DEFAULT_START, |_| false);
    file.assign(REPO, "/wt/a", 5, DEFAULT_START, |_| false);
    worktree_ports::save(&folder, &file).unwrap();
    assert_eq!(worktree_ports::load(&folder), file);

    // Overlapping or invalid blocks written by hand are dropped; a malformed file reads as empty.
    std::fs::write(
        folder.join(worktree_ports::FILE_NAME),
        r#"{"version":1,"repos":{"/r":{"/a":{"base":41000,"count":10},"/b":{"base":41005,"count":10},"/c":{"base":65530,"count":10},"/d":{"base":42000,"count":0}}}}"#,
    )
    .unwrap();
    let loaded = worktree_ports::load(&folder);
    assert_eq!(loaded.assigned(), vec![PortBlock { base: 41_000, count: 10 }]);
    std::fs::write(folder.join(worktree_ports::FILE_NAME), "not json").unwrap();
    assert_eq!(worktree_ports::load(&folder), PortsFile::default());
    support::place::remove_folder(&folder);
}

#[test]
fn a_port_something_listens_on_reads_in_use() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(worktree_ports::port_in_use(port));
    drop(listener);
}

#[test]
fn a_worktree_pane_carries_its_block_and_names_into_every_run() {
    let pane = PaneWorktree {
        worktree_path: Some("/home/me/.mightycode/worktrees/p1".into()),
        branch: Some("mighty/p1".into()),
        base_branch: Some("main".into()),
        checkout: Some("/repos/app".into()),
        port: 4141,
        port_base: Some(41_010),
        port_count: Some(10),
        ..PaneWorktree::default()
    };
    let env = pane.environment("/repos/app");
    assert_eq!(env["PORT"], "4141", "the old PORT stays");
    assert_eq!(env["MIGHTY_PORT"], "41010");
    assert_eq!(env["MIGHTY_PORT_COUNT"], "10");
    assert_eq!(env["MIGHTY_WORKTREE_NAME"], "p1");
    assert_eq!(env["MIGHTY_ROOT_PATH"], "/repos/app");
    assert_eq!(env["MIGHTY_WORKTREE_PATH"], "/home/me/.mightycode/worktrees/p1");
    assert!(env.keys().all(|key| mighty_run::model::PANE_ENVIRONMENT_KEYS.contains(&key.as_str())), "the agent gets every one");

    // Without a block (`ports: 0`) the names still go, the ports do not.
    let off = pane.environment("/repos/app");
    let without = PaneWorktree { port_base: None, port_count: None, ..pane }.environment("/repos/app");
    assert!(off.contains_key("MIGHTY_PORT") && !without.contains_key("MIGHTY_PORT") && without.contains_key("MIGHTY_WORKTREE_NAME"));
}

#[test]
fn a_non_concurrent_start_stops_only_the_same_action_in_other_worktrees_of_the_repository() {
    let running = [
        RunningAction { action: "e2e", repo: REPO, folder: "/wt/a" },
        RunningAction { action: "e2e", repo: REPO, folder: "/wt/b" },
        RunningAction { action: "dev", repo: REPO, folder: "/wt/a" },
        RunningAction { action: "e2e", repo: "/repos/other", folder: "/wt/o" },
        RunningAction { action: "e2e", repo: REPO, folder: "/wt/c" },
    ];
    assert_eq!(runs_to_stop(&running, "e2e", REPO, "/wt/c", false), vec![0, 1]);
    assert!(runs_to_stop(&running, "e2e", REPO, "/wt/c", true).is_empty(), "a concurrent action stops nothing");
    assert!(runs_to_stop(&running, "lint", REPO, "/wt/c", false).is_empty());
}

#[test]
fn dev_server_addresses_are_found_in_common_tool_output() {
    let found = |text: &str| detect_dev_url(&text.replace("\x1b[1m", "").replace("\x1b[22m", "")).map(|DevUrl { url, port }| (url, port));
    // Vite
    assert_eq!(
        found("  VITE v5.2.0  ready in 312 ms\n\n  ➜  Local:   http://localhost:41010/\n  ➜  Network: use --host to expose\n"),
        Some(("http://localhost:41010/".into(), 41_010))
    );
    // Next.js
    assert_eq!(
        found("   ▲ Next.js 14.2.3\n   - Local:        http://localhost:41011\n   - Environments: .env\n"),
        Some(("http://localhost:41011".into(), 41_011))
    );
    // Rails / Puma: 0.0.0.0 opens as localhost; the first address wins.
    assert_eq!(
        found("* Listening on http://0.0.0.0:41012\n* Listening on http://[::1]:41012\nUse Ctrl-C to stop"),
        Some(("http://localhost:41012".into(), 41_012))
    );
    // cargo-watch running an Axum server, with trailing punctuation and colour codes.
    assert_eq!(
        found("[Running 'cargo run']\n   Compiling api v0.1.0\nlistening on \x1b[1mhttp://127.0.0.1:41013/api\x1b[22m."),
        Some(("http://127.0.0.1:41013/api".into(), 41_013))
    );
    assert_eq!(found("Serving at https://[::1]:8443/app, press q"), Some(("https://[::1]:8443/app".into(), 8443)));
    for none in ["see https://example.com:3000 for docs", "http://localhost/ without a port", "xhttp://localhost:3000", "port 3000", ""] {
        assert_eq!(found(none), None, "{none}");
    }
    assert_eq!(run_actions::url_port("http://localhost:8000/docs"), Some(8000));
    assert_eq!(run_actions::url_port("https://localhost/"), Some(443));
}

fn shell(command: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        (comspec, vec!["/d".into(), "/s".into(), "/c".into(), command.into()])
    } else {
        ("/bin/sh".into(), vec!["-c".into(), command.into()])
    }
}

#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_teardown_past_its_limit_is_stopped_and_one_that_ends_reports_its_code() {
    let processes = support::place::processes();
    let folder = support::place::temp("worktree-teardown");

    let slow = if cfg!(windows) { "echo started && ping -n 30 127.0.0.1 >nul" } else { "echo started; sleep 30" };
    let (program, args) = shell(slow);
    let began = Instant::now();
    let outcome = run_actions::run_bounded(processes.as_ref(), SpawnRequest::new(program, args, &folder), Duration::from_secs(1)).await;
    assert!(outcome.timed_out, "{outcome:?}");
    assert_eq!(outcome.exit_code, None);
    assert!(!outcome.succeeded());
    assert!(began.elapsed() < Duration::from_secs(15), "stopped at the limit, not after the script: {:?}", began.elapsed());
    assert!(outcome.output.contains("started"), "the output before the stop is kept: {:?}", outcome.output);

    let (program, args) = shell("echo cleaned && exit 3");
    let outcome = run_actions::run_bounded(processes.as_ref(), SpawnRequest::new(program, args, &folder), Duration::from_secs(30)).await;
    assert!(!outcome.timed_out);
    assert_eq!(outcome.exit_code, Some(3));
    assert!(outcome.output.contains("cleaned"));

    let (program, args) = shell("echo ok");
    let outcome = run_actions::run_bounded(processes.as_ref(), SpawnRequest::new(program, args, &folder), Duration::from_secs(30)).await;
    assert!(outcome.succeeded(), "{outcome:?}");
    support::place::remove_folder(&folder);
}
