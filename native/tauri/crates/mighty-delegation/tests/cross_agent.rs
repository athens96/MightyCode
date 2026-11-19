//! A child of another agent than its parent's (C# MightyCode.Core.Tests/DelegationCrossAgentVerification.cs, ported from the Mac's
//! DelegationCrossAgentTests and held to native/contracts/fixtures/delegation-modes.json, DelegationModesFixtureTests): which mode it
//! starts in, how delegate names its agent, what the worktree button's menu offers, and the child a parent or the human makes. An
//! answer's MCP rendering is W7-C's (`mcp::delegation`); L698's panes and L706's service are W7-D's.

mod support;

use std::path::Path;

use mighty_core::shell::{ChildState as RowState, DelegationChildRow};
use mighty_delegation::coordinator::argument_problem;
use mighty_delegation::host::DelegationAgentAvailability;
use mighty_delegation::model::{ChildRecord, ChildRecordOptions, ChildState, request_key};
use mighty_delegation::providers::{
    self, AGENT_IDS, DelegationChildBlock, DelegationChildChoice, ModeCapability, capabilities, is_covered, is_no_wider,
    is_no_wider_across, widest_child_mode,
};
use mighty_delegation::request::{
    DelegationChildInfo, DelegationChildStatus, DelegationChildSummary, DelegationName, DelegationReasonCode,
};
use mighty_delegation::store::DelegationFile;
use mighty_delegation::tools::delegate::{first_input, human_first_run, human_note, unavailable};
use mighty_delegation::tools::{self};
use mighty_delegation::{git::worktree::ChildWorktree, sidebar};
use mighty_run::acp::accounts::CliAccountStatus;
use serde_json::Value;

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn sorted(mut items: Vec<String>) -> Vec<String> {
    items.sort();
    items
}

/// "delegation: every agent's modes and widest child modes are native/contracts/fixtures/delegation-modes.json"
#[test]
fn every_agents_modes_and_widest_child_modes_are_the_shared_fixture() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures/delegation-modes.json");
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let names = |value: &Value| value.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    let agents: Vec<String> = AGENT_IDS.iter().map(|a| a.to_string()).collect();
    assert_eq!(
        fixture["agents"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect::<Vec<_>>(),
        agents,
        "the agents are the app's three, in order"
    );
    assert_eq!(names(&fixture["capabilities"]), agents, "every agent has its capabilities");
    for (provider, modes) in fixture["capabilities"].as_object().unwrap() {
        assert_eq!(
            sorted(names(modes)),
            sorted(providers::starting_modes(provider).iter().map(|m| m.to_string()).collect()),
            "{provider}'s modes"
        );
        for (mode, expected) in modes.as_object().unwrap() {
            let own = sorted(capabilities(provider, mode).iter().map(|c| c.name().to_string()).collect());
            let wanted = sorted(expected.as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect());
            assert_eq!(own, wanted, "what {provider} {mode} may do");
        }
    }
    assert_eq!(names(&fixture["widestChildMode"]), agents, "every agent is a parent");
    for (parent, modes) in fixture["widestChildMode"].as_object().unwrap() {
        assert_eq!(
            sorted(names(modes)),
            sorted(providers::starting_modes(parent).iter().map(|m| m.to_string()).collect()),
            "{parent}'s modes as a parent"
        );
        for (parent_mode, children) in modes.as_object().unwrap() {
            assert_eq!(names(children), agents, "every child agent under {parent} {parent_mode}");
            for (child, expected) in children.as_object().unwrap() {
                assert_eq!(widest_child_mode(child, parent, parent_mode), expected.as_str(), "{child} under {parent} {parent_mode}");
            }
        }
    }
}

