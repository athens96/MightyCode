//! The m1 extension's records and pure rules beside the graph (C# R\MobileMighty.cs: `MobileWire`'s limits, `MobilePermission` with its
//! Antigravity card, `MobileSettings` and its options, `MobileRemoteSupport`, `MobileRateLimit`, `MobileGuidedRequest`; macOS
//! MobileRemoteModels.swift, MobileRemoteSupport.swift:7-185), kept out of the app so the contract can be checked without a window.
//! The graph's own rules (`MobileMightySupport`: blocks, runs, digest; the role view) are `mighty_graph::view::mobile`, re-exported here.
//! Owner: W8-B.

use indexmap::IndexMap;
use mighty_core::json::JsonOut;
use mighty_core::locale::Locale;
use mighty_core::model::{LogEntry, RunSession, SessionRateLimit};
use mighty_core::permission::ToolPermissionRequest;
pub use mighty_graph::view::mobile::*;
use mighty_run::antigravity::permissions::{OnceReason, PermissionRequest, kind_label};
use mighty_run::presentation::ToolPermissionPresentation;
use mighty_run::questionnaire::{UserQuestionAnswer, UserQuestionnaire};
use serde_json::{Map, Value};

use crate::phone::desk::MobileError;

/// `MobilePermissionField(Label, Value)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobilePermissionField {
    pub label: String,
    pub value: String,
}

impl MobileOut for MobilePermissionField {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("label", Some(&self.label));
        out.prop_str("value", Some(&self.value));
        out.end_object();
    }
}

/// `MobileAntigravityAction(Kind, Action, DisplayName, Target?, Rule?, OnceOnly?, OnceOnlyReason?)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileAntigravityAction {
    pub kind: String,
    pub action: String,
    pub display_name: String,
    pub target: Option<String>,
    pub rule: Option<String>,
    pub once_only: Option<bool>,
    pub once_only_reason: Option<String>,
}

impl MobileOut for MobileAntigravityAction {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("kind", Some(&self.kind));
        out.prop_str("action", Some(&self.action));
        out.prop_str("displayName", Some(&self.display_name));
        out.prop_str("target", self.target.as_deref());
        out.prop_str("rule", self.rule.as_deref());
        out.prop_opt_bool("onceOnly", self.once_only);
        out.prop_str("onceOnlyReason", self.once_only_reason.as_deref());
        out.end_object();
    }
}

/// `MobileAntigravityCard(Actions)`: what an Antigravity turn was refused, each action as the card shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileAntigravityCard {
    pub actions: Vec<MobileAntigravityAction>,
}

impl MobileAntigravityCard {
    /// `MobileAntigravityCard.ToolName`.
    pub const TOOL_NAME: &'static str = "AntigravityPermission";

    /// `MobileAntigravityCard.From`.
    pub fn from(card: &PermissionRequest) -> MobileAntigravityCard {
        MobileAntigravityCard {
            actions: card
                .actions
                .iter()
                .map(|a| MobileAntigravityAction {
                    kind: a.kind.clone(),
                    action: a.action.clone(),
                    display_name: a.display_name.clone(),
                    target: a.target.clone(),
                    rule: a.rule.clone(),
                    once_only: a.once_only().then_some(true),
                    // `JsonNamingPolicy.CamelCase.ConvertName(reason.ToString())`.
                    once_only_reason: a.once_only_reason().map(|r| {
                        match r {
                            OnceReason::Risky => "risky",
                            OnceReason::Interpreter => "interpreter",
                            OnceReason::AgyFolder => "agyFolder",
                        }
                        .to_string()
                    }),
                })
                .collect(),
        }
    }
}

impl MobileOut for MobileAntigravityCard {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.name("actions");
        self.actions.write(out);
        out.end_object();
    }
}

/// A questionnaire as `Wire.Json` writes C#'s `UserQuestionnaire`.
pub fn write_questionnaire(out: &mut JsonOut, value: &UserQuestionnaire) {
    out.begin_object();
    out.name("questions");
    out.begin_array();
    for q in &value.questions {
        out.begin_object();
        out.prop_str("header", Some(&q.header));
        out.prop_str("question", Some(&q.question));
        out.prop_bool("multiSelect", q.multi_select);
        out.name("options");
        out.begin_array();
        for o in &q.options {
            out.begin_object();
            out.prop_str("label", Some(&o.label));
            out.prop_str("description", Some(&o.description));
            out.end_object();
        }
        out.end_array();
        out.end_object();
    }
    out.end_array();
    out.end_object();
}

