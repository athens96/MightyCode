//! The pet's picture as a display list (C# WinUI\CompanionOverlay.cs's drawing: `RenderCard`, `TaskContent`, `RequestContent`, the plan,
//! `Presence`, `Pager`, `Button`, `QuestionBubble`, `Pet`, `Menu`, `Text`, `KeepEnd`, `Put`, `Icon`; plan §0.6, D8), laid out with the
//! window's own text measure ([`PetMeasure`]) as C# lays out with the DirectWrite layouts it draws, and drawn by the platform's
//! `PetWindow`. The display list's types are mighty-platform's (`pet`), re-exported here, because the platform crates draw them and
//! depend on nothing else. Each part is laid out and measured with the same code that draws it (C#'s null session measures): a
//! [`Painter`] that does not draw records nothing. Owner: W8-E.

pub use mighty_platform::pet::{
    BODY, ICONS, MONO, PetBitmap, PetColor, PetGlyphSize, PetItem, PetMeasure, PetPoint, PetRect, PetScene, PetSegment, PetText,
    PetTextAlign, PetTextRun, SYMBOLS, WIDE,
};

use mighty_core::design_tokens::{DesignColor, DesignPalette, Token, metrics};

use crate::bubble::{self, BubbleButton};
use crate::card::{CompanionCard, CompanionRequest};
use crate::layout;
use crate::plan::{self, PlanBlock};
use crate::progress::{CompanionProgress, ProgressLine};
use mighty_graph::flow::steps::ProgressState;

/// The bubble: padding 12 inside a radius of 18.
pub const INSET: f64 = 12.0;
pub const CORNER: f64 = 18.0;
pub const DISABLED_OPACITY: f64 = 0.45;
/// A small system button and the gap between two.
pub const BUTTON_HEIGHT: f64 = 20.0;
pub const BUTTON_RADIUS: f64 = metrics::RADIUS_FILE_ROW;
pub const BUTTON_GAP: f64 = 6.0;
/// The plan's text in its box: padding h8 v6, its blocks 6 apart; a list's marker in a column of 17 with 6 after it, each deeper level
/// 14 further in. One notch of the wheel moves three lines.
pub const PLAN_INSET_H: f64 = metrics::SPACING_MD;
pub const PLAN_INSET_V: f64 = metrics::SPACING_SM;
pub const PLAN_GAP: f64 = metrics::SPACING_SM;
pub const PLAN_MARKER: f64 = 17.0;
pub const PLAN_INDENT: f64 = 14.0;
pub const PLAN_MINIMUM: f64 = 40.0;
pub const PLAN_NOTCH: f64 = 39.0;
/// The pager's row and its chevron capsules.
pub const PAGER_HEIGHT: f64 = 16.0;
pub const PAGER_BUTTON: f64 = 22.0;
/// The pet's menu: rows of 26 with 13pt words, 4 inside a radius of 8, the row under the pointer on a wash of radius 4.
pub const MENU_ROW: f64 = 26.0;
pub const MENU_INSET: f64 = 4.0;
pub const MENU_ROW_RADIUS: f64 = 4.0;
pub const MENU_TYPE: f64 = 13.0;
/// The progress card's corner, its state pill's height, and the press on its heading that folds it.
pub const PROGRESS_CORNER: f64 = 14.0;
pub const PILL_HEIGHT: f64 = 18.0;
pub const PROGRESS_FOLD: &str = "progress-fold";
/// C# `DesignMetrics.Radius.Row`, `.Search`.
const RADIUS_ROW: f64 = 8.0;
const RADIUS_SEARCH: f64 = 7.0;

/// The palette of the bubble: the app's theme (`DesignTokens.Palette(!card.Light)`).
pub fn palette(light: bool) -> &'static DesignPalette {
    if light { &mighty_core::design_tokens::LIGHT } else { &mighty_core::design_tokens::DARK }
}

/// `DesignBrushes.ToColor(color, opacity)`.
pub fn paint(color: DesignColor, opacity: f64) -> PetColor {
    PetColor { r: color.r, g: color.g, b: color.b, a: (opacity.clamp(0.0, 1.0) * 255.0).round() as u8 }
}

/// `StatusGlyph.Tone` then `DesignPalette.TextToken`: a state's ink.
pub fn status_ink(status: &str) -> Token {
    match status {
        "running" => Token::Accent,
        "waiting" => Token::WaitText,
        "completed" => Token::DoneText,
        "error" | "failed" => Token::ErrText,
        "stopped" | "cancelled" | "interrupted" => Token::StopText,
        _ => Token::Ink2,
    }
}

/// `DesignPalette.Soft(DesignTone(blockStatus:))`: the wash of a state's pill.
pub fn status_soft(status: &str) -> Token {
    match status {
        "running" | "starting" | "queued" => Token::RunSoft,
        "waiting" => Token::WaitSoft,
        "completed" => Token::DoneSoft,
        "error" | "failed" => Token::ErrSoft,
        _ => Token::StopSoft,
    }
}

/// A text laid out and measured, not yet placed (C#'s `CanvasTextLayout`): its `LayoutBounds` width and height.
#[derive(Debug, Clone)]
pub struct Laid {
    pub text: PetText,
    pub width: f64,
    pub height: f64,
}

/// The plan's blocks and where each stands, measured once for a plan and a width (C# `PlanMeasure`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanMeasure {
    pub plan: String,
    pub width: f64,
    pub blocks: Vec<PlanBlock>,
    pub tops: Vec<f64>,
    pub heights: Vec<f64>,
    pub total: f64,
}

/// The plan box of the bubble last drawn: its box, how tall its whole text is, and how far it is scrolled.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlanBox {
    pub area: PetRect,
    pub extent: f64,
    pub scroll: f64,
}

/// Lays out (and, when `draw`, draws) the bubble's parts.
pub struct Painter<'a> {
    pub measure: &'a dyn PetMeasure,
    pub palette: &'static DesignPalette,
    pub draw: bool,
    pub items: Vec<PetItem>,
    /// What a press at each place means (C# `hitButtons`), in drawing order.
    pub hits: Vec<(PetRect, String)>,
    /// Where the presence mark or the busy answer turns.
    pub spinner: Option<PetPoint>,
    pub plan_measure: &'a mut Option<PlanMeasure>,
    pub plan: PlanBox,
}

fn run(text: &str) -> PetTextRun {
    PetTextRun { text: text.into(), color: PetColor::default(), bold: false, italic: false, mono: false }
}

impl<'a> Painter<'a> {
    pub fn new(
        measure: &'a dyn PetMeasure,
        palette: &'static DesignPalette,
        draw: bool,
        plan_measure: &'a mut Option<PlanMeasure>,
    ) -> Self {
        Painter { measure, palette, draw, items: vec![], hits: vec![], spinner: None, plan_measure, plan: PlanBox::default() }
    }

    fn color(&self, token: Token) -> PetColor {
        paint(self.palette[token], 1.0)
    }

    fn push(&mut self, item: PetItem) {
        if self.draw {
            self.items.push(item);
        }
    }

