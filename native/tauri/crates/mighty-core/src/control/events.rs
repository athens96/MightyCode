//! A pane's state, the transitions that are announced, and the events of the app's bus (MC-9). MC-32's schedules and triggers read
//! the same events.

use serde_json::{Map, Value, json};

/// What a pane is doing, as scripts and hooks see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneState {
    /// Nothing ran yet, or nothing runs and the last run left no outcome.
    Idle,
    Running,
    /// A run waits for the person: an approval or a question.
    Blocked,
    Done,
    Failed,
    Stopped,
}

/// Why a blocked pane waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockReason {
    Approval,
    Question,
}

impl BlockReason {
    pub fn name(self) -> &'static str {
        match self {
            BlockReason::Approval => "approval",
            BlockReason::Question => "question",
        }
    }
}

impl PaneState {
    pub const ALL: [PaneState; 6] =
        [PaneState::Idle, PaneState::Running, PaneState::Blocked, PaneState::Done, PaneState::Failed, PaneState::Stopped];

    pub fn name(self) -> &'static str {
        match self {
            PaneState::Idle => "idle",
            PaneState::Running => "running",
            PaneState::Blocked => "blocked",
            PaneState::Done => "done",
            PaneState::Failed => "failed",
            PaneState::Stopped => "stopped",
        }
    }

    pub fn parse(text: &str) -> Option<PaneState> {
        PaneState::ALL.into_iter().find(|state| state.name() == text)
    }

    /// The state of a pane from its saved status (`running`, `completed`, `error`, `stopped`, `idle`, …), whether a run of it is
    /// live or about to start, and the card it waits on, if any. A card wins over everything; a live run over the saved status.
    pub fn of(status: &str, live: bool, blocked: Option<BlockReason>) -> PaneState {
        if blocked.is_some() {
            return PaneState::Blocked;
        }
        if live || status == "running" {
            return PaneState::Running;
        }
        match status {
            "completed" => PaneState::Done,
            "error" => PaneState::Failed,
            "stopped" => PaneState::Stopped,
            _ => PaneState::Idle,
        }
    }

    /// Whether a run ended in this state.
    pub fn finished(self) -> bool {
        matches!(self, PaneState::Done | PaneState::Failed | PaneState::Stopped)
    }

    /// Whether nothing runs and nothing waits.
    pub fn quiet(self) -> bool {
        self == PaneState::Idle || self.finished()
    }
}

/// A transition hooks and webhooks can follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateEvent {
    /// A run began (from anything but a card: an answered card is the same run going on).
    Started,
    /// A run waits for an approval or an answer.
    Blocked,
    Done,
    Failed,
    Stopped,
    /// MC-32: a schedule's or trigger's run settled, or a fire was skipped (`automation.finished`, `automation.skipped`). Off until
    /// chosen.
    Automation,
}

impl StateEvent {
    pub const ALL: [StateEvent; 6] =
        [StateEvent::Started, StateEvent::Blocked, StateEvent::Done, StateEvent::Failed, StateEvent::Stopped, StateEvent::Automation];
    /// What settings choose until the person changes it: the moments that need someone.
    pub const DEFAULT: [StateEvent; 3] = [StateEvent::Blocked, StateEvent::Done, StateEvent::Failed];

    pub fn name(self) -> &'static str {
        match self {
            StateEvent::Started => "started",
            StateEvent::Blocked => "blocked",
            StateEvent::Done => "done",
            StateEvent::Failed => "failed",
            StateEvent::Stopped => "stopped",
            StateEvent::Automation => "automation",
        }
    }

    pub fn parse(text: &str) -> Option<StateEvent> {
        StateEvent::ALL.into_iter().find(|event| event.name() == text.trim())
    }

    /// A list of names (comma- or space-separated), unknown ones dropped, each once, in [`StateEvent::ALL`]'s order.
    pub fn parse_list(text: &str) -> Vec<StateEvent> {
        let named: Vec<StateEvent> = text.split([',', ' ']).filter_map(StateEvent::parse).collect();
        StateEvent::ALL.into_iter().filter(|event| named.contains(event)).collect()
    }

    /// `list` written back as [`StateEvent::parse_list`] reads it.
    pub fn list_text(list: &[StateEvent]) -> String {
        StateEvent::ALL.into_iter().filter(|event| list.contains(event)).map(StateEvent::name).collect::<Vec<_>>().join(",")
    }
}