/// `MobilePermission`: a pending request as the phone's card shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobilePermission {
    pub id: String,
    pub run_id: String,
    pub tool_name: String,
    pub title: String,
    pub headline: Option<String>,
    pub fields: Vec<MobilePermissionField>,
    pub summary: String,
    pub can_allow: bool,
    pub questionnaire: Option<UserQuestionnaire>,
    pub plan: Option<String>,
    pub received_at: Option<String>,
    pub antigravity: Option<MobileAntigravityCard>,
}

impl MobilePermission {
    /// `MobilePermission.FromAntigravity`: the card as one permission (toolName `AntigravityPermission`, the actions its fields too), so
    /// a phone that does not know these cards shows them with allow (once) and deny.
    pub fn from_antigravity(card: &PermissionRequest, locale: &Locale) -> MobilePermission {
        MobilePermission {
            id: card.id.clone(),
            run_id: card.run_id.clone(),
            tool_name: MobileAntigravityCard::TOOL_NAME.into(),
            title: locale.get("antigravity.permission.title"),
            headline: None,
            fields: card
                .actions
                .iter()
                .map(|a| MobilePermissionField {
                    label: kind_label(&a.kind),
                    value: a.target.clone().unwrap_or_else(|| a.display_name.clone()),
                })
                .collect(),
            summary: mighty_graph::node::clean(&card.names().join(", "), 2_048, true),
            can_allow: card.can_allow(),
            questionnaire: None,
            plan: None,
            received_at: card.received_at.clone(),
            antigravity: Some(MobileAntigravityCard::from(card)),
        }
    }

    /// `MobilePermission.From`: the structured card the desktop shows; a plan carries its text as `plan` and once more as its one field.
    pub fn from(request: &ToolPermissionRequest, locale: &Locale) -> MobilePermission {
        let plan = if request.can_answer_plan { request.plan() } else { None };
        if let Some(plan) = plan {
            let title = locale.get("plan.card.title");
            return MobilePermission {
                id: request.id.clone(),
                run_id: request.run_id.clone(),
                tool_name: request.tool_name.clone(),
                title: title.clone(),
                headline: None,
                fields: vec![MobilePermissionField { label: title, value: plan.clone() }],
                summary: request.summary.clone(),
                can_allow: request.can_allow,
                questionnaire: None,
                plan: Some(plan),
                received_at: request.received_at.clone(),
                antigravity: None,
            };
        }
        let card = ToolPermissionPresentation::make(&request.tool_name, &request.input_json);
        MobilePermission {
            id: request.id.clone(),
            run_id: request.run_id.clone(),
            tool_name: request.tool_name.clone(),
            title: card.title,
            headline: card.headline,
            fields: card.fields.into_iter().map(|f| MobilePermissionField { label: f.label, value: f.value }).collect(),
            summary: request.summary.clone(),
            can_allow: request.can_allow,
            questionnaire: if request.can_answer_questions { UserQuestionnaire::parse(&request.input_json) } else { None },
            plan: None,
            received_at: None,
            antigravity: None,
        }
    }
}

impl MobileOut for MobilePermission {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("runId", Some(&self.run_id));
        out.prop_str("toolName", Some(&self.tool_name));
        out.prop_str("title", Some(&self.title));
        out.prop_str("headline", self.headline.as_deref());
        out.name("fields");
        self.fields.write(out);
        out.prop_str("summary", Some(&self.summary));
        out.prop_bool("canAllow", self.can_allow);
        if let Some(q) = &self.questionnaire {
            out.name("questionnaire");
            write_questionnaire(out, q);
        }
        out.prop_str("plan", self.plan.as_deref());
        out.prop_str("receivedAt", self.received_at.as_deref());
        if let Some(card) = &self.antigravity {
            out.name("antigravity");
            card.write(out);
        }
        out.end_object();
    }
}

