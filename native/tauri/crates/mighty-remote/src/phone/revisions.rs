//! Revision bookkeeping for the m1 API (C# R\MobileRevisions.cs; M/MightyCode/AppStore+MobileRemote.swift:11-49, 271-323): a session's
//! revision moves when anything the phone shows of it changed; the state revision moves only when a summary (status, title, preview,
//! pending counts), the pane order, a workspace or the app update changed, so the phone's list is not sent again for every reply. Both
//! start at 1; `state` is told before `session:<id>`. Owner: W8-B.

use std::collections::HashMap;

use mighty_core::model::{RunSession, Workspace};

/// `MobileRevisions.Reading(Pane, Editable, Options, Mighty, Summary, Held)`: one pane as it stands. `summary` is the pane's summary
/// as it goes out without its revision; `held` the ids of the rows delegation holds in its queued list, in order.
#[derive(Debug, Clone)]
pub struct Reading<'a> {
    pub pane: &'a RunSession,
    pub editable: bool,
    pub options: i32,
    pub mighty: i32,
    pub summary: String,
    pub held: String,
}

/// `Fingerprint`: a cheap change detector for a pane, its metadata and the tail of its transcript (never the status line).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    status: String,
    title: String,
    provider: String,
    model: String,
    resume_id: Option<String>,
    count: usize,
    tail: String,
    usage: Option<String>,
    settings: String,
    view: String,
    limits: Option<String>,
    editable: bool,
    options: i32,
    mighty: i32,
    held: String,
}

impl Fingerprint {
    fn of(reading: &Reading) -> Fingerprint {
        let pane = reading.pane;
        let logs = pane.logs();
        let tail = logs[logs.len().saturating_sub(12)..]
            .iter()
            .map(|e| format!("{}:{}:{}", e.id(), e.text().encode_utf16().count(), e.activity.as_ref().map_or("", |a| a.state.as_str())))
            .collect::<Vec<_>>()
            .join("\n");
        let settings = pane.settings.clone().unwrap_or_default();
        Fingerprint {
            status: pane.status().into(),
            title: pane.title().into(),
            provider: pane.provider().into(),
            model: pane.model().into(),
            resume_id: pane.resume_id.clone(),
            count: logs.len(),
            tail,
            usage: pane.session_usage.as_ref().map(|u| u.updated_at.clone()),
            settings: format!("{}|{}", settings.effort, settings.permission_mode),
            view: format!("{}|{}", pane.agent_view_mode.as_deref().unwrap_or(""), pane.mighty_style.as_deref().unwrap_or("")),
            limits: pane.session_usage.as_ref().and_then(|u| u.rate_limits_updated_at.clone()),
            editable: reading.editable,
            options: reading.options,
            mighty: reading.mighty,
            held: reading.held.clone(),
        }
    }
}

/// `MobileRevisions`.
#[derive(Debug)]
pub struct MobileRevisions {
    sessions: HashMap<String, i64>,
    seen: HashMap<String, Fingerprint>,
    summaries: HashMap<String, (String, i64)>,
    order: Vec<String>,
    workspaces: String,
    update: String,
    state: i64,
}

impl Default for MobileRevisions {
    fn default() -> Self {
        MobileRevisions {
            sessions: HashMap::new(),
            seen: HashMap::new(),
            summaries: HashMap::new(),
            order: vec![],
            workspaces: String::new(),
            update: String::new(),
            state: 1,
        }
    }
}

impl MobileRevisions {
    pub fn new() -> MobileRevisions {
        MobileRevisions::default()
    }

    /// `State`.
    pub fn state(&self) -> i64 {
        self.state
    }

    /// `Session(id)`: 1 for a pane never read.
    pub fn session(&self, id: &str) -> i64 {
        self.sessions.get(id).copied().unwrap_or(1)
    }

    /// `Observe(spaces, panes, appUpdate)`: reads the panes again and returns the scopes whose revision moved — `state` first, then
    /// `session:<id>` for each pane. A pane whose summary alone moved has its own revision moved too.
    pub fn observe<'a>(
        &mut self,
        spaces: impl IntoIterator<Item = &'a Workspace>,
        panes: &[Reading],
        app_update: &str,
    ) -> Vec<(String, i64)> {
        let pane_order: Vec<String> = panes.iter().map(|p| p.pane.id().to_string()).collect();
        let workspace_key =
            spaces.into_iter().map(|w| format!("{}\u{1f}{}\u{1f}{}", w.id(), w.name(), w.path())).collect::<Vec<_>>().join("\n");
        let mut state_changed = workspace_key != self.workspaces || app_update != self.update || self.order != pane_order;
        let mut changed = vec![];
        let mut next_seen = HashMap::new();
        let mut next_summaries = HashMap::new();
        for reading in panes {
            let id = reading.pane.id().to_string();
            let fingerprint = Fingerprint::of(reading);
            let moved = self.seen.get(&id) != Some(&fingerprint);
            if moved {
                *self.sessions.entry(id.clone()).or_insert(0) += 1;
            }
            let previous = self.summaries.get(&id);
            if previous.is_none_or(|(summary, _)| *summary != reading.summary) {
                state_changed = true;
                if !moved {
                    *self.sessions.entry(id.clone()).or_insert(0) += 1;
                }
            }
            let revision = self.session(&id);
            if previous.is_none_or(|(_, before)| *before != revision) {
                changed.push(id.clone());
            }
            next_seen.insert(id.clone(), fingerprint);
            next_summaries.insert(id, (reading.summary.clone(), revision));
        }
        let gone: Vec<String> = self.summaries.keys().filter(|id| !next_summaries.contains_key(*id)).cloned().collect();
        for id in gone {
            self.sessions.remove(&id);
        }
        self.seen = next_seen;
        self.summaries = next_summaries;
        self.workspaces = workspace_key;
        self.update = app_update.to_string();
        self.order = pane_order;
        if state_changed {
            self.state += 1;
        }
        let mut result = vec![];
        if state_changed {
            result.push(("state".to_string(), self.state));
        }
        result.extend(changed.into_iter().map(|id| {
            let revision = self.session(&id);
            (format!("session:{id}"), revision)
        }));
        result
    }
}