    /// C# `Text(value, size, weight, wide, lines, family, tabular)`.
    #[allow(clippy::too_many_arguments, reason = "C#'s Text has these parameters")]
    pub fn text(&self, value: &str, size: f64, weight: u16, wide: f64, lines: u32, family: Option<&str>, tabular: bool) -> Laid {
        self.text_runs(value, vec![run(value)], size, weight, wide, lines, family, tabular)
    }

    #[allow(clippy::too_many_arguments, reason = "C#'s Text has these parameters")]
    fn text_runs(
        &self,
        value: &str,
        runs: Vec<PetTextRun>,
        size: f64,
        weight: u16,
        wide: f64,
        lines: u32,
        family: Option<&str>,
        tabular: bool,
    ) -> Laid {
        let mut text = PetText {
            rect: PetRect::new(0.0, 0.0, wide, 0.0),
            bounds: PetRect::default(),
            value: value.into(),
            runs,
            family: family.unwrap_or(BODY).into(),
            size,
            // Medium has no face of its own here: asked for 500, DirectWrite hands out the semibold instance of Segoe UI Variable and
            // the bold of the Hangul fallback. Regular is the nearer of the two weights there are.
            weight: if weight == 500 { 400 } else { weight },
            asked: weight,
            max_lines: lines,
            lines: 0,
            trimmed: false,
            align: PetTextAlign::Leading,
            tabular,
            ink: String::new(),
        };
        let measured = self.measure.measure(&text);
        if let Some(shown) = measured.shown {
            text.runs = vec![run(&shown)];
        }
        text.lines = measured.lines;
        text.trimmed = measured.trimmed;
        text.rect.height = measured.height;
        Laid { text, width: measured.width, height: measured.height }
    }

    /// C# `Put`: the text drawn at `x, y` in the ink of `token` (runs already coloured keep theirs).
    pub fn put(&mut self, laid: &Laid, x: f64, y: f64, token: Token) {
        if !self.draw {
            return;
        }
        let mut text = laid.text.clone();
        let ink = self.color(token);
        for run in &mut text.runs {
            if run.color == PetColor::default() {
                run.color = ink;
            }
        }
        text.rect.x = x;
        text.rect.y = y;
        text.bounds = PetRect::new(x, y, laid.width, laid.height);
        text.ink = token.camel();
        self.items.push(PetItem::Text(text));
    }