/// `MobileOption(Id, Label)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileOption {
    pub id: String,
    pub label: String,
}

impl MobileOption {
    pub fn new(id: &str, label: &str) -> MobileOption {
        MobileOption { id: id.into(), label: label.into() }
    }
}

impl MobileOut for MobileOption {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("label", Some(&self.label));
        out.end_object();
    }
}

/// `MobileStyleOption(Id, Label, Source?)`: `source` absent for `cli`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileStyleOption {
    pub id: String,
    pub label: String,
    pub source: Option<String>,
}

impl MobileOut for MobileStyleOption {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("label", Some(&self.label));
        out.prop_str("source", self.source.as_deref());
        out.end_object();
    }
}

/// `MobileSettingsOptions`: one pane's pickers, in the desktop menus' order; `efforts` absent when the provider has no effort setting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MobileSettingsOptions {
    pub models: Vec<MobileOption>,
    pub permission_modes: Vec<MobileOption>,
    pub efforts: Option<Vec<MobileOption>>,
    pub mighty_styles: Vec<MobileOption>,
    pub styles: Vec<MobileStyleOption>,
}

impl MobileOut for MobileSettingsOptions {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.name("models");
        self.models.write(out);
        out.name("permissionModes");
        self.permission_modes.write(out);
        if let Some(efforts) = &self.efforts {
            out.name("efforts");
            efforts.write(out);
        }
        out.name("mightyStyles");
        self.mighty_styles.write(out);
        out.name("styles");
        self.styles.write(out);
        out.end_object();
    }
}

/// `MobileSettings`: `editable` false while the pane runs; `effort` absent together with `options.efforts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileSettings {
    pub editable: bool,
    pub model: String,
    pub permission_mode: String,
    pub effort: Option<String>,
    pub agent_view_mode: String,
    pub mighty_style: String,
    pub style_id: String,
    pub options: MobileSettingsOptions,
}

impl MobileOut for MobileSettings {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_bool("editable", self.editable);
        out.prop_str("model", Some(&self.model));
        out.prop_str("permissionMode", Some(&self.permission_mode));
        out.prop_str("effort", self.effort.as_deref());
        out.prop_str("agentViewMode", Some(&self.agent_view_mode));
        out.prop_str("mightyStyle", Some(&self.mighty_style));
        out.prop_str("styleId", Some(&self.style_id));
        out.name("options");
        self.options.write(out);
        out.end_object();
    }
}

/// `MobileRateLimit(Label, UsedPercent, ResetsAt?)`.
#[derive(Debug, Clone, PartialEq)]
pub struct MobileRateLimit {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<String>,
}

impl MobileOut for MobileRateLimit {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("label", Some(&self.label));
        out.prop_float("usedPercent", self.used_percent);
        out.prop_str("resetsAt", self.resets_at.as_deref());
        out.end_object();
    }
}

/// `MobileGuidedRequest(Style, Action, Text)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileGuidedRequest {
    pub style: String,
    pub action: String,
    pub text: String,
}

// ── MobileRemoteSupport ─────────────────────────────────────────────────────────────────────────────────────────────────

/// `MobileRemoteSupport.PhoneEntries`: an `image` entry reaches the phone as the `system` line naming it, and no entry carries picture
/// references.
pub fn phone_entries(entries: &[LogEntry]) -> Vec<LogEntry> {
    entries
        .iter()
        .map(|entry| {
            if entry.kind() == "image" || entry.images.is_some() {
                let mut next = entry.clone();
                if entry.kind() == "image" {
                    next.kind = Some("system".into());
                }
                next.images = None;
                next
            } else {
                entry.clone()
            }
        })
        .collect()
}

/// `MobileRemoteSupport.Page`: the page older than `before`, newest last; a cursor the host no longer holds answers empty.
pub fn page(entries: &[LogEntry], before: &str, limit: usize) -> (Vec<LogEntry>, bool) {
    let Some(index) = entries.iter().position(|e| e.id() == before) else { return (vec![], false) };
    let count = index.min(limit.max(1));
    (entries[index - count..index].to_vec(), index > count)
}