/// "delegation: agents are named by their words, and modes compare within and across agents"
#[test]
fn agents_are_named_by_their_words_and_modes_compare_within_and_across_agents() {
    for (word, provider) in [
        ("claude", "claude"),
        ("codex", "codex"),
        ("antigravity", "gemini"),
        ("gemini", "gemini"),
        ("Codex", "codex"),
        (" ANTIGRAVITY\n", "gemini"),
    ] {
        assert_eq!(providers::agent(word), Some(provider), "the agent named {}", word.trim());
    }
    for word in ["", "gpt", "openai", "agy", "claude code", "codex-cli"] {
        assert_eq!(providers::agent(word), None, "no agent is named '{word}'");
    }
    assert_eq!(AGENT_IDS.iter().map(|a| providers::agent_name(a)).collect::<Vec<_>>(), ["claude", "codex", "antigravity"]);
    use ModeCapability::{RunsUnasked, WritesAsking, WritesUnasked};
    assert!(
        is_covered(&[WritesAsking], &[WritesUnasked])
            && !is_covered(&[WritesUnasked], &[WritesAsking])
            && !is_covered(&[RunsUnasked], &[WritesUnasked])
            && is_covered(&[], &[]),
        "the subset rule"
    );
    for parent_mode in ["plan", "manual"] {
        for (child, modes) in
            [("codex", &["manual", "acceptEdits", "onRequest", "fullAccess"][..]), ("gemini", &["acceptEdits", "fullAccess"])]
        {
            for mode in modes {
                assert!(!is_no_wider_across(mode, child, parent_mode, "claude"), "{child} {mode} under claude {parent_mode}");
            }
            assert_eq!(widest_child_mode(child, "claude", parent_mode), None, "{child} under claude {parent_mode}");
        }
    }
    assert!(
        is_no_wider_across("onRequest", "codex", "acceptEdits", "claude")
            && is_no_wider_across("acceptEdits", "gemini", "acceptEdits", "claude")
            && !is_no_wider_across("acceptEdits", "codex", "acceptEdits", "claude"),
        "under Claude acceptEdits"
    );
    assert!(
        is_no_wider_across("acceptEdits", "codex", "auto", "claude") && !is_no_wider_across("fullAccess", "codex", "auto", "claude"),
        "under Claude auto"
    );
    assert!(
        is_no_wider_across("acceptEdits", "claude", "onRequest", "codex")
            && is_no_wider_across("manual", "claude", "onRequest", "codex")
            && !is_no_wider_across("auto", "claude", "onRequest", "codex")
            && is_no_wider_across("auto", "claude", "acceptEdits", "codex"),
        "under Codex onRequest and acceptEdits"
    );
    assert!(
        is_no_wider_across("plan", "claude", "manual", "codex")
            && providers::child_can_write("claude", "plan")
            && ["manual", "acceptEdits", "auto", "fullAccess"].iter().all(|m| !is_no_wider_across(m, "claude", "manual", "codex")),
        "under a read-only Codex"
    );
    assert_eq!(widest_child_mode("claude", "codex", "manual"), Some("plan"));
    assert_eq!(widest_child_mode("gemini", "codex", "manual"), None);
    assert!(
        !is_no_wider_across("onRequest", "claude", "fullAccess", "codex")
            && !is_no_wider_across("acceptEdits", "codex", "onRequest", "claude"),
        "a mode the agent lacks"
    );
    assert_eq!(widest_child_mode("codex", "claude", "sideways"), None, "an unknown parent mode");
    assert_eq!(widest_child_mode("other", "claude", "fullAccess"), None, "an unknown child agent");
    for provider in AGENT_IDS {
        let modes = providers::starting_modes(provider);
        for (own, stored) in modes.iter().enumerate() {
            for (asked, mode) in modes.iter().enumerate() {
                assert_eq!(is_no_wider_across(mode, provider, stored, provider), asked <= own, "{provider} {mode} under {stored}");
                assert_eq!(is_no_wider(mode, stored, provider), asked <= own, "{provider} {mode} under {stored} (3 arguments)");
            }
        }
    }
    let human = |child, parent, mode, plan| providers::human_child_mode(child, parent, mode, plan);
    assert_eq!(human("claude", "claude", "auto", false).as_deref(), Some("auto"));
    assert_eq!(human("claude", "claude", "auto", true).as_deref(), Some("plan"));
    assert_eq!(human("codex", "codex", "onRequest", true).as_deref(), Some("onRequest"), "Codex has no plan mode");
    assert_eq!(human("codex", "codex", "manual", false), None);
    assert_eq!(human("gemini", "gemini", "acceptEdits", true), None, "Antigravity's plan child cannot write");
    assert_eq!(human("codex", "claude", "auto", true).as_deref(), Some("acceptEdits"));
    assert_eq!(human("claude", "codex", "onRequest", true).as_deref(), Some("acceptEdits"));
    assert_eq!(human("claude", "codex", "manual", false).as_deref(), Some("plan"));
    assert_eq!(human("gemini", "claude", "plan", false), None);
    for (provider, mode) in [("codex", "manual"), ("gemini", "plan"), ("gemini", "manual")] {
        assert!(providers::may_have_human_child("claude", provider, mode), "{provider} {mode} may have a Claude child");
    }
    assert!(
        providers::may_have_human_child("claude", "claude", "plan") && !providers::may_have_human_child("shell", "claude", "fullAccess"),
        "only an agent pane"
    );
}

fn account(installed: bool, logged_in: Option<bool>) -> CliAccountStatus {
    CliAccountStatus { installed, logged_in, ..CliAccountStatus::new("codex") }
}

fn choice(agent: &str, mode: Option<&str>, blocked: Option<DelegationChildBlock>) -> DelegationChildChoice {
    DelegationChildChoice { agent: agent.into(), mode: mode.map(str::to_string), blocked }
}