    /// C# `KeepEnd`: one line that keeps its end, as `.truncationMode(.head)` does.
    pub fn keep_end(&self, value: &str, size: f64, weight: u16, wide: f64) -> String {
        let fits = |candidate: &str| self.text(candidate, size, weight, WIDE, 1, None, false).width <= wide;
        if fits(value) {
            return value.to_string();
        }
        let chars: Vec<char> = value.chars().collect();
        let (mut low, mut high) = (1usize, chars.len());
        while low < high {
            let middle = (low + high) / 2;
            let candidate: String = std::iter::once('\u{2026}').chain(chars[middle..].iter().copied()).collect();
            if fits(&candidate) {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        std::iter::once('\u{2026}').chain(chars[low.min(chars.len())..].iter().copied()).collect()
    }

    /// C# `Icon`: a glyph of an icon font, its ink `tall` high, centred on a point; `over` a second glyph of the cell in its own ink.
    #[allow(clippy::too_many_arguments, reason = "C#'s Icon has these parameters")]
    pub fn icon(&mut self, family: &str, glyph: &str, tall: f64, x: f64, y: f64, color: PetColor, over: Option<(&str, PetColor)>) {
        self.push(PetItem::Glyph {
            family: family.into(),
            glyph: glyph.into(),
            center: PetPoint { x, y },
            size: PetGlyphSize::Ink(tall),
            weight: 400,
            color,
            over: over.map(|(g, c)| (g.to_string(), c)),
        });
    }

    fn fill(&mut self, rect: PetRect, radius: f64, color: PetColor) {
        self.push(PetItem::Fill { rect, radius, color });
    }

    /// `DrawRoundedRectangle` of a hairline, half a pixel in (C#'s `+ .5f`, `- 1`).
    fn hairline(&mut self, rect: PetRect, radius: f64, color: PetColor, width: f64) {
        self.push(PetItem::Stroke {
            rect: PetRect::new(rect.x + 0.5, rect.y + 0.5, rect.width - 1.0, rect.height - 1.0),
            radius: radius - 0.5,
            color,
            width,
        });
    }

    /// The task (C# `TaskContent`) inside the bubble's padding: laid out, drawn when drawing, its height returned. `room` is what a bubble
    /// of a fixed height leaves for it.
    pub fn task_content(&mut self, current: &CompanionCard, x: f64, y: f64, w: f64, room: Option<f64>) -> f64 {
        let pager = current.count > 1 || current.pending;
        let asks = current.request.as_deref().is_some_and(|r| !r.is_empty());
        // The header: the presence mark in its 15pt frame, the workspace over the title, and at the right the state word over the clock,
        // each column centred on the row.
        let elapsed = current.elapsed.as_deref();
        let state = self.text(&current.state, 9.0, 400, WIDE, 1, None, false);
        let clock = elapsed.map(|e| self.text(e, 10.0, 400, WIDE, 1, Some(MONO), false));
        const CLOCK_ICON: f64 = 10.0;
        const CLOCK_GAP: f64 = 4.0;
        let trailing = state.width.max(clock.as_ref().map_or(0.0, |c| CLOCK_ICON + CLOCK_GAP + c.width)).ceil();
        let trailing_height = state.height + clock.as_ref().map_or(0.0, |c| 2.0 + c.height);
        // 15 for the mark, then 7 before the names, 7 before the spacer and 7 after it.
        let leading = (w - 15.0 - 7.0 - 7.0 - 7.0 - trailing).max(1.0);
        let workspace = (!current.workspace.is_empty()).then(|| self.text(&current.workspace, 9.0, 500, leading, 1, None, false));
        let title = self.text(&current.title, 11.0, 600, leading, 1, None, false);
        let leading_height = workspace.as_ref().map_or(0.0, |s| s.height + 1.0) + title.height;
        let head = 15f64.max(leading_height.max(trailing_height)).ceil();
        if self.draw {
            self.presence(&current.status, x + 7.5, y + head / 2.0);
            let mut line = (y + (head - leading_height) / 2.0).round();
            if let Some(workspace) = &workspace {
                self.put(workspace, x + 22.0, line, Token::Ink2);
                line += workspace.height + 1.0;
            }
            self.put(&title, x + 22.0, line, Token::Ink);
            let mut line = (y + (head - trailing_height) / 2.0).round();
            self.put(&state, x + w - state.width, line, Token::Ink2);
            if let Some(clock) = &clock {
                line += state.height + 2.0;
                let ink = self.color(Token::Ink2);
                self.icon(
                    ICONS,
                    "\u{E823}",
                    CLOCK_ICON,
                    x + w - clock.width - CLOCK_GAP - CLOCK_ICON / 2.0,
                    line + clock.height / 2.0,
                    ink,
                    None,
                );
                self.put(clock, x + w - clock.width, line, Token::Ink2);
            }
        }
        // "요청" and "작업", each a 9pt label two down from the top of its words.
        let asked = mighty_run::strings::get("graph.timeline.kind.request");
        let doing = mighty_run::strings::get("transcript.tool.fallback");
        let asked_tag = asks.then(|| self.text(&asked, 9.0, 500, WIDE, 1, None, false));
        let doing_tag = self.text(&doing, 9.0, 500, WIDE, 1, None, false);
        let asked_width = asked_tag.as_ref().map_or(0.0, |t| t.width.ceil());
        let doing_width = doing_tag.width.ceil();
        let request_width = (w - asked_width - 6.0).max(1.0);
        let work_width = (w - doing_width - 6.0).max(1.0);
        // Two lines each while the height follows the content; a taller bubble shows more of the request and the work instead of
        // padding, and the work keeps its lines first.
        let (mut request_lines, mut work_lines) = (2u32, 2u32);
        if let Some(available) = room {
            let probe = self.text("Ag", 11.0, 400, WIDE, 1, None, false);
            let apart =
                head + if asks { 6.0 + 6.0 + metrics::STROKE_LINE } else { 0.0 } + 6.0 + if pager { 5.0 + PAGER_HEIGHT } else { 0.0 };
            let lines = (((available - apart) / probe.height) as i64).max(if asks { 2 } else { 1 });
            let all_work = self.text(&current.work, 11.0, 400, work_width, 0, None, false);
            work_lines = i64::from(all_work.text.lines).clamp(1, lines - i64::from(asks)) as u32;
            if asks {
                let all_request = self.text(current.request.as_deref().unwrap_or(""), 11.0, 400, request_width, 0, None, false);
                request_lines = i64::from(all_request.text.lines).clamp(1, lines - i64::from(work_lines)) as u32;
            }
        }
        let mut cursor = y + head;
        if asks {
            cursor += 6.0;
            let request = current.request.as_deref().unwrap_or("");
            let words = self.text(request, 11.0, 400, request_width, request_lines, None, false);
            let tag = asked_tag.as_ref().expect("asked");
            if self.draw {
                self.put(tag, x, cursor + 2.0, Token::Ink2);
                self.put(&words, x + asked_width + 6.0, cursor, Token::Ink);
            }
            cursor += (tag.height + 2.0).max(words.height).ceil() + 6.0;
            let line = self.color(Token::Line);
            self.fill(PetRect::new(x, cursor, w, metrics::STROKE_LINE), 0.0, line);
            cursor += metrics::STROKE_LINE;
        }
        cursor += 6.0;
        let words = self.text(&current.work, 11.0, 400, work_width, work_lines, None, false);
        if self.draw {
            self.put(&doing_tag, x, cursor + 2.0, Token::Ink2);
            self.put(&words, x + doing_width + 6.0, cursor, Token::Ink);
        }
        cursor += (doing_tag.height + 2.0).max(words.height).ceil();
        if pager {
            cursor += 5.0;
            if self.draw {
                self.pager(current, x, cursor, w);
            }
            cursor += PAGER_HEIGHT;
        }
        cursor - y
    }

    /// What stands under a request's body (C#'s `Tail`): what went wrong, the buttons and the pager.
    fn tail(&mut self, current: &CompanionCard, request: &CompanionRequest, x: f64, top: f64, w: f64) -> f64 {
        const SPACING: f64 = 7.0;
        let mut at = top;
        if let Some(error) = request.error.as_deref().filter(|e| !e.is_empty()) {
            at += SPACING;
            let words = self.text(error, 9.0, 400, w, 2, None, false);
            self.put(&words, x, at, Token::ErrText);
            at += words.height.ceil();
        }
        let row: Vec<&BubbleButton> = request.buttons.iter().filter(|b| !b.own_row).collect();
        if !row.is_empty() {
            // Open (and what else opens elsewhere) from the left, the answers from the right edge, 6 apart.
            at += SPACING;
            let mut edge = x + w;
            let free = w - row.iter().map(|b| self.button_width(b, WIDE)).sum::<f64>() - (row.len() as f64 - 1.0) * BUTTON_GAP;
            // Words too long for the row end one button in an ellipsis: the prominent one, else the longest.
            let squeezed = if free < 0.0 {
                row.iter()
                    .filter(|b| !b.icon)
                    .map(|b| (b.prominent, self.button_width(b, WIDE), b.id.clone()))
                    .fold(None::<(bool, f64, String)>, |best, next| match best {
                        Some(best) if (best.0, best.1) >= (next.0, next.1) => Some(best),
                        _ => Some(next),
                    })
                    .map(|best| best.2)
            } else {
                None
            };
            for button in row.iter().filter(|b| !b.leading).rev() {
                let limit = if squeezed.as_deref() == Some(button.id.as_str()) { self.button_width(button, WIDE) + free } else { WIDE };
                let wide = self.button_width(button, limit);
                edge -= wide;
                if self.draw {
                    self.button(button, edge, at, wide);
                }
                edge -= BUTTON_GAP;
            }
            // The small spinner before the answers while one is on its way.
            if request.busy && self.draw {
                self.spinner = Some(PetPoint { x: edge - 5.0, y: at + BUTTON_HEIGHT / 2.0 });
            }
            let mut next = x;
            for button in row.iter().filter(|b| b.leading) {
                let wide = self.button_width(button, WIDE);
                if self.draw {
                    self.button(button, next, at, wide);
                }
                next += wide + BUTTON_GAP;
            }
            at += BUTTON_HEIGHT;
        }
        let mut under = !row.is_empty();
        for button in request.buttons.iter().filter(|b| b.own_row) {
            // A long answer on a row of its own, as wide as the bubble.
            at += if under { BUTTON_GAP } else { SPACING };
            under = true;
            if self.draw {
                self.button(button, x, at, w);
            }
            at += BUTTON_HEIGHT;
        }
        if current.count > 1 || current.pending {
            at += SPACING;
            if self.draw {
                self.pager(current, x, at, w);
            }
            at += PAGER_HEIGHT;
        }
        at - top
    }

    /// An approval, one question or a finished plan (C# `RequestContent`), laid out and drawn like the task: 7 between its parts.
    pub fn request_content(&mut self, current: &CompanionCard, request: &CompanionRequest, x: f64, y: f64, w: f64) -> f64 {
        const SPACING: f64 = 7.0;
        let plan = request.plan.is_some();
        // The header: the symbol in the amber ink, the title with when it came, and where it comes from, which keeps its end.
        let icon_width = if request.question {
            14.0
        } else if plan {
            12.0
        } else {
            10.5
        };
        let icon_height: f64 = 13.5;
        let title = self.text(&request.title, 11.0, 600, WIDE, 1, None, true);
        let received = request.received.as_deref().filter(|r| !r.is_empty()).map(|came| self.text(came, 9.0, 400, WIDE, 1, None, true));
        let titled = icon_width + 6.0 + title.width + received.as_ref().map_or(0.0, |r| 6.0 + r.width);
        let kept = self.keep_end(&request.origin, 9.0, 400, (w - titled - 12.0).max(1.0));
        let origin = self.text(&kept, 9.0, 400, WIDE, 1, None, false);
        let head = icon_height.max(title.height.max(origin.height)).ceil();
        if self.draw {
            let wait = self.color(Token::WaitText);
            if request.question {
                self.question_bubble(x + icon_width / 2.0, y + head / 2.0);
            } else if plan {
                self.icon(ICONS, "\u{E9D5}", 12.0, x + icon_width / 2.0, y + head / 2.0, wait, None);
            } else {
                self.icon(SYMBOLS, "\u{270B}", icon_height, x + icon_width / 2.0, y + head / 2.0, wait, None);
            }
            self.put(&title, x + icon_width + 6.0, (y + (head - title.height) / 2.0).round(), Token::Ink);
            if let Some(came) = &received {
                self.put(came, x + icon_width + 6.0 + title.width + 6.0, (y + (head - came.height) / 2.0).round(), Token::Ink2);
            }
            self.put(&origin, x + w - origin.width, (y + (head - origin.height) / 2.0).round(), Token::Ink2);
        }
        let mut cursor = y + head;
        if plan {
            // The plan in small Markdown on the ink at 6%: whole while the taller window has room for it under the header and over the
            // buttons, scrolling in that room once it has not.
            let measure = self.measure_plan(request.plan.as_deref().unwrap_or(""), w - 2.0 * PLAN_INSET_H);
            let drawing = std::mem::replace(&mut self.draw, false);
            let tail = self.tail(current, request, x, 0.0, w);
            self.draw = drawing;
            let room = PLAN_MINIMUM.max(layout::TALL_HEIGHT - layout::CHROME - 2.0 * INSET - head - SPACING - tail);
            let area = PetRect::new(x, cursor + SPACING, w, (measure.total + 2.0 * PLAN_INSET_V).min(room).ceil());
            self.draw_plan(&measure, area);
            cursor = area.bottom();
        } else if request.question {
            self.stack(&mut cursor, request.headline.as_deref(), 11.0, 500, 3, Token::Ink, None, x, w, SPACING);
            // A choice: its label over its description on the subtle wash, or on the accent at 16% once picked. A press answers.
            for (index, option) in request.options.iter().enumerate() {
                cursor += SPACING;
                let lead = if request.multi_select { 10.0 + 6.0 } else { 0.0 };
                let label = self.text(&option.label, 11.0, 500, (w - 16.0 - lead).max(1.0), 1, None, false);
                let description = (!option.description.is_empty())
                    .then(|| self.text(&option.description, 9.0, 400, (w - 16.0 - lead).max(1.0), 1, None, false));
                let row_height = (5.0 + label.height + description.as_ref().map_or(0.0, |d| 1.0 + d.height) + 5.0).ceil();
                if self.draw {
                    let wash = if option.picked {
                        paint(self.palette[Token::Accent], 0.16)
                    } else {
                        paint(self.palette.subtle(), metrics::SUBTLE_OPACITY)
                    };
                    self.fill(PetRect::new(x, cursor, w, row_height), RADIUS_SEARCH, wash);
                    if request.multi_select {
                        let ink = self.color(if option.picked { Token::Accent } else { Token::Ink2 });
                        self.icon(
                            ICONS,
                            if option.picked { "\u{E73D}" } else { "\u{E739}" },
                            10.0,
                            x + 8.0 + 5.0,
                            cursor + 5.0 + 1.0 + 5.0,
                            ink,
                            None,
                        );
                    }
                    self.put(&label, x + 8.0 + lead, cursor + 5.0, Token::Ink);
                    if let Some(description) = &description {
                        self.put(description, x + 8.0 + lead, cursor + 5.0 + label.height + 1.0, Token::Ink2);
                    }
                    self.hits.push((PetRect::new(x, cursor, w, row_height), format!("{}{index}", bubble::CHOICE)));
                }
                cursor += row_height;
            }
        } else {
            self.stack(&mut cursor, Some(&request.detail), 10.0, 400, 1, Token::Ink2, None, x, w, SPACING);
            self.stack(&mut cursor, request.headline.as_deref(), 11.0, 500, 2, Token::Ink, None, x, w, SPACING);
            match request.code.as_deref().filter(|c| !c.is_empty()) {
                Some(code) if request.boxed => {
                    // The command, three lines in a box of the primary ink at 6%.
                    cursor += SPACING;
                    let words = self.text(code, 10.0, 400, (w - 14.0).max(1.0), 3, Some(MONO), false);
                    let box_height = words.height.ceil() + 10.0;
                    if self.draw {
                        let wash = paint(self.palette[Token::Ink], 0.06);
                        self.fill(PetRect::new(x, cursor, w, box_height), metrics::RADIUS_SEGMENT, wash);
                        self.put(&words, x + 7.0, cursor + 5.0, Token::Ink);
                    }
                    cursor += box_height;
                }
                code => self.stack(&mut cursor, code, 10.0, 400, 3, Token::Ink, Some(MONO), x, w, SPACING),
            }
        }
        cursor += self.tail(current, request, x, cursor, w);
        cursor - y
    }

    /// C#'s `Stack`: a part under the last, 7 apart, when it has words.
    #[allow(clippy::too_many_arguments, reason = "C#'s local Stack closes over these")]
    fn stack(
        &mut self,
        cursor: &mut f64,
        value: Option<&str>,
        size: f64,
        weight: u16,
        lines: u32,
        ink: Token,
        family: Option<&str>,
        x: f64,
        w: f64,
        spacing: f64,
    ) {
        let Some(value) = value.filter(|v| !v.is_empty()) else { return };
        *cursor += spacing;
        let words = self.text(value, size, weight, w, lines, family, false);
        self.put(&words, x, *cursor, ink);
        *cursor += words.height.ceil();
    }

    /// C# `MeasurePlan`: the plan's blocks and where each stands at this width, kept while the plan and the width stay.
    fn measure_plan(&mut self, plan: &str, width: f64) -> PlanMeasure {
        if let Some(known) = self.plan_measure.as_ref().filter(|k| k.width == width && k.plan == plan) {
            return known.clone();
        }
        let blocks = plan::plan_blocks(plan);
        let (mut tops, mut heights, mut total) = (Vec::with_capacity(blocks.len()), Vec::with_capacity(blocks.len()), 0.0);
        let drawing = std::mem::replace(&mut self.draw, false);
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                total += PLAN_GAP;
            }
            tops.push(total);
            let height = self.plan_block(block, 0.0, 0.0, width);
            heights.push(height);
            total += height;
        }
        self.draw = drawing;
        let measure = PlanMeasure { plan: plan.into(), width, blocks, tops, heights, total };
        *self.plan_measure = Some(measure.clone());
        measure
    }

