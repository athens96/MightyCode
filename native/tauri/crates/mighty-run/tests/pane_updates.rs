//! `RunService::update_pane`, C#'s light updates of one pane (`DesktopService.SetShellStatus`, DesktopService.cs:165;
//! `UpdateShellBlockRun`, :219): ShellBlockRunVerification.TheServiceKeepsTheRecordWithThePane's service half — the pane's record
//! replaced without the whole state's normalize, nothing for a pane that is gone, the save each change asks for, and a full update
//! after it keeps it. The rules of the change itself (only an agent pane keeps shell block records, a shell pane takes its terminal's
//! status) are mighty-terminal's, checked there.

use std::collections::BTreeMap;
use std::sync::Arc;

use mighty_core::model::{AppSnapshot, RunSession, Workspace};
use mighty_platform::fake::{FakeProcessHost, FakeShellEnvironment};
use mighty_run::ServiceEvent;
use mighty_run::providers::ProviderCatalog;
use mighty_run::service::{PaneSave, RunService, RunServiceOptions};

fn service() -> (RunService, tokio::sync::mpsc::UnboundedReceiver<ServiceEvent>) {
    let workspace = Workspace { id: Some("w".into()), path: Some(std::env::temp_dir().display().to_string()), ..Workspace::default() };
    let pane = |id: &str, kind: &str| {
        Arc::new(RunSession { id: Some(id.into()), workspace_id: Some("w".into()), kind: Some(kind.into()), ..RunSession::default() })
    };
    let snapshot = AppSnapshot {
        workspaces: Some(vec![workspace]),
        sessions: Some(vec![pane("agent", "claude"), pane("shell", "shell")]),
        ..AppSnapshot::default()
    };
    let catalog =
        Arc::new(ProviderCatalog::new(Arc::new(FakeProcessHost::default()), Arc::new(FakeShellEnvironment::new(BTreeMap::new())), None));
    RunService::new(
        snapshot,
        catalog,
        RunServiceOptions {
            plugin_directory: String::new(),
            permission_card: false,
            remote: None,
            configure: None,
            admitting: None,
            binding: None,
            delegation: None,
        },
    )
}

#[tokio::test]
async fn the_service_replaces_one_pane_and_saves_as_asked() {
    let (service, mut events) = service();
    let status = |service: &RunService, id: &str| service.snapshot().session(id).map(|s| s.status().to_string());
    let changed = service.update_pane("shell", |pane| {
        let mut next = pane.clone();
        next.status = Some("running".into());
        Some((next, PaneSave::Later))
    });
    assert!(changed && status(&service, "shell").as_deref() == Some("running"), "the pane took the change");
    assert!(events.try_recv().is_err(), "a change saved later asks for no save");
    assert!(!service.update_pane("shell", |_| None), "no change");
    assert!(!service.update_pane("gone", |pane| Some((pane.clone(), PaneSave::Now))), "a pane that is gone");
    assert!(service.update_pane("agent", |pane| Some((pane.clone(), PaneSave::Soon))));
    assert!(matches!(events.try_recv(), Ok(ServiceEvent::SaveSoon)), "on the schedule");
    assert!(service.update_pane("shell", |pane| {
        let mut next = pane.clone();
        next.status = Some("completed".into());
        Some((next, PaneSave::Now))
    }));
    assert!(matches!(events.try_recv(), Ok(ServiceEvent::SaveNow)), "at once");
    service.update(|state| state.clone()).unwrap();
    assert_eq!(status(&service, "shell").as_deref(), Some("completed"), "a full update keeps it");
    service.dispose().await;
    assert!(!service.update_pane("shell", |pane| Some((pane.clone(), PaneSave::Now))), "a closing service changes nothing");
}

/// A pane the split state's load left on disk (its body read only when needed, mighty-core store.rs): holding one entry of three.
fn on_disk(service: &RunService) {
    use mighty_core::model::{LogEntry, SessionDigest};
    let entry = |id: &str| LogEntry { id: Some(id.into()), kind: Some("assistant".into()), text: Some(id.into()), ..LogEntry::default() };
    let tail = vec![entry("e3")];
    let mut snapshot = service.snapshot();
    let at = snapshot.sessions().iter().position(|s| s.id() == "agent").unwrap();
    let pane = RunSession {
        logs: Some(tail.clone()),
        body_on_disk: true,
        stored_digest: Some(Arc::new(SessionDigest { log_count: 3, tail, run_ids: vec![] })),
        ..(*snapshot.sessions()[at]).clone()
    };
    snapshot.sessions_mut()[at] = Arc::new(pane);
    service.replace(snapshot);
    service.set_body_loader(Arc::new(move |pane: &RunSession| {
        Some(RunSession {
            logs: Some(vec![entry("e1"), entry("e2"), entry("e3")]),
            body_on_disk: false,
            stored_digest: None,
            ..pane.clone()
        })
    }));
}

#[tokio::test]
async fn a_pane_whose_body_is_on_disk_is_read_whole_before_a_change_to_it() {
    let (service, _events) = service();
    on_disk(&service);
    assert!(service.snapshot().session("agent").unwrap().body_on_disk);
    assert!(service.update_pane("agent", |pane| {
        let mut next = pane.clone();
        next.status = Some("idle".into());
        Some((next, PaneSave::Later))
    }));
    let pane = service.snapshot().session("agent").unwrap().clone();
    assert!(!pane.body_on_disk);
    assert_eq!(pane.logs().iter().map(|e| e.id()).collect::<Vec<_>>(), ["e1", "e2", "e3"], "the whole history, then the change");
}

#[tokio::test]
async fn reading_a_workspaces_bodies_reads_only_those_still_on_disk() {
    let (service, _events) = service();
    on_disk(&service);
    assert!(service.ensure_workspace_bodies("w"));
    assert!(!service.ensure_workspace_bodies("w"), "nothing left to read");
    assert_eq!(service.snapshot().session("agent").unwrap().log_count(), 3);
}