/// "delegation: what the app knows of each CLI, the worktree menu and its cap"
#[test]
fn what_the_app_knows_of_each_cli_the_worktree_menu_and_its_cap() {
    use DelegationAgentAvailability::{NotInstalled, Ready, SignedOut};
    assert_eq!(providers::availability(None, None), Ready, "nothing known");
    assert_eq!(providers::availability(Some(true), Some(&account(true, Some(true)))), Ready, "found and signed in");
    assert_eq!(providers::availability(Some(true), Some(&account(true, None))), Ready, "sign-in unreadable");
    assert_eq!(providers::availability(Some(false), None), NotInstalled, "not found");
    assert_eq!(providers::availability(Some(true), Some(&account(false, None))), NotInstalled, "says it is not installed");
    assert_eq!(providers::availability(None, Some(&account(true, Some(false)))), SignedOut, "signed out");

    let states = |agent: &str| match agent {
        "codex" => SignedOut,
        _ => NotInstalled,
    };
    assert_eq!(
        providers::human_child_choices("claude", "auto", false, 0, states),
        [
            choice("claude", Some("auto"), None),
            choice("codex", Some("acceptEdits"), Some(DelegationChildBlock::SignedOut)),
            choice("gemini", Some("acceptEdits"), Some(DelegationChildBlock::NotInstalled))
        ],
        "Claude auto's menu"
    );
    let ready = providers::human_child_choices("claude", "auto", true, 2, |_| Ready);
    assert_eq!(
        ready.iter().map(|c| c.mode.clone()).collect::<Vec<_>>(),
        [Some("plan".into()), Some("acceptEdits".into()), Some("acceptEdits".into())],
        "plan first stays with Claude's own child"
    );
    assert!(ready.iter().all(|c| c.blocked.is_none()), "two open children leave room");
    assert_eq!(
        providers::human_child_choices("claude", "manual", false, 0, |_| Ready).iter().map(|c| c.blocked).collect::<Vec<_>>(),
        [None, Some(DelegationChildBlock::NoMode), Some(DelegationChildBlock::NoMode)]
    );
    assert_eq!(
        providers::human_child_choices("codex", "manual", false, 0, |_| Ready),
        [
            choice("claude", Some("plan"), None),
            choice("codex", None, Some(DelegationChildBlock::NoMode)),
            choice("gemini", None, Some(DelegationChildBlock::NoMode))
        ]
    );
    let codex = providers::human_child_choices("codex", "onRequest", false, 0, |_| Ready);
    assert_eq!(
        codex.iter().map(|c| c.mode.clone()).collect::<Vec<_>>(),
        [Some("acceptEdits".into()), Some("onRequest".into()), Some("acceptEdits".into())]
    );
    assert!(codex.iter().all(|c| c.blocked.is_none()), "Codex onRequest offers all three");
    assert!(
        providers::human_child_choices("codex", "fullAccess", false, 3, |_| Ready)
            .iter()
            .all(|c| c.blocked == Some(DelegationChildBlock::Cap)),
        "three open children: none more"
    );
    assert_eq!(
        providers::human_child_choices("codex", "manual", false, 3, |_| Ready),
        [
            choice("claude", Some("plan"), Some(DelegationChildBlock::Cap)),
            choice("codex", None, Some(DelegationChildBlock::NoMode)),
            choice("gemini", None, Some(DelegationChildBlock::NoMode))
        ],
        "NoMode wins over the cap on a read-only Codex pane"
    );
    assert_eq!(
        providers::human_child_choices("claude", "manual", false, 3, |a| if a == "gemini" { SignedOut } else { NotInstalled })
            .iter()
            .map(|c| c.blocked)
            .collect::<Vec<_>>(),
        [Some(DelegationChildBlock::Cap), Some(DelegationChildBlock::NotInstalled), Some(DelegationChildBlock::SignedOut)],
        "another agent not installed or signed out shows that"
    );
    assert_eq!(providers::availability(Some(false), Some(&account(true, Some(false)))), NotInstalled);
    assert_eq!(providers::availability(Some(true), Some(&account(false, Some(false)))), NotInstalled);

    let row = |id: &str, parent: &str, state: RowState, human: bool, agent: &str| DelegationChildRow {
        id: id.into(),
        parent_session_id: parent.into(),
        state,
        task: None,
        parent_branch: String::new(),
        parent_checkout: None,
        can_undo: false,
        held_notices: 0,
        started_by_human: human,
        agent: agent.into(),
    };
    let rows = [
        row("a", "p", RowState::Running, true, "codex"),
        row("b", "p", RowState::Reported, true, "claude"),
        row("c", "p", RowState::Running, false, "claude"),
        row("d", "p", RowState::Closed, true, "gemini"),
        row("e", "q", RowState::Running, true, "claude"),
    ];
    assert_eq!(sidebar::open_human_child_count("p", &rows), 2, "open human children of p");
    let file = DelegationFile::of(
        vec![ChildRecord::make_with(
            "x",
            "p",
            "/w/x",
            "main",
            COMMIT,
            "acceptEdits",
            "k",
            ChildRecordOptions { started_by_human: Some(true), provider: Some("codex".into()), ..Default::default() },
        )],
        vec![],
        vec![],
        vec![],
        vec![],
    );
    let listed = sidebar::rows(&file);
    assert!(listed[0].agent == "codex" && listed[0].started_by_human, "a row carries the human's mark and the child's agent");

    let other = [choice("codex", None, Some(DelegationChildBlock::NoMode)), choice("claude", Some("plan"), None)];
    let menu_only = |provider: &str, mode: &str, choices: &[DelegationChildChoice], is_child: bool, on: bool, host: bool| {
        providers::menu_only("claude", provider, mode, is_child, on, host, choices.iter().any(|c| c.blocked.is_none()))
    };
    assert!(menu_only("codex", "manual", &other, false, true, true), "a read-only Codex shows the menu alone");
    assert!(
        !menu_only("codex", "manual", &other, true, true, true)
            && !menu_only("codex", "manual", &other, false, false, true)
            && !menu_only("codex", "manual", &other, false, true, false)
    );
    assert!(
        !menu_only("codex", "acceptEdits", &other, false, true, true) && !menu_only("claude", "plan", &other, false, true, true),
        "where the pane's own child can write, Ctrl+Enter and the click make one"
    );
    for (provider, mode) in [("codex", "manual"), ("gemini", "plan"), ("gemini", "manual")] {
        let choices = providers::human_child_choices(provider, mode, false, 0, |_| Ready);
        assert!(choices.iter().filter(|c| c.blocked.is_none()).map(|c| c.agent.as_str()).eq(["claude"]), "{provider} {mode}: only Claude");
        assert_eq!(choices[0].mode.as_deref(), Some("plan"));
        assert!(menu_only(provider, mode, &choices, false, true, true), "{provider} {mode}: the menu alone");
        let without = providers::human_child_choices(provider, mode, false, 0, |a| if a == "claude" { NotInstalled } else { Ready });
        assert!(!menu_only(provider, mode, &without, false, true, true), "{provider} {mode}: nothing to run, no button");
    }
    assert!(
        providers::returns_to_composer(Some("codex"), "claude")
            && providers::returns_to_composer(Some("claude"), "gemini")
            && !providers::returns_to_composer(None, "claude")
            && !providers::returns_to_composer(Some("codex"), "codex"),
        "who gets a text whose child did not start"
    );
}

