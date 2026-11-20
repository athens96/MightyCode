//! The pet's progress card over its bubble (the Mac's CompanionProgress.swift and `CompanionProgressCard` in
//! AgentCompanionViews.swift): the progress panel of the agent the bubble shows — its newest request (mighty-graph `progress_of`, the
//! page's own panel) — every step and piece of background work the panel holds, in a list as tall as eight lines at most that scrolls
//! inside the card (opened on the step in progress, as the page's panel scrolls in its own box); it folds to its heading (`progressCollapsed` in the settings). A request with neither steps nor background work has no card, and the
//! card goes with the task bubble: none while a request for the user takes the bubble or the bubble is put away. Its words and clocks
//! are read by the app once a second, as the bubble's are. Owner: W8-E.

use mighty_core::graph::MightyGraphRun;
use mighty_core::locale::Locale;
use mighty_core::time::DateTimeOffset;
use mighty_graph::flow::card::elapsed_words;
use mighty_graph::flow::steps::{self, ProgressState};
use mighty_graph::support;

/// How many lines the card's list shows before it scrolls: its height is at most this many lines and a section heading.
pub const VISIBLE_LINES: usize = 8;
/// The list's box at its tallest.
pub const MAXIMUM_LIST: f64 = VISIBLE_LINES as f64 * ROW_HEIGHT + SECTION_HEIGHT;
/// The heading, a line, the "외 N개" and "백그라운드" lines.
pub const HEADER_HEIGHT: f64 = 26.0;
pub const ROW_HEIGHT: f64 = 20.0;
pub const SECTION_HEIGHT: f64 = 18.0;
/// The card's padding under its last line, and the gap to the bubble.
pub const PADDING: f64 = 8.0;
pub const GAP: f64 = 6.0;

/// One line of the card: a step (`agent` None) or a piece of background work (a sub-agent or a command), its state and its time (how
/// long it took, or the time since it began while it runs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressLine {
    pub text: String,
    pub state: ProgressState,
    pub agent: Option<bool>,
    pub time: Option<String>,
}

/// The card's words, read once for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionProgress {
    /// "진행 상황".
    pub title: String,
    /// "요청 27".
    pub heading: String,
    /// "3/9", None without steps.
    pub count: Option<String>,
    pub state_word: String,
    pub status: String,
    pub running: bool,
    pub rows: Vec<ProgressLine>,
    /// The row the list opens on: the step in progress, else the last one done.
    pub focus: usize,
    /// "외 4개" for the steps the page's panel leaves out too (past its 100 lines).
    pub more: Option<String>,
    /// "백그라운드" and "1/3" (running of all of it), with the background work.
    pub background_title: String,
    pub background_count: String,
    pub background: Vec<ProgressLine>,
    pub background_more: Option<String>,
}

fn time(start: Option<DateTimeOffset>, end: Option<DateTimeOffset>, ticking: bool, now: DateTimeOffset, locale: &Locale) -> Option<String> {
    let start = start?;
    match end {
        None if ticking => Some(elapsed_words(start, now, locale)),
        Some(end) if end.utc_ticks() >= start.utc_ticks() => Some(elapsed_words(start, end, locale)),
        _ => None,
    }
}

fn more(count: usize, locale: &Locale) -> Option<String> {
    (count > 0).then(|| locale.get_with("graph.progress.more", &[("count", &count.to_string())]))
}

impl CompanionProgress {
    /// The card for `progress` at `now`; None when its request has no steps and no background work.
    pub fn make(progress: &steps::Progress, now: DateTimeOffset, locale: &Locale) -> Option<CompanionProgress> {
        if progress.rows.is_empty() && progress.background.is_empty() {
            return None;
        }
        let rows = &progress.rows;
        let focus = rows
            .iter()
            .position(|r| r.state == ProgressState::Current)
            .or_else(|| rows.iter().rposition(|r| r.state == ProgressState::Done))
            .unwrap_or(0);
        let shown: Vec<ProgressLine> = rows
            .iter()
            .map(|row| ProgressLine {
                text: row.text.clone(),
                state: row.state,
                agent: None,
                time: time(row.started_at, row.ended_at, row.state == ProgressState::Current && progress.running, now, locale),
            })
            .collect();
        let background: Vec<ProgressLine> = progress
            .background
            .iter()
            .map(|item| ProgressLine {
                text: item.text.clone(),
                state: item.state,
                agent: Some(item.agent),
                time: time(item.started_at, item.ended_at, item.state == ProgressState::Current, now, locale),
            })
            .collect();
        let all_background = progress.background.len() + progress.background_hidden;
        let running_background = progress.background.iter().filter(|b| b.state == ProgressState::Current).count();
        Some(CompanionProgress {
            title: progress.title.clone(),
            heading: progress.heading.clone(),
            count: progress.count.clone(),
            state_word: progress.state_word.clone(),
            status: progress.status.clone(),
            running: progress.running,
            rows: shown,
            focus,
            more: more(progress.hidden, locale),
            background_title: locale.get("graph.progress.background"),
            background_count: format!("{running_background}/{all_background}"),
            background_more: more(progress.background_hidden, locale),
            background,
        })
    }

