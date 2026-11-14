//! The app side of delegation as the coordinator reaches it (C# DelegationCoordinator.cs:45-98, `IDelegationHost`,
//! `DelegationChildPane`, `DelegationPaneActivity`, `DelegationPaneState`; DelegationProviders.cs `DelegationAgentAvailability`;
//! M/DelegationCoordinator.swift:83-150). Owner: W7-A (the trait's signatures are final from W7 Step 0: W7-D implements it in
//! [`crate::app`], W7-A's tests fake it in `tests/support/host.rs`).

use mighty_platform::BoxFuture;

use crate::model::DeliveryRoute;

/// `DelegationChildPane`: a child pane the coordinator asks the host to open. `session_id` is chosen by the coordinator, so the
/// branch `mighty/<session id>` and the worktree exist before the pane does; `mode` is the starting permission mode the parent asked
/// for, written once; `folder` the folder the child works in, inside its worktree; `started_by_human`: the human started it from the
/// parent's composer; `provider`: the agent CLI the child runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationChildPane {
    pub session_id: String,
    pub parent_session_id: String,
    pub mode: String,
    pub folder: String,
    pub started_by_human: bool,
    pub provider: String,
}

/// `DelegationPaneActivity`: what a pane is doing, as delivery needs to know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DelegationPaneActivity {
    /// A run is going.
    Running,
    /// Idle after a run that finished normally since this launch, and a run can start in it now.
    Finished,
    /// Idle any other way: stopped, errored, no run since this launch, or no run can start in it now.
    #[default]
    Idle,
}

/// `DelegationPaneState`: one open pane as the host sees it now. `permission_mode`: its stored mode; `folder`: where its runs work;
/// `parent_session_id`: the parent when this pane is a delegated child; `run_id`: the run going now, or the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationPaneState {
    pub session_id: String,
    pub permission_mode: String,
    pub folder: String,
    pub kind: String,
    pub provider: String,
    pub parent_session_id: Option<String>,
    pub run_id: Option<String>,
    pub activity: DelegationPaneActivity,
}

/// `DelegationAgentAvailability`: whether an agent's CLI can run a child, as the app last learnt it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DelegationAgentAvailability {
    #[default]
    Ready,
    /// The CLI was not found, or says it is not installed.
    NotInstalled,
    /// The CLI says it is signed out.
    SignedOut,
}

/// `IDelegationHost`: the desktop service in the app, a fake in tests. The coordinator reaches panes only through this, and calls it
/// from a task of its own, never from inside its locked section. A host that cannot take a call answers false or None (C#: "a host that
/// throws is a host that did not take the call").
pub trait DelegationHost: Send + Sync {
    /// Opens the child's pane of `pane.provider` in `pane.mode`, working in `pane.folder` and linked to its parent. False when no pane
    /// could be made.
    fn create_pane(&self, pane: DelegationChildPane) -> BoxFuture<'_, bool>;
    /// Starts a run with `input` in the pane: the new run's id, or None when none started.
    fn start_run(&self, session_id: String, input: String) -> BoxFuture<'_, Option<String>>;
    /// Hands `input` to the pane by `route` (steered into its running Claude run, or as its next run): the id of the run that got it,
    /// or None when it did not get it.
    fn deliver(&self, input: String, session_id: String, route: DeliveryRoute) -> BoxFuture<'_, Option<String>>;
    /// The pane now, or None when it is closed.
    fn pane_state(&self, session_id: String) -> BoxFuture<'_, Option<DelegationPaneState>>;
    /// Stops the pane's run, if one is going, and returns once it has ended. Does nothing for an idle or closed pane.
    fn stop_run(&self, session_id: String) -> BoxFuture<'_, ()>;
    /// Shows `text` in the pane's transcript as the app's note. Does nothing for a closed pane; a host that shows no notes keeps this.
    fn note(&self, text: String, session_id: String) -> BoxFuture<'_, ()> {
        let _ = (text, session_id);
        Box::pin(async {})
    }
    /// Whether the `provider` agent's CLI can run a child, as the app last learnt it (the app never asks a CLI for this); a host that
    /// knows nothing against any agent keeps this.
    fn agent_availability(&self, provider: String) -> BoxFuture<'_, DelegationAgentAvailability> {
        let _ = provider;
        Box::pin(async { DelegationAgentAvailability::Ready })
    }
}