/// "delegation: delegate takes an optional agent, and every answer names the child's agent"
#[test]
fn delegate_takes_an_optional_agent_and_every_answer_names_the_childs_agent() {
    assert_eq!(DelegationReasonCode::AgentUnknown.name(), "agent_unknown");
    assert_eq!(DelegationReasonCode::AgentUnavailable.name(), "agent_unavailable");
    assert_eq!(tools::DELEGATE.arguments.iter().map(|a| a.name).collect::<Vec<_>>(), ["task", "mode", "agent"]);
    assert_eq!(tools::DELEGATE.arguments.iter().map(|a| a.required).collect::<Vec<_>>(), [true, true, false], "only agent is optional");
    let keys = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    assert!(
        argument_problem(keys(&["task", "mode"]).iter(), &tools::DELEGATE).is_none()
            && argument_problem(keys(&["task", "mode", "agent"]).iter(), &tools::DELEGATE).is_none(),
        "with or without agent"
    );
    let problem = Some("delegate takes exactly these string arguments: task, mode, and optionally agent.".to_string());
    assert_eq!(argument_problem(keys(&["task", "agent"]).iter(), &tools::DELEGATE), problem, "mode is still required");
    assert_eq!(argument_problem(keys(&["task", "mode", "model"]).iter(), &tools::DELEGATE), problem, "no other argument");
    assert!(argument_problem(keys(&["child"]).iter(), &tools::CHILD_STATUS).is_none());
    assert_eq!(
        argument_problem(keys(&[]).iter(), &tools::CHILD_STATUS).as_deref(),
        Some("child_status takes exactly these string arguments: child.")
    );
    for agent in ["Claude", "Codex", "Antigravity"] {
        assert!(tools::DELEGATE.arguments[1].description.contains(&format!("{agent} child")), "mode names a {agent} child's modes");
    }
    let detail = unavailable("codex", DelegationAgentAvailability::SignedOut).unwrap();
    assert_eq!(detail, "Codex is not signed in on this PC; the user can check it in Mighty Code's Settings › Components");
    assert!(
        unavailable("gemini", DelegationAgentAvailability::NotInstalled).is_some_and(|d| d.ends_with("Settings › Components"))
            && unavailable("codex", DelegationAgentAvailability::Ready).is_none()
    );

    assert_eq!(
        request_key("parent-1", "run-1", "Fix the login bug", "acceptEdits", None),
        "0767d17eeeaaa20c3f5c0e3a11dc260bd005cc116a76ea02adb2c78fb3a48ef7"
    );
    assert_eq!(
        request_key("parent-1", "run-1", "Fix the login bug", "acceptEdits", Some("codex")),
        "f051d5f56c52b257a1483760fee991d3622cd74bfb03cd1a90da23be50177e8b",
        "a Codex child's key"
    );
    assert_eq!(
        request_key("parent-1", "run-1", "Fix the login bug", "acceptEdits", Some("gemini")),
        "28e97194accdf6d2519a39c9a1144dda67ee4d149fd3193204edc986fc040e3c",
        "an Antigravity child's key"
    );

    let gemini = ChildRecord::make_with(
        "c1",
        "p",
        "/w/c1",
        "main",
        COMMIT,
        "acceptEdits",
        "k",
        ChildRecordOptions { state: Some(ChildState::Running), provider: Some("gemini".into()), ..Default::default() },
    );
    assert_eq!(DelegationChildInfo::of(&gemini).agent, "antigravity", "delegate's child");
    assert_eq!(DelegationChildSummary::of(&gemini).agent, "antigravity", "list_children's row");
    assert_eq!(DelegationChildStatus::of(&gemini, None).agent, "antigravity", "child_status");
    assert_eq!(DelegationChildInfo::of(&ChildRecord { provider: None, ..gemini.clone() }).agent, "claude", "a Claude child");
    assert!(
        serde_json::to_string(&DelegationChildInfo::of(&gemini)).unwrap().contains("\"agent\":\"antigravity\""),
        "the pane pipe carries the agent"
    );
}