    /// The card of the newest request among a pane's runs, headed by its request number as the page's panel is: `earlier` is how many
    /// requests the pane's older history holds before these runs (the page counts them too).
    pub fn newest(runs: &[MightyGraphRun], earlier: usize, now: DateTimeOffset, locale: &Locale) -> Option<CompanionProgress> {
        let shown = support::shown_runs(runs, locale);
        let shown: &[MightyGraphRun] = &shown;
        let index = support::newest_request_index(shown)?;
        let ordinal = support::request_ordinals(shown)[index] as usize + earlier;
        let heading = locale.get_with("graph.timeline.requestOrdinal", &[("n", &ordinal.to_string())]);
        CompanionProgress::make(&steps::progress_of(index, shown, &heading, locale), now, locale)
    }

    /// How tall the whole list is, steps, "외 N개" and the background work under its heading: it scrolls when taller than
    /// [`list_height`](Self::list_height).
    pub fn list_extent(&self) -> f64 {
        let mut extent = self.rows.len() as f64 * ROW_HEIGHT + if self.more.is_some() { SECTION_HEIGHT } else { 0.0 };
        if !self.background.is_empty() {
            extent += SECTION_HEIGHT
                + self.background.len() as f64 * ROW_HEIGHT
                + if self.background_more.is_some() { SECTION_HEIGHT } else { 0.0 };
        }
        extent
    }

    /// How tall the list's box stands: the whole list up to [`VISIBLE_LINES`] lines and a section heading.
    pub fn list_height(&self) -> f64 {
        self.list_extent().min(MAXIMUM_LIST)
    }

    /// Where the list stands scrolled before anyone scrolls it: the line before the focus at the top, so the step in progress shows with
    /// the one done before it.
    pub fn opening_scroll(&self) -> f64 {
        (self.focus.saturating_sub(1) as f64 * ROW_HEIGHT).clamp(0.0, (self.list_extent() - self.list_height()).max(0.0))
    }

    /// How tall the card stands over the bubble, the gap to it included: the window grows by this much.
    pub fn height(&self, collapsed: bool) -> f64 {
        let height = HEADER_HEIGHT + GAP;
        if collapsed {
            return height;
        }
        height + self.list_height() + PADDING
    }

    /// The card without its clocks: a clock that ticks redraws the card, nothing more.
    pub fn without_clocks(&self) -> CompanionProgress {
        let still = |lines: &[ProgressLine]| lines.iter().map(|l| ProgressLine { time: None, ..l.clone() }).collect();
        CompanionProgress { rows: still(&self.rows), background: still(&self.background), ..self.clone() }
    }
}

#[cfg(test)]
mod tests {
    use mighty_core::graph::{FlowStep, GraphAgentFigures, MightyGraphAgent};

    use super::*;

    const T0: i64 = 621_355_968_000_000_000 + 1_800_000_000 * 10_000_000;

    fn at(seconds: i64) -> DateTimeOffset {
        DateTimeOffset::from_utc_ticks(T0 + seconds * 10_000_000)
    }

    fn stamp(seconds: i64) -> String {
        steps::stamp(&at(seconds))
    }

    fn ko() -> Locale {
        Locale::new("ko")
    }

    /// A request of `steps` steps, the one at `current` in progress (all done when `current` is past them), with `background` commands
    /// running in the background.
    fn run(id: &str, steps: usize, current: usize, background: usize) -> MightyGraphRun {
        let running = current < steps || background > 0;
        MightyGraphRun {
            id: Some(id.into()),
            input: Some("배포해 줘".into()),
            status: Some(if running { "running" } else { "completed" }.into()),
            root_entries: Some(vec![]),
            result_entries: Some(vec![]),
            provider: Some("claude".into()),
            source_run_id: Some(id.into()),
            steps: Some(
                (0..steps)
                    .map(|index| FlowStep {
                        key: Some(format!("text:{index}")),
                        content: Some(format!("단계 {}", index + 1)),
                        active_form: None,
                        status: Some(
                            match index.cmp(&current) {
                                std::cmp::Ordering::Less => "completed",
                                std::cmp::Ordering::Equal => "in_progress",
                                std::cmp::Ordering::Greater => "pending",
                            }
                            .into(),
                        ),
                        started_at: (index <= current).then(|| stamp(index as i64 * 10)),
                        ended_at: (index < current).then(|| stamp(index as i64 * 10 + 9)),
                    })
                    .collect(),
            ),
            agents: Some(
                (0..background)
                    .map(|index| MightyGraphAgent {
                        id: Some(format!("t{index}")),
                        title: Some(format!("작업 {index}")),
                        input: Some(String::new()),
                        status: Some("running".into()),
                        entries: Some(vec![]),
                        kind: Some("task".into()),
                        background: Some(true),
                        figures: Some(GraphAgentFigures { started_at: Some(stamp(1)), ..GraphAgentFigures::default() }),
                        ..MightyGraphAgent::default()
                    })
                    .collect(),
            ),
            ..MightyGraphRun::default()
        }
    }