    /// C# `DrawPlan`: the plan's text in its box, clipped and moved up by what was scrolled, only the blocks in sight drawn, a slim thumb
    /// at the right edge once it outgrows the box. A press on it is no button's.
    fn draw_plan(&mut self, measure: &PlanMeasure, area: PetRect) {
        let extent = measure.total + 2.0 * PLAN_INSET_V;
        let scroll = self.plan.scroll.clamp(0.0, (extent - area.height).max(0.0));
        self.plan = PlanBox { area, extent, scroll };
        if !self.draw {
            return;
        }
        let wash = paint(self.palette[Token::Ink], 0.06);
        self.fill(area, RADIUS_ROW, wash);
        let outer = std::mem::take(&mut self.items);
        let (inner, first, wide) = (area.x + PLAN_INSET_H, area.y + PLAN_INSET_V - scroll, area.width - 2.0 * PLAN_INSET_H);
        for (index, block) in measure.blocks.iter().enumerate() {
            let at = first + measure.tops[index];
            if at + measure.heights[index] >= area.y && at <= area.bottom() {
                self.plan_block(block, inner, at, wide);
            }
        }
        let clipped = std::mem::replace(&mut self.items, outer);
        self.items.push(PetItem::Group {
            role: "plan".into(),
            rect: area,
            opacity: 1.0,
            items: vec![PetItem::Clip { rect: area, items: clipped }],
        });
        if extent > area.height {
            let thumb = 16f64.max(area.height * area.height / extent);
            let travel = area.height - 4.0 - thumb;
            let color = paint(self.palette[Token::Ink], 0.25);
            self.fill(PetRect::new(area.right() - 5.0, area.y + 2.0 + travel * scroll / (extent - area.height), 3.0, thumb), 1.5, color);
        }
        self.hits.push((area, "plan-text".into()));
    }