/// "delegation: a child's words follow its agent and name a Claude parent as one"
#[test]
fn a_childs_words_follow_its_agent_and_name_a_claude_parent_as_one() {
    let worktree = ChildWorktree {
        session_id: "c1".into(),
        worktree_path: r"C:\w\c1".into(),
        working_folder: r"C:\w\c1".into(),
        branch: "mighty/c1".into(),
        parent_branch: "main".into(),
        base_commit: COMMIT.into(),
    };
    let codex_of_claude = first_input("Do it.", &worktree, "codex", Some("claude"));
    assert!(
        codex_of_claude.starts_with("Another Claude pane") && codex_of_claude.contains("Do not commit"),
        "a Codex child of a Claude pane"
    );
    let claude_of_codex = first_input("Do it.", &worktree, "claude", Some("codex"));
    assert!(
        claude_of_codex.starts_with("Another pane")
            && claude_of_codex.contains("commit your work")
            && !claude_of_codex.contains("Do not commit"),
        "a Claude child of a Codex pane"
    );
    assert_eq!(first_input("Do it.", &worktree, "codex", Some("codex")), first_input("Do it.", &worktree, "codex", None));
    assert_eq!(first_input("Do it.", &worktree, "claude", Some("claude")), first_input("Do it.", &worktree, "claude", None));
    let note = human_note(&worktree, "gemini", Some("claude"));
    assert!(
        note.contains("beside another Claude pane") && note.contains("Do not commit"),
        "the note to an Antigravity child of a Claude pane"
    );
    assert!(
        human_note(&worktree, "claude", Some("codex")).contains("beside another pane so"),
        "the note to a Claude child of a Codex pane"
    );
    assert_eq!(
        human_first_run("Fix it.", &worktree, "gemini", Some("claude")).input,
        format!("Fix it.\n\n{note}"),
        "the human's first run carries that note"
    );
}

mod git {
    use std::path::Path;
    use std::sync::Arc;

    use mighty_delegation::coordinator::Coordinator;
    use mighty_delegation::git::auto_commit::message_for;
    use mighty_delegation::git::worktree::report_file_in;
    use mighty_delegation::host::{DelegationAgentAvailability, DelegationPaneActivity, DelegationPaneState};
    use mighty_delegation::model::{ChildRecord, ChildState};
    use mighty_delegation::reports::ChildRunEnd;
    use mighty_delegation::request::{DelegationCaller, DelegationReasonCode, DelegationResponse};
    use mighty_delegation::store::{DelegationFile, DelegationFileStore};
    use mighty_delegation::tools::delegate::{HumanChildStart, HumanChildStarter};

    use super::support::host::{self, TestHost, eventually, locale, request};
    use super::support::lane;
    use super::support::place::DelegationPlace;

    /// A temp repository whose pane "parent", running `provider` in `mode`, calls the tools from a run started in that mode.
    struct Cross {
        files: DelegationPlace,
        host: Arc<TestHost>,
        coordinator: Coordinator,
        provider: String,
        mode: String,
    }

    impl Cross {
        fn bytes(&self) -> Vec<u8> {
            std::fs::read(DelegationFileStore::new(self.files.profile()).file_path()).unwrap()
        }
        async fn call(&self, tool: &str, arguments: &[(&str, &str)]) -> DelegationResponse {
            let caller = DelegationCaller {
                provider: self.provider.clone(),
                delegation: true,
                permission_mode: self.mode.clone(),
                ..DelegationCaller::new("parent")
            };
            self.coordinator.handle_call(request(tool, arguments), caller).await
        }
        /// The human's send of `text` from the parent's worktree menu, naming `agent`; None is Ctrl+Enter.
        async fn send(&self, text: &str, agent: Option<&str>, plan_first: bool) -> HumanChildStart {
            let starter = HumanChildStarter {
                start: Arc::new(|child: String, _| Box::pin(async move { Some(format!("send-run-of-{child}")) })),
                failed: Arc::new(|_| Box::pin(async {})),
            };
            self.coordinator.start_human_child(text, "parent", starter, plan_first, agent).await
        }
        async fn wait_for(&self, id: &str, state: ChildState) -> bool {
            eventually(60_000, || self.coordinator.file().child(id).map(|c| c.state) == Some(state) && !self.coordinator.is_starting(id))
                .await
        }
        fn nothing_made(&self, before: &[u8]) {
            assert!(
                self.bytes() == before
                    && !Path::new(&self.files.root()).exists()
                    && self.host.created_panes().is_empty()
                    && self.host.started_runs().is_empty(),
                "the refused call made nothing"
            );
        }
        async fn report(&self, child: &ChildRecord, run_id: &str) {
            std::fs::write(report_file_in(&child.worktree_path), "# Report\n\nDone.\n").unwrap();
            self.coordinator.child_run_ended(&child.id, run_id, ChildRunEnd::Finished).await;
        }
        fn only(&self) -> ChildRecord {
            self.coordinator.file().children[0].clone()
        }
    }