/// The event a move from `from` to `to` announces, if any.
pub fn transition(from: PaneState, to: PaneState) -> Option<StateEvent> {
    if from == to {
        return None;
    }
    match to {
        PaneState::Running if from != PaneState::Blocked => Some(StateEvent::Started),
        PaneState::Blocked => Some(StateEvent::Blocked),
        PaneState::Done => Some(StateEvent::Done),
        PaneState::Failed => Some(StateEvent::Failed),
        PaneState::Stopped => Some(StateEvent::Stopped),
        _ => None,
    }
}

/// What `agent wait --until` waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitUntil {
    /// Nothing runs and nothing waits (a finished pane is idle too).
    Idle,
    Blocked,
    /// A run ended (done, failed or stopped).
    Done,
}

impl WaitUntil {
    pub fn parse(text: &str) -> Option<WaitUntil> {
        match text {
            "idle" => Some(WaitUntil::Idle),
            "blocked" => Some(WaitUntil::Blocked),
            "done" => Some(WaitUntil::Done),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            WaitUntil::Idle => "idle",
            WaitUntil::Blocked => "blocked",
            WaitUntil::Done => "done",
        }
    }

    pub fn reached(self, state: PaneState) -> bool {
        match self {
            WaitUntil::Idle => state.quiet(),
            WaitUntil::Blocked => state == PaneState::Blocked,
            WaitUntil::Done => state.finished(),
        }
    }
}

/// The pane an event is about: ids and names only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaneRef {
    pub pane: String,
    pub workspace: String,
    pub title: String,
    pub workspace_name: String,
    /// The agent's provider id (`claude`, `codex`, …); empty for a terminal.
    pub agent: String,
    /// `agent`, `terminal`, or the pane's own kind.
    pub kind: String,
}

impl PaneRef {
    pub fn is_agent(&self) -> bool {
        self.kind == "agent"
    }
}

/// The kind a pane is shown as: an agent pane, a terminal, or what the state calls it.
pub fn pane_kind(kind: &str) -> &str {
    match kind {
        "claude" => "agent",
        "shell" => "terminal",
        other => other,
    }
}

/// What happened.
#[derive(Debug, Clone, PartialEq)]
pub enum EventBody {
    State {
        from: PaneState,
        to: PaneState,
        event: Option<StateEvent>,
        reason: Option<BlockReason>,
    },
    /// The pane was marked as waiting (MC-26's kinds: `notify`, `bell`, `approval`, `question`, `setupFailed`).
    Attention {
        kind: String,
    },
    /// A run ended: its status, what it cost when the agent said, how long it took.
    RunCompleted {
        status: String,
        cost_usd: Option<f64>,
        duration_ms: Option<i64>,
    },
    /// MC-32: a schedule or trigger fired, was skipped, waits for the person, or its run settled. Names and states only, never the
    /// prompt or what the event carried.
    Automation(AutomationNote),
}

/// What an `automation.*` event says.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AutomationNote {
    /// `fired`, `skipped`, `blocked` or `finished`.
    pub event: String,
    /// The schedule's or trigger's name.
    pub name: String,
    /// `schedule`, `github`, `linear`, `webhook` or `standing`.
    pub kind: String,
    /// The run record's id.
    pub record: String,
    /// The record's state (`running`, `done`, `failed`, `skipped`, …).
    pub state: String,
    pub cost_usd: Option<f64>,
    pub url: Option<String>,
}

impl AutomationNote {
    pub const EVENTS: [&'static str; 4] = ["fired", "skipped", "blocked", "finished"];
}

/// One event of the bus.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlEvent {
    /// Rising by one per event of this app run.
    pub seq: u64,
    /// RFC 3339.
    pub at: String,
    pub pane: PaneRef,
    pub body: EventBody,
}

impl ControlEvent {
    pub const TYPES: [&'static str; 7] =
        ["pane.state", "attention", "run.completed", "automation.fired", "automation.skipped", "automation.blocked", "automation.finished"];

    pub fn type_name(&self) -> &'static str {
        match &self.body {
            EventBody::State { .. } => "pane.state",
            EventBody::Attention { .. } => "attention",
            EventBody::RunCompleted { .. } => "run.completed",
            EventBody::Automation(note) => match note.event.as_str() {
                "fired" => "automation.fired",
                "skipped" => "automation.skipped",
                "blocked" => "automation.blocked",
                _ => "automation.finished",
            },
        }
    }

