//! What the pet's bubble offers for the request it shows (C# Core\CompanionBubble.cs, the Mac's AgentCompanionViews.swift:299-452 and
//! AgentCompanion.swift:295-332; plan §1.7): which buttons, and how a question is answered there (a tap, Back, Next and Send). The plan's
//! blocks are [`crate::plan`]. The words are the app's (`Locale.Get`, the pane card's own). Owner: W8-E.

use indexmap::IndexMap;
use mighty_run::presentation::strings as permission;
use mighty_run::questionnaire::{QuestionnaireProgress, UserQuestionAnswer, UserQuestionnaire};
use mighty_run::strings::{get, get_with};

pub const OPEN: &str = "open";
pub const REVIEW: &str = "review";
pub const DENY: &str = "deny";
pub const ALLOW: &str = "allow";
pub const CUSTOM: &str = "questions";
pub const BACK: &str = "question-back";
pub const NEXT: &str = "question-commit";
pub const CHOICE: &str = "choice-";
pub const PLAN_REVIEW: &str = "plan-review";
pub const PLAN_CANCEL: &str = "plan-cancel";
pub const PLAN_REVISE: &str = "plan-revise";
pub const PLAN_APPROVE_CONFIRM: &str = "plan-approve-confirm";
pub const PLAN_APPROVE: &str = "plan-approve";

/// C# `CompanionBubbleButton(Id, Label, Prominent, Icon, Leading, OwnRow, Disabled)`: drawn as its symbol alone when `icon` (`label`
/// is what it is called), at the bubble's left edge when `leading`, on a row of its own under the others when `own_row`, faded and not
/// clickable when `disabled`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BubbleButton {
    pub id: String,
    pub label: String,
    pub prominent: bool,
    pub icon: bool,
    pub leading: bool,
    pub own_row: bool,
    pub disabled: bool,
}

impl BubbleButton {
    fn new(id: &str, label: String) -> BubbleButton {
        BubbleButton { id: id.into(), label, ..BubbleButton::default() }
    }
}

/// `CompanionBubble.Approval(canAllow)`: open at the left, Deny and Allow once at the right, Allow once off for a request that cannot be
/// allowed here; the whole request (Windows keeps its window for it, here the pane's own card) behind a symbol beside open.
pub fn approval(can_allow: bool) -> Vec<BubbleButton> {
    let mut buttons = vec![BubbleButton { leading: true, ..BubbleButton::new(OPEN, get("companion.button.open")) }];
    if can_allow {
        buttons.push(BubbleButton { icon: true, leading: true, ..BubbleButton::new(REVIEW, get("companion.button.review")) });
    }
    buttons.push(BubbleButton::new(DENY, permission::button_deny()));
    buttons.push(BubbleButton { prominent: true, disabled: !can_allow, ..BubbleButton::new(ALLOW, permission::button_allow_once()) });
    buttons
}

/// `CompanionBubble.Question(questionnaire, progress)`: open as its symbol, the answer written out (the pane's card) as another, Back
/// from the second question on, Next or Send where a tap alone does not answer, and Cancel.
pub fn question(questionnaire: &UserQuestionnaire, progress: &QuestionnaireProgress) -> Vec<BubbleButton> {
    let mut buttons = vec![
        BubbleButton { icon: true, leading: true, ..BubbleButton::new(OPEN, get("companion.button.open")) },
        BubbleButton { icon: true, leading: true, ..BubbleButton::new(CUSTOM, get("companion.button.answer")) },
    ];
    if progress.index > 0 {
        buttons.push(BubbleButton::new(BACK, get("companion.question.back")));
    }
    if needs_commit(questionnaire, progress) {
        let key = if progress.index + 1 < questionnaire.questions.len() { "companion.question.next" } else { "composer.send.name" };
        buttons.push(BubbleButton { prominent: true, disabled: !can_commit(questionnaire, progress), ..BubbleButton::new(NEXT, get(key)) });
    }
    buttons.push(BubbleButton::new(DENY, get("phone.questionnaire.cancel")));
    buttons
}