/// `MobileRemoteSupport.ViewMode`: an agent pane is always in the Mighty view.
pub fn view_mode(_raw: Option<&str>) -> &'static str {
    "mighty"
}

/// `MobileRemoteSupport.PreviewLine`: the newest entry with text that is not a tool activity.
pub fn preview_line(session: &RunSession) -> Option<&LogEntry> {
    session.logs().iter().rev().find(|e| e.activity.is_none() && !e.text().is_empty())
}

/// `MobileRemoteSupport.Style`: only the bundled Ouroboros has a word of its own; every other style travels as "cli".
pub fn style(raw: Option<&str>) -> &str {
    match raw {
        Some(raw) if MIGHTY_STYLES.contains(&raw) => raw,
        _ => CLI_STYLE,
    }
}

/// `MobileRemoteSupport.GuidedStylesAvailable`.
pub fn guided_styles_available(kind: &str, provider: &str, view_mode: &str) -> bool {
    kind == "claude" && provider == "claude" && view_mode == "mighty"
}

/// `MobileRemoteSupport.EffortOptions`: None when the provider has no reasoning-effort setting.
pub fn effort_options<S: AsRef<str>>(supported: bool, levels: &[S], locale: &Locale) -> Option<Vec<MobileOption>> {
    supported.then(|| {
        std::iter::once(MobileOption::new("default", &effort_label("default", locale)))
            .chain(levels.iter().map(|l| MobileOption::new(l.as_ref(), &effort_label(l.as_ref(), locale))))
            .collect()
    })
}

/// `MobileRemoteSupport.EffortLabel`.
pub fn effort_label(effort: &str, locale: &Locale) -> String {
    match effort {
        "low" => "Low".into(),
        "medium" => "Medium".into(),
        "high" => "High".into(),
        "xhigh" => "XHigh".into(),
        "max" => "Max".into(),
        "ultra" => locale.get("composer.effort.ultra"),
        _ => "Auto".into(),
    }
}

/// `MobileRemoteSupport.StyleAfter`: the style a settings request leaves the pane with.
pub fn style_after(requested: Option<&str>, shown: &str, stored: Option<&str>) -> Option<String> {
    match requested {
        None => stored.map(str::to_string),
        Some(r) if r == shown => stored.map(str::to_string),
        Some(CLI_STYLE) => None,
        Some(r) => Some(r.to_string()),
    }
}

