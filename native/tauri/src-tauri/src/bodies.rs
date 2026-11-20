//! The panes whose body the split state's load left on disk (docs/superpowers/plans/2026-11-20-session-state-files.md, phase 3): a
//! launch reads the active workspace's panes whole and the others' summaries only. A pane is read whole when something needs it: its
//! workspace becoming the active one (`View::sync`), a run starting on it or a change to it (the run service), the phone's or a
//! paired computer's look at it (`PhoneDesk::ensure_body`). The store never takes what such a pane holds for its whole body.

use std::sync::{Arc, Mutex};

use mighty_core::store::Store;
use mighty_run::service::BodyLoader;

use crate::host::{Host, lock};

/// The run service's reader of a pane's body: the store's, under its lock (a save waits, or the read waits for a save).
pub fn loader(store: Arc<Mutex<Store>>) -> BodyLoader {
    Arc::new(move |pane| lock(&store).load_pane_body(pane))
}

/// The panes in `ids` read whole if the load left their body on disk; the page's copy follows when one was.
pub fn ensure(host: &Arc<Host>, ids: &[&str]) {
    let Some(runs) = host.runs() else { return };
    if runs.service.ensure_bodies(ids) {
        host.view().sync();
    }
}