/// `CompanionBubble.Plan(sending)`: open and the plan's own view as symbols, Cancel and Request changes, then the two approvals each on
/// a row of its own, the one that edits by itself last and prominent; all off while an answer is on its way.
pub fn plan(sending: bool) -> Vec<BubbleButton> {
    vec![
        BubbleButton { icon: true, leading: true, disabled: sending, ..BubbleButton::new(OPEN, get("companion.button.open")) },
        BubbleButton { icon: true, leading: true, disabled: sending, ..BubbleButton::new(PLAN_REVIEW, get("companion.plan.review")) },
        BubbleButton { disabled: sending, ..BubbleButton::new(PLAN_CANCEL, get("plan.card.cancel")) },
        BubbleButton { disabled: sending, ..BubbleButton::new(PLAN_REVISE, get("plan.card.revise")) },
        BubbleButton { own_row: true, disabled: sending, ..BubbleButton::new(PLAN_APPROVE_CONFIRM, get("plan.card.approveConfirm")) },
        BubbleButton { prominent: true, own_row: true, disabled: sending, ..BubbleButton::new(PLAN_APPROVE, get("plan.card.approveAuto")) },
    ]
}

/// `QuestionTitle`: "선택 요청 2/3" while stepping through several questions.
pub fn question_title(questionnaire: &UserQuestionnaire, progress: &QuestionnaireProgress) -> String {
    let total = questionnaire.questions.len();
    if total > 1 {
        get_with("companion.question.titleProgress", &[("current", &(progress.index + 1).to_string()), ("total", &total.to_string())])
    } else {
        get("phone.questionnaire.title")
    }
}

/// `NeedsCommit`: Next or Send under a multi-choice question and on the last of several questions.
pub fn needs_commit(questionnaire: &UserQuestionnaire, progress: &QuestionnaireProgress) -> bool {
    progress.current(questionnaire).is_some_and(|question| {
        question.multi_select || questionnaire.questions.len() > 1 && progress.index + 1 == questionnaire.questions.len()
    })
}

/// `CanCommit`: something picked under a multi-choice question, an answer recorded under a single-choice one.
pub fn can_commit(questionnaire: &UserQuestionnaire, progress: &QuestionnaireProgress) -> bool {
    progress.current(questionnaire).is_some_and(|question| {
        if question.multi_select { !progress.selected().is_empty() } else { progress.answers().contains_key(&question.question) }
    })
}

/// `Picked`: toggled picks of a multi-choice question; the recorded answer when the user came back to one.
pub fn picked(label: &str, questionnaire: &UserQuestionnaire, progress: &QuestionnaireProgress) -> bool {
    progress.current(questionnaire).is_some_and(|question| {
        if question.multi_select {
            progress.selected().iter().any(|s| s == label)
        } else {
            progress.answers().get(&question.question).is_some_and(|answer| answer.selected_options.iter().any(|s| s == label))
        }
    })
}

/// `Choose`: a single-choice tap records and moves on, a multi-choice tap toggles; the answers to send come back only for a lone
/// single-choice question.
pub fn choose(
    label: &str,
    questionnaire: &UserQuestionnaire,
    progress: &mut QuestionnaireProgress,
) -> Option<IndexMap<String, UserQuestionAnswer>> {
    let step = progress.choose(label, questionnaire);
    if questionnaire.questions.len() == 1 { step.and_then(|s| s.answers) } else { None }
}

/// `Commit` (Next or Send): a multi-choice question's picks are recorded; the answers come back once every question has one.
pub fn commit(questionnaire: &UserQuestionnaire, progress: &mut QuestionnaireProgress) -> Option<IndexMap<String, UserQuestionAnswer>> {
    let question = progress.current(questionnaire)?.clone();
    if !can_commit(questionnaire, progress) {
        return None;
    }
    if question.multi_select {
        return progress.commit(None, questionnaire).and_then(|step| step.answers);
    }
    questionnaire.validate_answers(progress.answers()).ok()?;
    Some(progress.answers().clone())
}
