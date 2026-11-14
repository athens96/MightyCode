//! A push's message (C# `MobilePushMessage` and its hand encoder, `MobilePushRequest`, `MobilePushKinds`, `MobilePushButtonKinds`,
//! R\MobilePush.cs:15-195; plan §1.3): fields in Swift's `JSONEncoder` order (`v:1, id, kind, hostId, hostName, workspace, title,
//! summary, sessionId, [permission:{requestId,runId,buttons}], at`), escaping `" \ /`, `\b \f \n \r \t`, other C0 as `\u00xx`, all
//! else raw UTF-8; over 2048 bytes the summary empties, still over the push is skipped. Owner: W8-A.

use mighty_core::permission::{PLAN_TOOL, ToolPermissionRequest};
use mighty_run::antigravity::permissions::PermissionRequest as AntigravityCard;
use mighty_run::questionnaire::UserQuestionnaire;

use crate::push::seal::MAXIMUM_PLAINTEXT;

/// `MobilePushKinds.All`, in the host's order.
pub const KINDS: [&str; 6] = ["completed", "failed", "permission", "question", "plan", "signin"];
/// `permission.buttons`.
pub const BUTTONS: [&str; 4] = ["claude", "antigravity", "antigravityOnce", "none"];

pub const COMPLETED: &str = "completed";
pub const FAILED: &str = "failed";
pub const PERMISSION: &str = "permission";
pub const QUESTION: &str = "question";
pub const PLAN: &str = "plan";
pub const SIGNIN: &str = "signin";

pub const BUTTONS_CLAUDE: &str = "claude";
pub const BUTTONS_ANTIGRAVITY: &str = "antigravity";
pub const BUTTONS_ANTIGRAVITY_ONCE: &str = "antigravityOnce";
pub const BUTTONS_NONE: &str = "none";

/// `MobileAntigravityCard.ToolName`.
pub const ANTIGRAVITY_TOOL: &str = "AntigravityPermission";
/// Claude's question tool.
pub const QUESTION_TOOL: &str = "AskUserQuestion";

/// C# `MobilePushPermission(RequestId, RunId, Buttons)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushPermission {
    pub request_id: String,
    pub run_id: String,
    pub buttons: String,
}

/// C# `MobilePushMessage`: the plaintext inside a push envelope. `Debug` shows only its id, kind and session: never what the
/// notification says.
#[derive(Clone, PartialEq, Eq)]
pub struct PushMessage {
    pub v: i32,
    pub id: String,
    pub kind: String,
    pub host_id: String,
    pub host_name: String,
    pub workspace: String,
    pub title: String,
    pub summary: String,
    pub session_id: String,
    pub permission: Option<PushPermission>,
    pub at: String,
}

impl std::fmt::Debug for PushMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PushMessage {{ id: {:?}, kind: {:?}, session_id: {:?} }}", self.id, self.kind, self.session_id)
    }
}

impl PushMessage {
    /// UTF-8 JSON within the 2048-byte limit; the summary goes first when it does not fit. None means it does not fit even without the
    /// summary: skip this push and log its id only. The limit is measured on the bytes the Mac's `JSONEncoder` writes.
    pub fn encoded_for_push(&self) -> Option<Vec<u8>> {
        let data = self.encode();
        if data.len() <= MAXIMUM_PLAINTEXT {
            return Some(data);
        }
        let data = PushMessage { summary: String::new(), ..self.clone() }.encode();
        (data.len() <= MAXIMUM_PLAINTEXT).then_some(data)
    }

    /// The JSON Swift's `JSONEncoder` writes for this message, keys in declaration order.
    pub fn encode(&self) -> Vec<u8> {
        let mut json = format!("{{\"v\":{}", self.v);
        field(&mut json, "id", &self.id, false);
        field(&mut json, "kind", &self.kind, false);
        field(&mut json, "hostId", &self.host_id, false);
        field(&mut json, "hostName", &self.host_name, false);
        field(&mut json, "workspace", &self.workspace, false);
        field(&mut json, "title", &self.title, false);
        field(&mut json, "summary", &self.summary, false);
        field(&mut json, "sessionId", &self.session_id, false);
        if let Some(permission) = &self.permission {
            json.push_str(",\"permission\":{");
            field(&mut json, "requestId", &permission.request_id, true);
            field(&mut json, "runId", &permission.run_id, false);
            field(&mut json, "buttons", &permission.buttons, false);
            json.push('}');
        }
        field(&mut json, "at", &self.at, false);
        json.push('}');
        json.into_bytes()
    }
}

/// One `"name":"value"` as Swift escapes it: `"`, `\`, `/` and the C0 controls (`\b \f \n \r \t` short, the rest `\u00xx`); DEL,
/// U+2028 and emoji go out as UTF-8.
fn field(json: &mut String, name: &str, value: &str, first: bool) {
    json.push_str(if first { "\"" } else { ",\"" });
    json.push_str(name);
    json.push_str("\":\"");
    for c in value.chars() {
        match c {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '/' => json.push_str("\\/"),
            '\u{8}' => json.push_str("\\b"),
            '\u{c}' => json.push_str("\\f"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            c if (c as u32) < 0x20 => json.push_str(&format!("\\u{:04x}", c as u32)),
            c => json.push(c),
        }
    }
    json.push('"');
}

/// One action of an Antigravity card as a push reads it (C# `MobileAntigravityAction`'s `Rule` and `OnceOnly`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardAction {
    pub rule: Option<String>,
    pub once_only: bool,
}