    /// C# `PlanBlock`: one block of the plan in the bubble's small Markdown, its height returned.
    fn plan_block(&mut self, block: &PlanBlock, x: f64, y: f64, w: f64) -> f64 {
        if block.kind == "rule" {
            let line = self.color(Token::Line);
            self.fill(PetRect::new(x, y + metrics::SPACING_XS, w, metrics::STROKE_LINE), 0.0, line);
            return metrics::SPACING_XS + metrics::STROKE_LINE + metrics::SPACING_XS;
        }
        if block.kind == "code" {
            let code = self.text(&block.text, 10.0, 400, (w - 12.0).max(1.0), 0, Some(MONO), false);
            let high = code.height.ceil() + 8.0;
            if self.draw {
                let wash = paint(self.palette[Token::Ink], 0.06);
                self.fill(PetRect::new(x, y, w, high), metrics::RADIUS_SEGMENT, wash);
                self.put(&code, x + 6.0, y + 4.0, Token::Ink);
            }
            return high;
        }
        let (heading, item, quote) = (block.kind == "heading", block.kind == "item", block.kind == "quote");
        let size = if heading {
            match block.level {
                1 => 13.0,
                2 => 12.0,
                _ => 11.0,
            }
        } else {
            11.0
        };
        let over = if heading { metrics::SPACING_XXS } else { 0.0 };
        let lead = if item {
            f64::from(block.level) * PLAN_INDENT + PLAN_MARKER + 6.0
        } else if quote {
            3.0 + 8.0
        } else {
            0.0
        };
        let ink = if quote { Token::Ink2 } else { Token::Ink };
        let words =
            self.text_runs(&block.text, styled_runs(block), size, if heading { 600 } else { 400 }, (w - lead).max(1.0), 0, None, false);
        let tall = words.height.ceil();
        if self.draw {
            if quote {
                let bar = paint(self.palette[Token::Accent], 0.6);
                self.fill(PetRect::new(x, y + over, 3.0, tall), 2.0, bar);
            }
            if let (true, Some(marker)) = (item, block.marker.as_deref()) {
                // The marker ends its column of 17, level with the item's first line.
                let column = x + f64::from(block.level) * PLAN_INDENT + PLAN_MARKER;
                let line = if words.text.lines > 0 { words.height / f64::from(words.text.lines) } else { tall };
                if marker == "\u{2610}" || marker == "\u{2611}" {
                    let checked = marker == "\u{2611}";
                    let color = self.color(if checked { Token::Accent } else { Token::Ink2 });
                    self.icon(ICONS, if checked { "\u{E73D}" } else { "\u{E739}" }, 10.0, column - 5.0, y + over + line / 2.0, color, None);
                } else {
                    let mark = self.text(marker, 11.0, 400, WIDE, 1, None, true);
                    let token = if marker == "\u{2022}" { Token::Accent } else { Token::Ink2 };
                    self.put(&mark, column - mark.width, y + over, token);
                }
            }
            self.put(&words, x + lead, y + over, ink);
        }
        over + tall
    }

    /// C# `Presence`: the presence mark in its 15pt frame: a turning arc while it runs, the raised hand while it waits, the filled
    /// check or exclamation once it ended, else a ring; each in its state's ink.
    fn presence(&mut self, status: &str, x: f64, y: f64) {
        let ink = self.color(status_ink(status));
        match status {
            "running" | "starting" | "queued" => self.spinner = Some(PetPoint { x, y }),
            "waiting" => self.icon(SYMBOLS, "\u{270B}", 13.5, x, y, ink, None),
            "completed" => self.icon(ICONS, "\u{EC61}", 13.0, x, y, ink, None),
            "error" => {
                let card = self.color(Token::Card);
                self.icon(ICONS, "\u{F136}", 13.0, x, y, ink, Some(("\u{F13C}", card)));
            }
            _ => self.push(PetItem::Ellipse { center: PetPoint { x, y }, radius_x: 5.9, radius_y: 5.9, color: ink, stroke: Some(1.2) }),
        }
    }