    /// The state event this announces, if it is one (an automation run's end or skip announces [`StateEvent::Automation`]).
    pub fn state_event(&self) -> Option<StateEvent> {
        match &self.body {
            EventBody::State { event, .. } => *event,
            EventBody::Automation(note) if matches!(note.event.as_str(), "finished" | "skipped") => Some(StateEvent::Automation),
            _ => None,
        }
    }

    /// Whether this is an automation event (MC-32), which may name no pane.
    pub fn is_automation(&self) -> bool {
        matches!(self.body, EventBody::Automation(_))
    }

    /// The JSON scripts read: the type, the time, the pane's ids and names, then what happened.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("type".into(), json!(self.type_name()));
        out.insert("seq".into(), json!(self.seq));
        out.insert("at".into(), json!(self.at));
        out.insert("pane".into(), json!(self.pane.pane));
        out.insert("paneTitle".into(), json!(self.pane.title));
        out.insert("paneKind".into(), json!(self.pane.kind));
        out.insert("workspace".into(), json!(self.pane.workspace));
        out.insert("workspaceName".into(), json!(self.pane.workspace_name));
        out.insert("agent".into(), if self.pane.agent.is_empty() { Value::Null } else { json!(self.pane.agent) });
        match &self.body {
            EventBody::State { from, to, event, reason } => {
                out.insert("from".into(), json!(from.name()));
                out.insert("state".into(), json!(to.name()));
                out.insert("event".into(), event.map_or(Value::Null, |e| json!(e.name())));
                out.insert("reason".into(), reason.map_or(Value::Null, |r| json!(r.name())));
            }
            EventBody::Attention { kind } => {
                out.insert("kind".into(), json!(kind));
            }
            EventBody::RunCompleted { status, cost_usd, duration_ms } => {
                out.insert("status".into(), json!(status));
                out.insert("costUsd".into(), cost_usd.map_or(Value::Null, |c| json!(c)));
                out.insert("durationMs".into(), duration_ms.map_or(Value::Null, |d| json!(d)));
            }
            EventBody::Automation(note) => {
                out.insert("name".into(), json!(note.name));
                out.insert("kind".into(), json!(note.kind));
                out.insert("record".into(), json!(note.record));
                out.insert("state".into(), json!(note.state));
                out.insert("costUsd".into(), note.cost_usd.map_or(Value::Null, |c| json!(c)));
                out.insert("url".into(), note.url.as_ref().map_or(Value::Null, |u| json!(u)));
            }
        }
        Value::Object(out)
    }
}

/// A subscriber's filter: only these panes and types (empty: all).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventFilter {
    pub pane: Option<String>,
    pub types: Vec<String>,
}