/// What `MobilePushRequest.From(MobilePermission)` reads of a pending card: its ids, tool, summary, whether it may be allowed, whether
/// it carries a questionnaire or a plan, and an Antigravity card's actions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushCard {
    pub id: String,
    pub run_id: String,
    pub tool_name: String,
    pub summary: String,
    pub can_allow: bool,
    pub questionnaire: bool,
    pub plan: Option<String>,
    pub antigravity: Option<Vec<CardAction>>,
}

/// C# `MobilePushRequest(Id, RunId, Kind, Summary, Buttons)`: a pending request as a push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRequest {
    pub id: String,
    pub run_id: String,
    pub kind: String,
    pub summary: String,
    pub buttons: String,
}

impl PushRequest {
    pub fn new(id: &str, run_id: &str, kind: &str, summary: &str, buttons: &str) -> PushRequest {
        PushRequest { id: id.into(), run_id: run_id.into(), kind: kind.into(), summary: summary.into(), buttons: buttons.into() }
    }

    /// `MobilePushRequest.From(MobilePermission)`: a pending card as the phone sees it, read as a push (M/MobilePushRequest.init(_
    /// permission:)).
    pub fn from_card(card: &PushCard) -> PushRequest {
        let make = |kind: &str, buttons: &str| PushRequest::new(&card.id, &card.run_id, kind, &card.summary, buttons);
        if let Some(actions) = &card.antigravity {
            let ruled: Vec<&CardAction> = actions.iter().filter(|a| a.rule.is_some()).collect();
            let buttons = if !card.can_allow || ruled.is_empty() {
                BUTTONS_NONE
            } else if ruled.iter().any(|a| a.once_only) {
                BUTTONS_ANTIGRAVITY_ONCE
            } else {
                BUTTONS_ANTIGRAVITY
            };
            return make(PERMISSION, buttons);
        }
        if card.questionnaire || card.tool_name == QUESTION_TOOL {
            return make(QUESTION, BUTTONS_NONE);
        }
        if card.plan.is_some() || card.tool_name == PLAN_TOOL {
            return make(PLAN, BUTTONS_NONE);
        }
        make(PERMISSION, if card.can_allow { BUTTONS_CLAUDE } else { BUTTONS_NONE })
    }

    /// `MobilePushRequest.From(ToolPermissionRequest)`: a question reads as its questions and a plan as its first line, not as the
    /// tool's name (its activity summary); either is empty when it cannot be read, so the phone shows its own sentence.
    pub fn from_tool_request(request: &ToolPermissionRequest) -> PushRequest {
        // `MobilePermission.From`: the plan only when the pane can answer it, the questionnaire likewise.
        let plan = if request.can_answer_plan { request.plan() } else { None };
        let card = PushCard {
            id: request.id.clone(),
            run_id: request.run_id.clone(),
            tool_name: request.tool_name.clone(),
            summary: request.summary.clone(),
            can_allow: request.can_allow,
            questionnaire: plan.is_none() && request.can_answer_questions && UserQuestionnaire::parse(&request.input_json).is_some(),
            plan: plan.clone(),
            antigravity: None,
        };
        let push = PushRequest::from_card(&card);
        match push.kind.as_str() {
            QUESTION => {
                let summary = (request.tool_name == QUESTION_TOOL)
                    .then(|| UserQuestionnaire::parse(&request.input_json))
                    .flatten()
                    .map(|q| q.questions.iter().map(|q| q.question.as_str()).collect::<Vec<_>>().join(" · "))
                    .unwrap_or_default();
                PushRequest { summary, ..push }
            }
            PLAN => PushRequest { summary: request.plan().map(|p| plan_line(&p)).unwrap_or_default(), ..push },
            _ => push,
        }
    }

    /// `MobilePushRequest.From(MobilePermission.FromAntigravity(card))`: an Antigravity card is a permission with its buttons from its
    /// rules.
    pub fn from_antigravity(card: &AntigravityCard) -> PushRequest {
        PushRequest::from_card(&PushCard {
            id: card.id.clone(),
            run_id: card.run_id.clone(),
            tool_name: ANTIGRAVITY_TOOL.into(),
            summary: mighty_run::activity::clean(Some(&card.names().join(", ")), 2_048, true),
            can_allow: card.can_allow(),
            questionnaire: false,
            plan: None,
            antigravity: Some(card.actions.iter().map(|a| CardAction { rule: a.rule.clone(), once_only: a.once_only() }).collect()),
        })
    }
}

/// `MobilePushRequest.PlanLine`: the plan's first line with text, without its Markdown heading marks.
pub fn plan_line(plan: &str) -> String {
    plan.split(['\r', '\n'])
        .map(|line| line.trim_start_matches(['#', ' ', '\t']).trim_end_matches([' ', '\t']))
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}