    /// The progress card over the bubble (the Mac's `CompanionProgressCard`) in `area`: on the bubble's card at radius 14 with its
    /// hairline, a 26-high heading — the clipboard in the accent, "진행 상황", the request's number and the steps done, the state in its
    /// pill (the spinner before it while the request runs) and the fold's chevron — that folds the card; open, the step lines, "외 N개",
    /// then the background work under its own small heading, each line a press that opens the pane. The spinner's centre is
    /// [`Painter::spinner`].
    pub fn progress_card(&mut self, progress: &CompanionProgress, collapsed: bool, area: PetRect) {
        use crate::progress::{HEADER_HEIGHT, ROW_HEIGHT, SECTION_HEIGHT};
        let outer = std::mem::take(&mut self.items);
        let card = self.color(Token::Card);
        self.fill(area, PROGRESS_CORNER, card);
        let edge = paint(self.palette[Token::Ink], 0.10);
        self.hairline(area, PROGRESS_CORNER, edge, metrics::STROKE_LINE);
        let (x, w) = (area.x + metrics::SPACING_MD, area.width - 2.0 * metrics::SPACING_MD);
        let middle = area.y + HEADER_HEIGHT / 2.0;
        // The right of the heading first: the chevron, the state's pill and the spinner before it.
        const CHEVRON: f64 = 9.0;
        let gap = metrics::SPACING_XS;
        let chevron_x = x + w - CHEVRON / 2.0;
        let ink2 = self.color(Token::Ink2);
        let tip = if collapsed { -1.8 } else { 1.8 };
        for (from, to) in
            [((chevron_x - 3.5, middle - tip), (chevron_x, middle + tip)), ((chevron_x, middle + tip), (chevron_x + 3.5, middle - tip))]
        {
            self.push(PetItem::Line {
                from: PetPoint { x: from.0, y: from.1 },
                to: PetPoint { x: to.0, y: to.1 },
                color: ink2,
                width: 1.5,
                round: true,
            });
        }
        let word = self.text(&progress.state_word, 10.0, 700, WIDE, 1, None, false);
        let pill_width = (word.width + 2.0 * metrics::SPACING_SM).ceil();
        let pill_x = x + w - CHEVRON - gap - pill_width;
        let soft = paint(self.palette[status_soft(&progress.status)], 1.0);
        self.fill(PetRect::new(pill_x, middle - PILL_HEIGHT / 2.0, pill_width, PILL_HEIGHT), PILL_HEIGHT / 2.0, soft);
        self.put(&word, pill_x + metrics::SPACING_SM, (middle - word.height / 2.0).round(), status_ink(&progress.status));
        let mut right = pill_x;
        if progress.running {
            self.spinner = Some(PetPoint { x: pill_x - metrics::SPACING_SM - 4.5, y: middle });
            right -= metrics::SPACING_SM + 9.0;
        }
        // The left: the clipboard, the title, the request's number (cut to what is left) and the count.
        let accent = self.color(Token::Accent);
        self.icon(ICONS, "\u{E9D5}", 10.0, x + 5.0, middle, accent, None);
        let mut cursor = x + 10.0 + gap;
        let title = self.text(&progress.title, 10.0, 700, WIDE, 1, None, false);
        self.put(&title, cursor, (middle - title.height / 2.0).round(), Token::Ink);
        cursor += title.width + gap;
        let count = progress.count.as_deref().map(|c| self.text(c, 10.0, 600, WIDE, 1, None, true));
        let count_width = count.as_ref().map_or(0.0, |c| c.width + gap);
        let heading = self.text(&progress.heading, 10.0, 400, (right - gap - count_width - cursor).max(1.0), 1, None, false);
        self.put(&heading, cursor, (middle - heading.height / 2.0).round(), Token::Ink2);
        cursor += heading.width + gap;
        if let Some(count) = &count {
            self.put(count, cursor, (middle - count.height / 2.0).round(), Token::Ink2);
        }
        self.hits.push((PetRect::new(area.x, area.y, area.width, HEADER_HEIGHT), PROGRESS_FOLD.into()));
        if !collapsed {
            let mut y = area.y + HEADER_HEIGHT;
            for row in &progress.rows {
                self.progress_line(row, x, y, w);
                y += ROW_HEIGHT;
            }
            if let Some(more) = &progress.more {
                self.progress_more(more, x, y);
                y += SECTION_HEIGHT;
            }
            if !progress.background.is_empty() {
                // "백그라운드 1/3" at the foot of its 18.
                let label = self.text(&progress.background_title, 9.0, 600, WIDE, 1, None, false);
                let count = self.text(&progress.background_count, 9.0, 400, WIDE, 1, None, true);
                self.put(&label, x, y + SECTION_HEIGHT - label.height, Token::Ink2);
                self.put(&count, x + label.width + gap, y + SECTION_HEIGHT - count.height, Token::Ink2);
                y += SECTION_HEIGHT;
                for item in &progress.background {
                    self.progress_line(item, x, y, w);
                    y += ROW_HEIGHT;
                }
                if let Some(more) = &progress.background_more {
                    self.progress_more(more, x, y);
                }
            }
            let lines = PetRect::new(area.x, area.y + HEADER_HEIGHT, area.width, area.height - HEADER_HEIGHT);
            self.hits.push((lines, "open".into()));
        }
        let drawn = std::mem::replace(&mut self.items, outer);
        self.push(PetItem::Group { role: "progress".into(), rect: area, opacity: 1.0, items: drawn });
    }

    /// A line of the progress card: its state's mark in a column of 12, a background piece's symbol (a sub-agent or a command), its words
    /// on one line (the step in progress semibold, one to come in the secondary ink, a finished one faded) and its time at the right.
    fn progress_line(&mut self, line: &ProgressLine, x: f64, y: f64, w: f64) {
        let gap = metrics::SPACING_XS;
        let middle = y + crate::progress::ROW_HEIGHT / 2.0;
        self.progress_mark(line.state, x + 6.0, middle);
        let mut cursor = x + 12.0 + gap;
        if let Some(agent) = line.agent {
            let ink = self.color(Token::Ink2);
            self.icon(ICONS, if agent { "\u{E77B}" } else { "\u{E756}" }, 9.0, cursor + 4.5, middle, ink, None);
            cursor += 9.0 + gap;
        }
        let time = line.time.as_deref().map(|t| self.text(t, 9.0, 400, WIDE, 1, None, true));
        let time_width = time.as_ref().map_or(0.0, |t| t.width.ceil() + gap);
        if let Some(time) = &time {
            self.put(time, x + w - time.width, (middle - time.height / 2.0).round(), Token::Ink2);
        }
        let current = line.state == ProgressState::Current;
        let words = self.text(&line.text, 10.5, if current { 600 } else { 400 }, (x + w - time_width - cursor).max(1.0), 1, None, false);
        let ink = if line.state == ProgressState::Pending { Token::Ink2 } else { Token::Ink };
        if line.state == ProgressState::Done {
            let outer = std::mem::take(&mut self.items);
            self.put(&words, cursor, (middle - words.height / 2.0).round(), ink);
            let drawn = std::mem::replace(&mut self.items, outer);
            self.push(PetItem::Group {
                role: "done".into(),
                rect: PetRect::new(cursor, y, words.width, crate::progress::ROW_HEIGHT),
                opacity: 0.75,
                items: drawn,
            });
        } else {
            self.put(&words, cursor, (middle - words.height / 2.0).round(), ink);
        }
    }

    /// "외 N개" in its 18.
    fn progress_more(&mut self, more: &str, x: f64, y: f64) {
        let words = self.text(more, 9.0, 400, WIDE, 1, None, false);
        self.put(&words, x, (y + (crate::progress::SECTION_HEIGHT - words.height) / 2.0).round(), Token::Ink2);
    }

    /// A step's mark, 10 across: the filled check once done, a ring with its middle filled while in progress, a ring to come, a ring with
    /// a square once stopped, the filled exclamation once failed.
    fn progress_mark(&mut self, state: ProgressState, x: f64, y: f64) {
        let center = PetPoint { x, y };
        match state {
            ProgressState::Done => {
                let ink = self.color(Token::Done);
                self.icon(ICONS, "\u{EC61}", 10.0, x, y, ink, None);
            }
            ProgressState::Current => {
                let ink = self.color(Token::Run);
                self.push(PetItem::Ellipse { center, radius_x: 4.4, radius_y: 4.4, color: ink, stroke: Some(1.2) });
                self.push(PetItem::Ellipse { center, radius_x: 2.4, radius_y: 2.4, color: ink, stroke: None });
            }
            ProgressState::Pending => {
                let ink = self.color(Token::Ink2);
                self.push(PetItem::Ellipse { center, radius_x: 4.4, radius_y: 4.4, color: ink, stroke: Some(1.0) });
            }
            ProgressState::Stopped => {
                let ink = self.color(Token::Stop);
                self.push(PetItem::Ellipse { center, radius_x: 4.4, radius_y: 4.4, color: ink, stroke: Some(1.2) });
                self.fill(PetRect::new(x - 1.8, y - 1.8, 3.6, 3.6), 0.8, ink);
            }
            ProgressState::Failed => {
                let (ink, card) = (self.color(Token::Err), self.color(Token::Card));
                self.icon(ICONS, "\u{F136}", 10.0, x, y, ink, Some(("\u{F13C}", card)));
            }
        }
    }

