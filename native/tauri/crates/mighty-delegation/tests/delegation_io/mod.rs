//! What the delegation server's pipe checks share (C# DelegationAppVerification's `Canned`, `Gated`, `DelegationIOHandler`): handlers
//! that answer a delegation call by its tool, wait to be opened, or gate it as the switch and the caller say. W7-C.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indexmap::IndexMap;
use mighty_delegation::model::{ChildState, DeliveryLane, MergeKind, MergeRecord};
use mighty_delegation::request::{
    DETACHED_MESSAGE, DelegationCaller, DelegationChildInfo, DelegationChildStatus, DelegationChildSummary, DelegationFollowUpInfo,
    DelegationHandler, DelegationReasonCode, DelegationRequest, DelegationResponse, unknown_tool_message,
};
use mighty_platform::{AgentIoConnector, AgentIoListener, BoxFuture};
use tokio::sync::Notify;

/// The OS's own Agent IO transport: Windows' named pipe, the Mac's Unix socket (M-C) in a fresh profile folder of its own.
#[cfg(windows)]
pub fn os_transport() -> (Arc<dyn AgentIoListener>, Arc<dyn AgentIoConnector>) {
    (Arc::new(mighty_platform_windows::NamedPipeListener::new()), Arc::new(mighty_platform_windows::NamedPipeConnector))
}

/// The OS's own Agent IO transport: Windows' named pipe, the Mac's Unix socket (M-C) in a fresh profile folder of its own.
#[cfg(target_os = "macos")]
pub fn os_transport() -> (Arc<dyn AgentIoListener>, Arc<dyn AgentIoConnector>) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos();
    let profile = std::env::temp_dir().join(format!("mighty-io-{}-{nanos}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
    (
        Arc::new(mighty_platform_macos::agent_io::UnixSocketListener::new(&profile)),
        Arc::new(mighty_platform_macos::agent_io::UnixSocketConnector),
    )
}

/// `DelegationSupport.Commit`: a commit id.
pub const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// `Request(tool, (name, value)…)`.
pub fn request(tool: &str, arguments: &[(&str, &str)]) -> DelegationRequest {
    DelegationRequest {
        tool: tool.to_string(),
        arguments: arguments.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<IndexMap<_, _>>(),
    }
}

/// `DelegationIOHandler(isSwitchOn)`: the first three gates, then "Unknown delegation tool X." (no tool behind it).
pub struct IoHandler {
    pub on: bool,
}

impl IoHandler {
    /// `DelegationIOHandler.Gate`.
    pub fn gate(caller: &DelegationCaller, on: bool) -> Option<DelegationResponse> {
        if !on {
            return Some(DelegationResponse::failure(DETACHED_MESSAGE));
        }
        if !(caller.kind == "claude" && matches!(caller.provider.as_str(), "claude" | "codex")) {
            return Some(DelegationResponse::refusal(DelegationReasonCode::ClaudeOnly));
        }
        (!caller.delegation).then(|| DelegationResponse::failure(DETACHED_MESSAGE))
    }
}

impl DelegationHandler for IoHandler {
    fn handle(&self, request: DelegationRequest, caller: DelegationCaller) -> BoxFuture<'static, DelegationResponse> {
        let answer = IoHandler::gate(&caller, self.on).unwrap_or_else(|| DelegationResponse::failure(unknown_tool_message(&request.tool)));
        Box::pin(async move { answer })
    }
}

/// `Canned(delay)`: answers by the tool's name, so every answer's shape crosses the pipe; records each call and its caller.
pub struct Canned {
    pub delay: Duration,
    pub calls: Arc<Mutex<Vec<(DelegationRequest, DelegationCaller)>>>,
}

impl Canned {
    pub fn new(delay: Duration) -> Canned {
        Canned { delay, calls: Arc::default() }
    }

    /// `Canned.Answers`, in C#'s order.
    pub fn answers() -> Vec<(&'static str, DelegationResponse)> {
        let child = DelegationChildInfo {
            id: "c1".into(),
            state: ChildState::Creating,
            branch: "mighty/c1".into(),
            starting_mode: "plan".into(),
            agent: "claude".into(),
        };
        let summary = |id: &str, state, revision| DelegationChildSummary { id: id.into(), state, revision, agent: "claude".into() };
        vec![
            ("delegate", DelegationResponse { child: Some(child), ..Default::default() }),
            (
                "list_children",
                DelegationResponse {
                    children: Some(vec![summary("c1", ChildState::Reported, 2), summary("c2", ChildState::Running, 0)]),
                    ..Default::default()
                },
            ),
            (
                "child_status",
                DelegationResponse {
                    status: Some(DelegationChildStatus {
                        id: "c1".into(),
                        state: ChildState::Reported,
                        revision: 1,
                        head: Some(COMMIT.into()),
                        report: Some("# Report 한글".into()),
                        report_truncated: false,
                        agent: "claude".into(),
                    }),
                    ..Default::default()
                },
            ),
            (
                "merge",
                DelegationResponse {
                    merged: Some(MergeRecord {
                        child_id: "c1".into(),
                        kind: MergeKind::ToolFastForward,
                        parent_branch: "main".into(),
                        pre_merge_commit: COMMIT.into(),
                        merged_commit: "b".repeat(40),
                        child_head: "b".repeat(40),
                    }),
                    ..Default::default()
                },
            ),
            (
                "follow_up",
                DelegationResponse {
                    follow_up: Some(DelegationFollowUpInfo { id: "f1".into(), child: "c1".into(), lane: DeliveryLane::Held, remaining: 1 }),
                    ..Default::default()
                },
            ),
            ("discard", DelegationResponse::refusal(DelegationReasonCode::DiscardHumanOnly)),
            ("slow", DelegationResponse::failure("slow answer")),
        ]
    }
}

impl DelegationHandler for Canned {
    fn handle(&self, request: DelegationRequest, caller: DelegationCaller) -> BoxFuture<'static, DelegationResponse> {
        let tool = request.tool.clone();
        self.calls.lock().unwrap().push((request, caller));
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Canned::answers().into_iter().find(|(name, _)| *name == tool).map(|(_, answer)| answer).unwrap_or_default()
        })
    }
}

/// `Gated`: its calls wait until it is opened, then answer "released".
#[derive(Default)]
pub struct Gated {
    pub open: Arc<Notify>,
    pub opened: Arc<std::sync::atomic::AtomicBool>,
    pub entered: Arc<AtomicUsize>,
}

impl Gated {
    pub fn release(&self) {
        self.opened.store(true, Ordering::SeqCst);
        self.open.notify_waiters();
    }
}

impl DelegationHandler for Gated {
    fn handle(&self, _request: DelegationRequest, _caller: DelegationCaller) -> BoxFuture<'static, DelegationResponse> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let (open, opened) = (self.open.clone(), self.opened.clone());
        Box::pin(async move {
            loop {
                let notified = open.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if opened.load(Ordering::SeqCst) {
                    break;
                }
                notified.await;
            }
            DelegationResponse::failure("released")
        })
    }
}

/// `Verification.Until(condition, milliseconds)`: polls every 20 ms.
pub async fn until(condition: impl Fn() -> bool, milliseconds: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(milliseconds);
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline, "the condition did not hold in time");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A full path for the binding's executable (`Path.Combine(Path.GetTempPath(), "Mighty Code", "MightyCode.exe")`).
pub fn executable() -> String {
    std::env::temp_dir().join("Mighty Code").join("MightyCode.exe").to_string_lossy().into_owned()
}

pub fn temp() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}
