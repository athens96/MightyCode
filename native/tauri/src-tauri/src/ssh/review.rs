//! MC-40: 요청 변경 검토 and 바뀐 부분별 받기·버리기 for a pane of an SSH workspace: its requests' snapshots on the far side
//! (mighty-files `remote_review`), read and written over the machine's ssh off the page's thread, with the same rules and the same
//! answers as a local pane's (files.rs `runChanges*`). The decisions are kept in the files pane's `hunk-reviews` folder, named by the
//! machine, the remote folder and the pane.

use std::sync::Arc;

use mighty_files::remote_review::{RemoteReview, SshExec};

use crate::host::Host;

/// The review of `pane` when it is a pane of an SSH workspace: Some(None) when its machine cannot be reached or it has no snapshots,
/// None for any other pane. Reads the far side: call it off the page's thread (it waits on ssh).
pub fn load(host: &Arc<Host>, pane: &str, handle: tokio::runtime::Handle) -> Option<Option<RemoteReview>> {
    let (workspace, working) = {
        let view = host.view();
        let session = view.snapshot.session(pane)?;
        (view.snapshot.workspace(session.workspace_id()).filter(|w| w.is_ssh()).cloned()?, session.working_folder.clone())
    };
    let reference = workspace.ssh.clone()?;
    let Ok(mut launch) = super::launch_for(host, &workspace) else { return Some(None) };
    // MC-47: a pane in its own worktree on the machine keeps its requests' snapshots under that worktree's folder.
    if let Some(folder) = working.filter(|f| mighty_core::ssh::valid_remote_dir(f)) {
        launch.dir = folder;
    }
    let Ok(files) = crate::files::state(host) else { return Some(None) };
    let dir = launch.dir.clone();
    let exec = Arc::new(SshExec { processes: host.processes.clone(), launch, handle });
    let scope = format!("ssh\n{}\n{}\n{pane}", reference.machine, dir);
    Some(RemoteReview::load(exec, &dir, pane, files.reviews.clone(), scope))
}

/// Whether `pane` is a pane of an SSH workspace.
pub fn is_ssh_pane(host: &Host, pane: &str) -> bool {
    let view = host.view();
    view.snapshot.session(pane).and_then(|s| view.snapshot.workspace(s.workspace_id())).is_some_and(|w| w.is_ssh())
}

/// The folder a review of an SSH pane names (`host:dir`).
pub fn shown_root(host: &Host, pane: &str) -> String {
    let view = host.view();
    view.snapshot
        .session(pane)
        .and_then(|s| {
            let reference = view.snapshot.workspace(s.workspace_id())?.ssh.as_ref()?;
            let dir = s.working_folder.as_deref().filter(|f| mighty_core::ssh::valid_remote_dir(f)).unwrap_or(&reference.dir);
            Some(format!("{}:{dir}", reference.host))
        })
        .unwrap_or_default()
}

/// The review queue's reading of an SSH pane: the newest of `candidates` that changed files, with its counts (files.rs
/// `request_changes`'s, over the snapshots). Waits on ssh: off the page's thread.
pub fn request_changes(
    host: &Arc<Host>,
    pane: &str,
    candidates: &[String],
    handle: tokio::runtime::Handle,
) -> Option<(String, crate::files::RequestCounts)> {
    let review = load(host, pane, handle)??;
    let changed = review.changed_runs();
    candidates.iter().filter(|run| changed.contains(run)).find_map(|run| {
        review.prefetch(run);
        let items = mighty_files::revert::plan_on(&review, &review, run);
        crate::files::counts(&items).map(|counts| (run.clone(), counts))
    })
}