    /// C# `Pager`: "‹ ● ○ ○ 1 / 3 ›" under a bubble while more than one agent is busy; the raised hand turns to the agent whose request
    /// is behind this page.
    fn pager(&mut self, current: &CompanionCard, x: f64, y: f64, w: f64) {
        let paged = current.count > 1;
        let mut group = Vec::new();
        let outer = std::mem::take(&mut self.items);
        if paged {
            for (id, at, direction) in [("previous", x, -1.0), ("next", x + w - PAGER_BUTTON, 1.0)] {
                let wash = paint(self.palette.subtle(), metrics::SUBTLE_OPACITY);
                self.fill(PetRect::new(at, y, PAGER_BUTTON, PAGER_HEIGHT), PAGER_HEIGHT / 2.0, wash);
                // chevron.left / chevron.right at 9pt semibold.
                let tip = at + PAGER_BUTTON / 2.0 + direction * 2.2;
                let back = at + PAGER_BUTTON / 2.0 - direction * 2.2;
                let middle = y + PAGER_HEIGHT / 2.0;
                let ink = self.color(Token::Ink);
                self.push(PetItem::Line {
                    from: PetPoint { x: back, y: middle - 4.0 },
                    to: PetPoint { x: tip, y: middle },
                    color: ink,
                    width: 1.5,
                    round: true,
                });
                self.push(PetItem::Line {
                    from: PetPoint { x: tip, y: middle },
                    to: PetPoint { x: back, y: middle + 4.0 },
                    color: ink,
                    width: 1.5,
                    round: true,
                });
                self.hits.push((PetRect::new(at - 3.0, y - 3.0, PAGER_BUTTON + 6.0, PAGER_HEIGHT + 6.0), id.into()));
            }
        }
        let words = format!("{} / {}", current.position, current.count);
        let label = paged.then(|| self.text(&words, 9.0, 500, WIDE, 1, None, true));
        let dots = if paged && current.count <= 8 { current.count as f64 * 5.0 + (current.count as f64 - 1.0) * 4.0 } else { 0.0 };
        let label_width = label.as_ref().map_or(0.0, |l| l.width);
        let total = dots
            + if dots > 0.0 && label.is_some() { 6.0 } else { 0.0 }
            + label_width
            + if current.pending { (if paged { 6.0 } else { 0.0 }) + 16.0 } else { 0.0 };
        let mut cursor = x + (w - total) / 2.0;
        if dots > 0.0 {
            for index in 0..current.count {
                let color = if index + 1 == current.position { self.color(Token::Accent) } else { paint(self.palette[Token::Ink], 0.22) };
                self.push(PetItem::Ellipse {
                    center: PetPoint { x: cursor + 2.5 + index as f64 * 9.0, y: y + PAGER_HEIGHT / 2.0 },
                    radius_x: 2.5,
                    radius_y: 2.5,
                    color,
                    stroke: None,
                });
            }
            cursor += dots + 6.0;
        }
        if let Some(label) = &label {
            self.put(label, cursor, (y + (PAGER_HEIGHT - label.height) / 2.0).round(), Token::Ink2);
            cursor += label_width + 6.0;
        }
        if current.pending {
            let wait = self.color(Token::WaitText);
            self.icon(SYMBOLS, "\u{270B}", 9.5, cursor + 8.0, y + PAGER_HEIGHT / 2.0, wait, None);
            self.hits.push((PetRect::new(cursor - 2.0, y - 3.0, 20.0, PAGER_HEIGHT + 6.0), "pending".into()));
        }
        group.append(&mut std::mem::replace(&mut self.items, outer));
        self.push(PetItem::Group { role: "pager".into(), rect: PetRect::new(x, y, w, PAGER_HEIGHT), opacity: 1.0, items: group });
    }

    /// C# `ButtonWidth`: a symbol is 26 wide; words are their width and 16, at least 26, at most `limit`.
    fn button_width(&self, button: &BubbleButton, limit: f64) -> f64 {
        if button.icon {
            return 26.0;
        }
        let label = self.text(&button.label, 11.0, 400, WIDE, 1, None, false);
        26f64.max(limit.min(label.width.ceil() + 16.0))
    }

    /// C# `Button`: a small system button, bordered on the raised card with a line, or prominent in the accent; a disabled one at the
    /// pane's disabled-button opacity with no hit area of its own.
    fn button(&mut self, button: &BubbleButton, x: f64, y: f64, wide: f64) {
        let outer = std::mem::take(&mut self.items);
        let rect = PetRect::new(x, y, wide, BUTTON_HEIGHT);
        if button.prominent {
            let accent = self.color(Token::Accent);
            self.fill(rect, BUTTON_RADIUS, accent);
        } else {
            let raised = self.color(Token::CardRaised);
            self.fill(rect, BUTTON_RADIUS, raised);
            let line = self.color(Token::Line);
            self.hairline(rect, BUTTON_RADIUS, line, metrics::STROKE_LINE);
        }
        let ink = if button.prominent { Token::OnAccent } else { Token::Ink };
        if button.icon {
            let color = self.color(ink);
            self.icon(ICONS, button_symbol(&button.id), 11.0, x + wide / 2.0, y + BUTTON_HEIGHT / 2.0, color, None);
        } else {
            let label = self.text(&button.label, 11.0, 400, (wide - 16.0).max(1.0), 1, None, false);
            self.put(&label, x + (wide - label.width) / 2.0, (y + (BUTTON_HEIGHT - label.height) / 2.0).round(), ink);
        }
        let drawn = std::mem::replace(&mut self.items, outer);
        let opacity = if button.disabled { DISABLED_OPACITY } else { 1.0 };
        self.items.push(PetItem::Group { role: format!("button:{}", button.id), rect, opacity, items: drawn });
        // A disabled button still owns its place: a press there is nobody's, never the bubble's own "open".
        self.hits.push((rect, if button.disabled { "disabled".into() } else { button.id.clone() }));
    }