/// `MobileRemoteSupport.SingleLine`: a style button's text folded to one line.
pub fn single_line(text: &str) -> String {
    text.split(['\r', '\n', '\u{85}', '\u{2028}', '\u{2029}', '\u{0B}', '\u{0C}'])
        .map(|line| line.trim_matches([' ', '\t']))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `MobileRemoteSupport.ArgumentHint`: the argument a listed command takes, in the user's language.
pub fn argument_hint(argument: Option<mighty_composer::slash::SlashArgument>, locale: &Locale) -> Option<String> {
    use mighty_composer::slash::SlashArgument;
    match argument {
        Some(SlashArgument::Model) => Some(locale.get("remote.slashHint.model")),
        Some(SlashArgument::Permission) => Some(locale.get("remote.slashHint.permission")),
        _ => None,
    }
}

/// `AccountUsageSupport.WindowLabel` (mighty-accounts `usage::window_label`, which this crate does not depend on): a rate-limit window's
/// name in the user's language; a grouped window `<window>@<group>` reads "<group> · <window>".
pub fn window_label(kind: &str, locale: &Locale) -> String {
    if let Some(at) = kind.find('@') {
        let group = match &kind[at + 1..] {
            "gemini" => locale.get("accountUsage.antigravityGroupGemini"),
            "claude_gpt" => locale.get("accountUsage.antigravityGroupClaudeGPT"),
            other => other.to_string(),
        };
        return locale.get("accountUsage.groupWindow").replace("{window}", &window_label(&kind[..at], locale)).replace("{group}", &group);
    }
    let lowered = kind.to_lowercase();
    let session = || locale.get("windows.accountUsage.windowSession");
    let weekly = || locale.get("windows.accountUsage.windowWeekly");
    match lowered.as_str() {
        "session" | "five_hour" | "5h" | "primary" => return session(),
        "weekly" | "seven_day" | "7d" | "secondary" => return weekly(),
        "daily" => return locale.get("windows.accountUsage.windowDaily"),
        "monthly" => return locale.get("windows.accountUsage.windowMonthly"),
        "spend_limit" => return locale.get("windows.accountUsage.windowSpendLimit"),
        _ => {}
    }
    let rest = || kind.get(10..).unwrap_or("");
    if lowered.starts_with("seven_day_") {
        return format!("{} {}", weekly(), rest());
    }
    if lowered.starts_with("five_hour_") {
        return format!("{} {}", session(), rest());
    }
    let digits =
        |unit: char| lowered.len() > 1 && lowered.ends_with(unit) && lowered[..lowered.len() - 1].bytes().all(|b| b.is_ascii_digit());
    if digits('m') {
        return locale.get_with("windows.accountUsage.minutes", &[("count", &lowered[..lowered.len() - 1])]);
    }
    if digits('h') {
        return locale.get_with("windows.accountUsage.hours", &[("count", &lowered[..lowered.len() - 1])]);
    }
    kind.replace('_', " ")
}

/// `MobileRemoteSupport.RateLimits`: the account windows a session last reported, under the desktop's names; None when none is
/// measurable.
pub fn rate_limits(limits: Option<&[SessionRateLimit]>, locale: &Locale) -> Option<Vec<MobileRateLimit>> {
    let mapped: Vec<MobileRateLimit> = limits
        .unwrap_or_default()
        .iter()
        .filter_map(|l| {
            let percent = l.percent_used.filter(|p| p.is_finite())?;
            Some(MobileRateLimit {
                label: window_label(&l.kind, locale),
                used_percent: percent.clamp(0.0, 100.0),
                resets_at: l.resets_at.clone(),
            })
        })
        .collect();
    (!mapped.is_empty()).then_some(mapped)
}

fn text<'a>(body: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Value::as_str)
}

fn style_id_ok(style: &str) -> bool {
    let b = style.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..].iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn action_id_ok(action: &str) -> bool {
    let b = action.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b[1..].iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b':' | b'-'))
}

/// `MobileRemoteSupport.GuidedRequest`: a style button's request, `styleId`+`actionId` or the older `style`+`skill`.
pub fn guided_request(body: &Map<String, Value>, locale: &Locale) -> Result<MobileGuidedRequest, MobileError> {
    let style = text(body, "styleId").or_else(|| text(body, "style"));
    let action = if text(body, "styleId").is_some() { text(body, "actionId").or_else(|| text(body, "skill")) } else { text(body, "skill") };
    let Some(style) = style.filter(|s| *s != CLI_STYLE && style_id_ok(s)) else {
        return Err(MobileError::request(400, locale.get("remote.error.unknownStyle"), None));
    };
    let Some(action) = action.filter(|a| action_id_ok(a)) else {
        return Err(MobileError::request(400, locale.get("remote.error.badSkillName"), None));
    };
    let words = text(body, "text").unwrap_or("").trim();
    if words.len() > MAXIMUM_TEXT || words.contains('\0') {
        return Err(MobileError::request(400, locale.get("remote.error.textTooLong"), None));
    }
    Ok(MobileGuidedRequest { style: style.into(), action: action.into(), text: words.into() })
}

/// The phone's `answers` object as C# reads `Dictionary<string, UserQuestionAnswer>` (`Wire.Json`: names without regard to case, a
/// missing `selectedOptions` null); None for anything it cannot read or with a null selection.
pub fn answers(value: &Map<String, Value>) -> Option<IndexMap<String, UserQuestionAnswer>> {
    let mut result = IndexMap::new();
    for (key, item) in value {
        let item = item.as_object()?;
        let object = mighty_core::json::Obj::new(item);
        let selected = match object.get("selectedOptions") {
            Some(Value::Array(list)) => list.iter().map(|v| v.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?,
            _ => return None,
        };
        let custom = match object.get("customText") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            _ => return None,
        };
        result.insert(key.clone(), UserQuestionAnswer { selected_options: selected, custom_text: custom });
    }
    Some(result)
}