    async fn cross(provider: &str, mode: &str) -> Cross {
        let files = DelegationPlace::new();
        let store = DelegationFileStore::new(files.profile());
        store.save(&DelegationFile::empty()).unwrap();
        let test_host = TestHost::new(Some(files.profile().into()), "run");
        test_host.set_first_run(Some(|id: &str| format!("run-of-{id}")));
        test_host.set_makes_panes(true);
        std::fs::create_dir_all(files.repo()).unwrap();
        files.repository_default().await;
        test_host.set(DelegationPaneState {
            session_id: "parent".into(),
            permission_mode: mode.into(),
            folder: files.repo(),
            kind: "claude".into(),
            provider: provider.into(),
            parent_session_id: None,
            run_id: Some("run-1".into()),
            activity: DelegationPaneActivity::Running,
        });
        let coordinator = host::coordinator(store, test_host.clone(), host::maker(&files, 50_000_000_000), || true).unwrap();
        Cross { files, host: test_host, coordinator, provider: provider.into(), mode: mode.into() }
    }

    fn made(answer: HumanChildStart) -> ChildRecord {
        match answer {
            HumanChildStart::Started(child) => *child,
            other => panic!("No child was made: {other:?}"),
        }
    }

    fn agent_words(key: &str, label: &str) -> String {
        locale().get_with(key, &[("agent", label)])
    }