    /// C# `QuestionBubble` (questionmark.bubble.fill): the bubble and its tail drawn, the mark cut in the card's colour.
    fn question_bubble(&mut self, x: f64, y: f64) {
        let ink = self.color(Token::WaitText);
        let (left, top) = (x - 7.0, y - 6.5);
        self.fill(PetRect::new(left, top, 14.0, 10.5), 3.6, ink);
        let point = |px: f64, py: f64| PetPoint { x: px, y: py };
        self.push(PetItem::Path {
            start: point(left + 2.6, top + 9.5),
            segments: vec![PetSegment::Line { to: point(left + 2.2, top + 13.2) }, PetSegment::Line { to: point(left + 6.6, top + 9.5) }],
            color: ink,
        });
        let card = self.color(Token::Card);
        self.push(PetItem::Glyph {
            family: BODY.into(),
            glyph: "?".into(),
            center: point(x, top + 5.25),
            size: PetGlyphSize::Font(9.0),
            weight: 800,
            color: card,
            over: None,
        });
    }
}

/// The symbol a button is drawn as: the request or the plan elsewhere (arrow.up.left.and.arrow.down.right), an answer written out, else
/// the pane (arrow.up.forward.app).
pub fn button_symbol(id: &str) -> &'static str {
    match id {
        bubble::REVIEW | bubble::PLAN_REVIEW => "\u{E740}",
        bubble::CUSTOM => "\u{E70F}",
        _ => "\u{E8A7}",
    }
}

/// A block's text as runs: each character in the faces of the spans over it (C#'s `SetFontWeight`, `SetFontStyle`, `SetFontFamily`).
fn styled_runs(block: &PlanBlock) -> Vec<PetTextRun> {
    let chars: Vec<char> = block.text.chars().collect();
    let spans = block.spans.as_deref().unwrap_or(&[]);
    let face = |at: usize| {
        let mut face = (false, false, false);
        for span in spans {
            if span.length == 0 || span.start + span.length > chars.len() || at < span.start || at >= span.start + span.length {
                continue;
            }
            match span.style {
                "bold" => face.0 = true,
                "italic" => face.1 = true,
                _ => face.2 = true,
            }
        }
        face
    };
    let mut runs: Vec<PetTextRun> = Vec::new();
    for (at, c) in chars.iter().enumerate() {
        let (bold, italic, mono) = face(at);
        match runs.last_mut() {
            Some(last) if (last.bold, last.italic, last.mono) == (bold, italic, mono) => last.text.push(*c),
            _ => runs.push(PetTextRun { text: c.to_string(), color: PetColor::default(), bold, italic, mono }),
        }
    }
    if runs.is_empty() {
        runs.push(run(""));
    }
    runs
}

/// C# `Pet`: the pet in its frame, 192 × 208 scaled to fit; with no pet image, the paw in the accent with 35 around it.
pub fn pet(frame: PetRect, atlas: bool, row: i32, column: u32, accent: PetColor) -> Vec<PetItem> {
    if !atlas {
        // pawprint.fill on its 16-unit grid, the same shape as the main window's pet button.
        let side = frame.width.min(frame.height) - 70.0;
        let unit = side / 16.0;
        let origin = PetPoint { x: frame.x + (frame.width - side) / 2.0, y: frame.y + (frame.height - side) / 2.0 };
        let at = |px: f64, py: f64| PetPoint { x: origin.x + unit * px, y: origin.y + unit * py };
        let mut items: Vec<PetItem> = [(2.3, 5.5), (6.0, 2.8), (10.0, 2.8), (13.7, 5.5)]
            .iter()
            .map(|(tx, ty)| PetItem::Ellipse {
                center: at(*tx, *ty),
                radius_x: 1.7 * unit,
                radius_y: 2.0 * unit,
                color: accent,
                stroke: None,
            })
            .collect();
        let cubic =
            |a: (f64, f64), b: (f64, f64), c: (f64, f64)| PetSegment::Cubic { first: at(a.0, a.1), second: at(b.0, b.1), to: at(c.0, c.1) };
        items.push(PetItem::Path {
            start: at(8.0, 6.5),
            segments: vec![
                cubic((5.7, 6.5), (2.0, 10.8), (2.0, 12.8)),
                cubic((2.0, 16.7), (6.0, 14.5), (8.0, 14.5)),
                cubic((10.0, 14.5), (14.0, 16.7), (14.0, 12.8)),
                cubic((14.0, 10.8), (10.3, 6.5), (8.0, 6.5)),
            ],
            color: accent,
        });
        return items;
    }
    let row = row.clamp(0, 8);
    let column = column.min(crate::pet::ROW_FRAMES[row as usize] - 1);
    let (cell_w, cell_h) = (f64::from(crate::pet::CELL_WIDTH), f64::from(crate::pet::CELL_HEIGHT));
    let fit = (frame.width / cell_w).min(frame.height / cell_h);
    let dest = PetRect::new(
        frame.x + (frame.width - cell_w * fit) / 2.0,
        frame.y + (frame.height - cell_h * fit) / 2.0,
        cell_w * fit,
        cell_h * fit,
    );
    vec![PetItem::Sprite { cell: PetRect::new(f64::from(column) * cell_w, f64::from(row) * cell_h, cell_w, cell_h), dest, opacity: 1.0 }]
}

/// The menu's two entries (C# `LayoutMenu`): 펫 숨기기, 에이전트 열기.
pub fn menu_entries() -> [(&'static str, String); 2] {
    [("hide", mighty_run::strings::get("menu.hidePet")), ("open", mighty_run::strings::get("menu.openAgent"))]
}

/// C# `Menu`: the card, a line, radius 8; the row under the pointer on the subtle wash.
pub fn menu(painter: &mut Painter, area: PetRect, items: &[(PetRect, String, String)], hover: Option<usize>) {
    let card = paint(painter.palette[Token::Card], 1.0);
    let line = paint(painter.palette[Token::Line], 1.0);
    let outer = std::mem::take(&mut painter.items);
    painter.fill(area, RADIUS_ROW, card);
    painter.hairline(area, RADIUS_ROW, line, metrics::STROKE_LINE);
    for (index, (bounds, id, label)) in items.iter().enumerate() {
        let row_outer = std::mem::take(&mut painter.items);
        if Some(index) == hover {
            let wash = paint(painter.palette.subtle(), metrics::SUBTLE_OPACITY);
            painter.fill(*bounds, MENU_ROW_RADIUS, wash);
        }
        let words = painter.text(label, MENU_TYPE, 400, (bounds.width - 20.0).max(1.0), 1, None, false);
        painter.put(&words, bounds.x + 10.0, (bounds.y + (bounds.height - words.height) / 2.0).round(), Token::Ink);
        let drawn = std::mem::replace(&mut painter.items, row_outer);
        painter.items.push(PetItem::Group { role: format!("menu:{id}"), rect: *bounds, opacity: 1.0, items: drawn });
    }
    let drawn = std::mem::replace(&mut painter.items, outer);
    painter.items.push(PetItem::Group { role: "menu".into(), rect: area, opacity: 1.0, items: drawn });
}

/// The spinner's arc (C# `Spinner`): three quarters of a ring, turning once a second in twelve steps.
pub fn spinner(center: PetPoint, step: u32, color: PetColor) -> PetItem {
    PetItem::Arc {
        center,
        radius: 4.5,
        start: f64::from(step) * std::f64::consts::TAU / 12.0,
        sweep: std::f64::consts::TAU * 0.75,
        color,
        width: 1.5,
    }
}