    #[test]
    fn a_request_without_steps_or_background_work_has_no_card() {
        assert_eq!(CompanionProgress::newest(&[run("r", 0, 0, 0)], 0, at(60), &ko()), None);
        assert_eq!(CompanionProgress::newest(&[], 0, at(60), &ko()), None);
    }

    #[test]
    fn the_card_holds_every_step_and_opens_on_the_one_in_progress() {
        let card = CompanionProgress::newest(&[run("r", 9, 6, 0)], 0, at(75), &ko()).expect("a card");
        assert_eq!(card.rows.len(), 9, "every step, no \"외 N개\"");
        assert_eq!(card.rows[4..7].iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["5. 단계 5", "6. 단계 6", "7. 단계 7"]);
        assert_eq!(
            card.rows.iter().map(|r| r.state).collect::<Vec<_>>()[4..7],
            [ProgressState::Done, ProgressState::Done, ProgressState::Current]
        );
        assert_eq!(card.more, None);
        assert_eq!(card.focus, 6);
        // Nine lines outgrow the eight-line box; it opens with the step before the one in progress at its top, held to its end.
        assert_eq!(card.list_extent(), 9.0 * ROW_HEIGHT);
        assert_eq!(card.list_height(), MAXIMUM_LIST);
        assert_eq!(card.opening_scroll(), (9.0 * ROW_HEIGHT - MAXIMUM_LIST).min(5.0 * ROW_HEIGHT));
        assert_eq!(card.count.as_deref(), Some("6/9"));
        assert_eq!(card.heading, "요청 1");
        assert_eq!((card.title.as_str(), card.status.as_str(), card.running), ("진행 상황", "running", true));
        // A finished step took its time; the step in progress counts from its start; one to come has none.
        assert_eq!(card.rows[4].time.as_deref(), Some("9초"));
        assert_eq!(card.rows[6].time.as_deref(), Some("15초"));
        assert_eq!(card.rows[7].time, None);
        assert!(card.background.is_empty() && card.background_more.is_none());
        // The page numbers the request after the pane's older history.
        assert_eq!(CompanionProgress::newest(&[run("r", 9, 6, 0)], 26, at(75), &ko()).unwrap().heading, "요청 27");
    }

    #[test]
    fn the_newest_request_is_the_one_shown() {
        let runs = [run("old", 3, 9, 0), run("new", 2, 0, 0)];
        let card = CompanionProgress::newest(&runs, 0, at(5), &ko()).expect("a card");
        assert_eq!((card.heading.as_str(), card.count.as_deref()), ("요청 2", Some("0/2")));
        assert_eq!(card.rows[0].state, ProgressState::Current);
    }

    #[test]
    fn background_work_alone_makes_a_card_and_folding_shrinks_it() {
        let card = CompanionProgress::newest(&[run("r", 0, 0, 5)], 0, at(61), &ko()).expect("a card");
        assert!(card.rows.is_empty() && card.count.is_none() && card.more.is_none());
        assert_eq!(card.background.len(), 5, "every piece of background work, no \"외 N개\"");
        assert_eq!(card.background_more, None);
        assert_eq!((card.background_title.as_str(), card.background_count.as_str()), ("백그라운드", "5/5"));
        assert!(card.background.iter().all(|b| b.agent == Some(false) && b.time.as_deref() == Some("1분 0초")));
        let open = card.height(false);
        let folded = card.height(true);
        assert_eq!(folded, HEADER_HEIGHT + GAP);
        assert_eq!(open, folded + SECTION_HEIGHT + 5.0 * ROW_HEIGHT + PADDING);
        assert_eq!(card.opening_scroll(), 0.0, "it fits: nothing to scroll");
    }

    #[test]
    fn a_finished_request_keeps_its_card_without_ticking() {
        let card = CompanionProgress::newest(&[run("r", 3, 9, 0)], 0, at(500), &ko()).expect("a card");
        assert_eq!((card.status.as_str(), card.running, card.count.as_deref()), ("completed", false, Some("3/3")));
        assert!(card.rows.iter().all(|r| r.state == ProgressState::Done && r.time.as_deref() == Some("9초")));
        assert_eq!(card.without_clocks().rows.iter().filter(|r| r.time.is_some()).count(), 0);
        assert_eq!(card.height(false), HEADER_HEIGHT + GAP + 3.0 * ROW_HEIGHT + PADDING);
    }
}