    /// "delegation: a Claude parent's Codex child gets the worktree and the app's commit"
    #[tokio::test]
    async fn a_claude_parents_codex_child_gets_the_worktree_and_the_apps_commit() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let place = cross("claude", "auto").await;
            let start = place.files.fixture_git(&["rev-parse", "HEAD"]).await;
            let info = place
                .call("delegate", &[("task", "Add a note."), ("mode", "onRequest"), ("agent", "codex")])
                .await
                .child
                .expect("delegate made no Codex child");
            assert!(
                info.agent == "codex" && info.starting_mode == "onRequest" && place.wait_for(&info.id, ChildState::Running).await,
                "a Codex child in onRequest runs"
            );
            let child = place.only();
            assert!(
                child.provider.as_deref() == Some("codex") && child.agent() == "codex" && child.notifies_parent(),
                "its record runs Codex and notifies its parent"
            );
            assert!(
                place
                    .host
                    .created_panes()
                    .iter()
                    .map(|p| (p.provider.as_str().to_string(), p.mode.clone()))
                    .eq([("codex".to_string(), "onRequest".to_string())]),
                "its pane runs Codex in onRequest"
            );
            assert_eq!(place.host.availability_asked(), ["codex"], "the host was asked about Codex once");
            let first = place.host.started_runs()[0].1.clone();
            assert!(
                first.starts_with("Another Claude pane") && first.contains("Do not commit"),
                "a Codex child's first words, from a Claude pane"
            );
            std::fs::write(Path::new(&child.worktree_path).join("note.txt"), "Note\n").unwrap();
            place.report(&child, &format!("run-of-{}", child.id)).await;
            let head = place.files.git_in(&child.worktree_path, &["rev-parse", "HEAD"]).await;
            assert!(
                head != start
                    && place.files.git_in(&child.worktree_path, &["log", "-1", "--format=%s"]).await == message_for(&child.id, 1)
                    && place.only().report_head.as_deref() == Some(head.as_str()),
                "the app committed the Codex child's work"
            );
            assert!(place.call("list_children", &[]).await.children.unwrap().iter().map(|c| c.agent.as_str()).eq(["codex"]));
            assert_eq!(place.call("child_status", &[("child", &child.id)]).await.status.map(|s| s.agent).as_deref(), Some("codex"));
            assert!(
                place.coordinator.file().notices.iter().map(|n| n.child_id.as_str()).eq([child.id.as_str()]),
                "one notice for the parent"
            );
        })
        .await;
    }

    /// "delegation: a Codex parent's Claude child commits its own work"
    #[tokio::test]
    async fn a_codex_parents_claude_child_commits_its_own_work() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let place = cross("codex", "acceptEdits").await;
            let start = place.files.fixture_git(&["rev-parse", "HEAD"]).await;
            let info = place
                .call("delegate", &[("task", "Add a note."), ("mode", "acceptEdits"), ("agent", "claude")])
                .await
                .child
                .expect("delegate made no Claude child");
            assert!(info.agent == "claude" && place.wait_for(&info.id, ChildState::Running).await, "a Claude child runs");
            let child = place.only();
            assert!(child.provider.is_none() && child.agent() == "claude", "its record is a Claude child's");
            assert!(place.host.created_panes().iter().map(|p| p.provider.as_str()).eq(["claude"]), "its pane runs Claude");
            let first = place.host.started_runs()[0].1.clone();
            assert!(
                first.starts_with("Another pane") && first.contains("commit your work") && !first.contains("Do not commit"),
                "a Claude child's first words, from a Codex pane"
            );
            std::fs::write(Path::new(&child.worktree_path).join("note.txt"), "Note\n").unwrap();
            place.report(&child, &format!("run-of-{}", child.id)).await;
            assert_eq!(place.files.git_in(&child.worktree_path, &["rev-parse", "HEAD"]).await, start, "no app commit");
            assert_eq!(place.only().report_head.as_deref(), Some(start.as_str()), "the report is at the start");
            assert_eq!(
                place.call("merge", &[("child", &child.id), ("expected_head", &start)]).await,
                DelegationResponse::refusal(DelegationReasonCode::MergeHumanOnly),
                "merges follow the parent's agent"
            );
        })
        .await;
    }

    /// "delegation: an unknown or unavailable agent, or a mode it cannot take, is refused and nothing is made"
    #[tokio::test]
    async fn an_unknown_or_unavailable_agent_or_a_mode_it_cannot_take_is_refused_and_nothing_is_made() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let place = cross("claude", "acceptEdits").await;
            place.host.set_agent("gemini", DelegationAgentAvailability::NotInstalled);
            place.host.set_agent("codex", DelegationAgentAvailability::SignedOut);
            let before = place.bytes();
            let unknown = place.call("delegate", &[("task", "Do it."), ("mode", "acceptEdits"), ("agent", "gpt")]).await;
            assert!(
                unknown.refused == Some(DelegationReasonCode::AgentUnknown) && unknown.error.is_some_and(|e| e.contains("\"gpt\"")),
                "an unknown agent"
            );
            assert_eq!(
                place.call("delegate", &[("task", "Do it."), ("mode", "acceptEdits"), ("agent", "antigravity")]).await,
                DelegationResponse::refusal_with(
                    DelegationReasonCode::AgentUnavailable,
                    "Antigravity is not installed on this PC; the user can check it in Mighty Code's Settings › Components"
                )
            );
            assert_eq!(
                place.call("delegate", &[("task", "Do it."), ("mode", "onRequest"), ("agent", "codex")]).await,
                DelegationResponse::refusal_with(
                    DelegationReasonCode::AgentUnavailable,
                    "Codex is not signed in on this PC; the user can check it in Mighty Code's Settings › Components"
                )
            );
            assert_eq!(
                place.call("delegate", &[("task", "Do it."), ("mode", "auto"), ("agent", "codex")]).await.error.as_deref(),
                Some("mode must be one of a Codex child's: manual, acceptEdits, onRequest, fullAccess.")
            );
            assert!(
                place
                    .call("delegate", &[("task", "Do it."), ("mode", "manual"), ("agent", "codex")])
                    .await
                    .error
                    .is_some_and(|e| e.starts_with("A Codex child in manual mode cannot change files"))
            );
            assert!(
                place
                    .call("delegate", &[("task", "Do it."), ("mode", "plan"), ("agent", "gemini")])
                    .await
                    .error
                    .is_some_and(|e| e.starts_with("An Antigravity child in plan mode cannot change files"))
            );
            place.host.set_agent("codex", DelegationAgentAvailability::Ready);
            assert_eq!(
                place.call("delegate", &[("task", "Do it."), ("mode", "acceptEdits"), ("agent", "codex")]).await,
                DelegationResponse::refusal(DelegationReasonCode::WiderMode),
                "Codex acceptEdits is wider"
            );
            assert_eq!(
                place.call("delegate", &[("task", "Do it."), ("mode", "fullAccess"), ("agent", "codex")]).await,
                DelegationResponse::refusal(DelegationReasonCode::WiderMode),
                "Codex fullAccess is wider"
            );
            place.nothing_made(&before);
            assert!(
                !place.host.availability_asked().contains(&"claude".to_string()),
                "an agent the parent runs itself is never asked about"
            );
        })
        .await;
    }

    /// "delegation: the cap counts every agent, and the same call for another agent is another child"
    #[tokio::test]
    async fn the_cap_counts_every_agent_and_the_same_call_for_another_agent_is_another_child() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let place = cross("claude", "fullAccess").await;
            let own = place.call("delegate", &[("task", "Same task."), ("mode", "acceptEdits")]).await.child.unwrap();
            let codex = place.call("delegate", &[("task", "Same task."), ("mode", "acceptEdits"), ("agent", "codex")]).await.child.unwrap();
            assert!(
                own.id != codex.id && own.agent == "claude" && codex.agent == "codex",
                "the same call for another agent is another child"
            );
            assert_eq!(
                place.call("delegate", &[("task", "Same task."), ("mode", "acceptEdits"), ("agent", "claude")]).await.child.map(|c| c.id),
                Some(own.id.clone())
            );
            assert_eq!(
                place.call("delegate", &[("task", "Same task."), ("mode", "acceptEdits"), ("agent", " ")]).await.child.map(|c| c.id),
                Some(own.id.clone())
            );
            assert_eq!(
                place.call("delegate", &[("task", "Same task."), ("mode", "acceptEdits"), ("agent", "CODEX")]).await.child.map(|c| c.id),
                Some(codex.id.clone())
            );
            let gemini =
                place.call("delegate", &[("task", "Same task."), ("mode", "fullAccess"), ("agent", "antigravity")]).await.child.unwrap();
            assert!(gemini.agent == "antigravity" && gemini.starting_mode == "fullAccess");
            assert_eq!(
                place.call("delegate", &[("task", "A fourth."), ("mode", "acceptEdits"), ("agent", "codex")]).await,
                DelegationResponse::refusal(DelegationReasonCode::WidthCap),
                "three open children of any agents"
            );
            for info in [&own, &codex, &gemini] {
                assert!(place.wait_for(&info.id, ChildState::Running).await, "{} runs", info.agent);
            }
            let mut providers: Vec<String> = place.host.created_panes().into_iter().map(|p| p.provider).collect();
            providers.sort();
            assert_eq!(providers, ["claude", "codex", "gemini"], "each pane runs its agent");
        })
        .await;
    }

    /// "delegation: the human's menu starts a child of the agent it names, in that agent's widest mode"
    #[tokio::test]
    async fn the_humans_menu_starts_a_child_of_the_agent_it_names_in_that_agents_widest_mode() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let place = cross("claude", "auto").await;
            let codex = made(place.send("Try it in Codex.", Some("codex"), true).await);
            assert!(
                codex.provider.as_deref() == Some("codex") && codex.starting_mode == "acceptEdits" && codex.started_by_human == Some(true),
                "a Codex child in its widest mode"
            );
            let gemini = made(place.send("Try it in Antigravity.", Some("gemini"), false).await);
            assert!(gemini.provider.as_deref() == Some("gemini") && gemini.starting_mode == "acceptEdits");
            let claude = made(place.send("Plan it in Claude.", None, true).await);
            assert!(claude.provider.is_none() && claude.starting_mode == "plan", "Ctrl+Enter's Claude child in plan");
            for id in [&codex.id, &gemini.id, &claude.id] {
                assert!(place.wait_for(id, ChildState::Running).await);
            }
            let mut providers: Vec<String> = place.host.created_panes().into_iter().map(|p| p.provider).collect();
            providers.sort();
            assert!(
                providers == ["claude", "codex", "gemini"] && place.host.created_panes().iter().all(|p| p.started_by_human),
                "each pane runs its agent, the human's"
            );
            assert_eq!(
                place.send("One more.", Some("codex"), false).await,
                HumanChildStart::Refused(DelegationReasonCode::WidthCap),
                "the cap"
            );
            assert!(place.call("list_children", &[]).await.children.unwrap().is_empty(), "the parent lists none of them");
        })
        .await;
    }

    /// "delegation: the human's menu says why another agent cannot run the text"
    #[tokio::test]
    async fn the_humans_menu_says_why_another_agent_cannot_run_the_text() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let place = cross("claude", "manual").await;
            let before = place.bytes();
            assert_eq!(
                place.send("Try it.", Some("codex"), false).await,
                HumanChildStart::Failed(agent_words("delegation.human.error.agentNoMode", "Codex")),
                "no Codex mode fits"
            );
            place.host.set_agent("gemini", DelegationAgentAvailability::SignedOut);
            assert_eq!(
                place.send("Try it.", Some("gemini"), false).await,
                HumanChildStart::Failed(agent_words("delegation.human.error.agentSignedOut", "Antigravity"))
            );
            place.host.set_agent("codex", DelegationAgentAvailability::NotInstalled);
            assert_eq!(
                place.send("Try it.", Some("codex"), false).await,
                HumanChildStart::Failed(agent_words("delegation.human.error.agentNotInstalledWindows", "Codex"))
            );
            let words = agent_words("delegation.human.error.agentNotInstalledWindows", "Codex");
            assert!(
                words.contains("›") && !words.contains("› CLI") && !words.contains("Mac"),
                "the words point to Settings › 구성 요소 (no CLI tab) and name no Mac"
            );
            assert_eq!(
                place.send("Try it.", Some("other"), false).await,
                HumanChildStart::Refused(DelegationReasonCode::AgentUnknown),
                "no such agent"
            );
            place.nothing_made(&before);
            let read_only = cross("codex", "manual").await;
            let claude = made(read_only.send("Plan it.", Some("claude"), false).await);
            assert!(claude.provider.is_none() && claude.starting_mode == "plan", "a Claude plan child of a read-only Codex");
            assert_eq!(
                read_only.send("Try it.", Some("gemini"), false).await,
                HumanChildStart::Failed(agent_words("delegation.human.error.agentNoMode", "Antigravity"))
            );
        })
        .await;
    }

    /// "delegation: the human's child of the pane's own agent never asks about its CLI"
    #[tokio::test]
    async fn the_humans_child_of_the_panes_own_agent_never_asks_about_its_cli() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let place = cross("claude", "auto").await;
            place.host.set_agent("claude", DelegationAgentAvailability::NotInstalled);
            let first = made(place.send("First.", None, false).await);
            let second = made(place.send("Second.", Some("claude"), false).await);
            for id in [&first.id, &second.id] {
                assert!(place.wait_for(id, ChildState::Running).await);
            }
            assert!(place.host.availability_asked().is_empty(), "the pane's own agent is never asked about");
            let _ = place.send("Third.", Some("codex"), false).await;
            assert_eq!(place.host.availability_asked(), ["codex"], "Codex asked once");
        })
        .await;
    }
}