impl EventFilter {
    pub fn admits(&self, event: &ControlEvent) -> bool {
        self.pane.as_deref().is_none_or(|pane| pane == event.pane.pane)
            && (self.types.is_empty() || self.types.iter().any(|t| t == event.type_name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_state_comes_from_its_status_its_run_and_its_card() {
        assert_eq!(PaneState::of("idle", false, None), PaneState::Idle);
        assert_eq!(PaneState::of("", false, None), PaneState::Idle);
        assert_eq!(PaneState::of("running", false, None), PaneState::Running);
        assert_eq!(PaneState::of("completed", true, None), PaneState::Running, "a live run wins over the saved status");
        assert_eq!(PaneState::of("completed", false, None), PaneState::Done);
        assert_eq!(PaneState::of("error", false, None), PaneState::Failed);
        assert_eq!(PaneState::of("stopped", false, None), PaneState::Stopped);
        assert_eq!(PaneState::of("running", true, Some(BlockReason::Question)), PaneState::Blocked);
        for state in PaneState::ALL {
            assert_eq!(PaneState::parse(state.name()), Some(state));
        }
    }

    #[test]
    fn transitions_announce_starts_cards_and_ends_once() {
        use PaneState::*;
        assert_eq!(transition(Idle, Running), Some(StateEvent::Started));
        assert_eq!(transition(Done, Running), Some(StateEvent::Started));
        assert_eq!(transition(Blocked, Running), None, "an answered card is the same run");
        assert_eq!(transition(Running, Blocked), Some(StateEvent::Blocked));
        assert_eq!(transition(Running, Done), Some(StateEvent::Done));
        assert_eq!(transition(Blocked, Failed), Some(StateEvent::Failed));
        assert_eq!(transition(Running, Stopped), Some(StateEvent::Stopped));
        assert_eq!(transition(Running, Running), None);
        assert_eq!(transition(Done, Idle), None);
    }

    #[test]
    fn event_lists_read_and_write_back_in_one_order() {
        assert_eq!(StateEvent::parse_list("done, blocked,nope,done"), vec![StateEvent::Blocked, StateEvent::Done]);
        assert_eq!(StateEvent::parse_list(""), vec![]);
        assert_eq!(StateEvent::list_text(&StateEvent::DEFAULT), "blocked,done,failed");
        assert_eq!(StateEvent::parse_list(&StateEvent::list_text(&StateEvent::ALL)), StateEvent::ALL.to_vec());
    }

    #[test]
    fn waits_reach_their_state() {
        use PaneState::*;
        assert!(WaitUntil::Idle.reached(Idle) && WaitUntil::Idle.reached(Done) && WaitUntil::Idle.reached(Failed));
        assert!(!WaitUntil::Idle.reached(Running) && !WaitUntil::Idle.reached(Blocked));
        assert!(WaitUntil::Blocked.reached(Blocked) && !WaitUntil::Blocked.reached(Running));
        assert!(WaitUntil::Done.reached(Done) && WaitUntil::Done.reached(Failed) && WaitUntil::Done.reached(Stopped));
        assert!(!WaitUntil::Done.reached(Idle) && !WaitUntil::Done.reached(Running));
        assert_eq!(WaitUntil::parse("blocked"), Some(WaitUntil::Blocked));
        assert_eq!(WaitUntil::parse("busy"), None);
    }

    #[test]
    fn an_events_json_names_the_pane_and_what_happened_only() {
        let pane = PaneRef {
            pane: "p1".into(),
            workspace: "w1".into(),
            title: "Fix tests".into(),
            workspace_name: "app".into(),
            agent: "claude".into(),
            kind: "agent".into(),
        };
        let event = ControlEvent {
            seq: 7,
            at: "2026-11-23T10:00:00+00:00".into(),
            pane: pane.clone(),
            body: EventBody::State {
                from: PaneState::Running,
                to: PaneState::Blocked,
                event: Some(StateEvent::Blocked),
                reason: Some(BlockReason::Approval),
            },
        };
        let value = event.to_json();
        assert_eq!(value["type"], "pane.state");
        assert_eq!(value["seq"], 7);
        assert_eq!(value["pane"], "p1");
        assert_eq!(value["paneTitle"], "Fix tests");
        assert_eq!(value["workspaceName"], "app");
        assert_eq!(value["from"], "running");
        assert_eq!(value["state"], "blocked");
        assert_eq!(value["event"], "blocked");
        assert_eq!(value["reason"], "approval");
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "type",
                "seq",
                "at",
                "pane",
                "paneTitle",
                "paneKind",
                "workspace",
                "workspaceName",
                "agent",
                "from",
                "state",
                "event",
                "reason"
            ]
        );
        let done = ControlEvent {
            body: EventBody::RunCompleted { status: "completed".into(), cost_usd: Some(0.25), duration_ms: Some(1200) },
            ..event.clone()
        };
        assert_eq!(done.to_json()["costUsd"], 0.25);
        assert_eq!(done.type_name(), "run.completed");
        let filter = EventFilter { pane: Some("p1".into()), types: vec!["run.completed".into()] };
        assert!(filter.admits(&done) && !filter.admits(&event));
        assert!(!EventFilter { pane: Some("p2".into()), types: vec![] }.admits(&done));
        assert!(EventFilter::default().admits(&event));
        let fired = ControlEvent {
            body: EventBody::Automation(AutomationNote {
                event: "finished".into(),
                name: "deps".into(),
                kind: "schedule".into(),
                record: "r1".into(),
                state: "done".into(),
                cost_usd: Some(0.5),
                url: Some("https://github.com/a/b/pull/1".into()),
            }),
            ..event.clone()
        };
        assert_eq!(fired.type_name(), "automation.finished");
        assert_eq!(fired.state_event(), Some(StateEvent::Automation));
        assert!(fired.is_automation() && !event.is_automation());
        let value = fired.to_json();
        assert_eq!((value["name"].as_str(), value["state"].as_str(), value["costUsd"].as_f64()), (Some("deps"), Some("done"), Some(0.5)));
        assert!(ControlEvent::TYPES.contains(&fired.type_name()));
        assert!(EventFilter { pane: None, types: vec!["automation.finished".into()] }.admits(&fired));
    }
}
