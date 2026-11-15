//! C#'s companion checks, one test each under its name (Tests\Verification.cs L504, L505, L512, L726-L728): CompanionVerification.cs,
//! CompanionLayoutVerification.cs and QuestionnaireProgressVerification.ThePetAnswersFromItsBubble. The words are Korean, as C#'s
//! `Verification` pins them.

use std::path::{Path, PathBuf};

use mighty_companion::bubble::{self, BubbleButton};
use mighty_companion::layout::{self, Edges};
use mighty_companion::pet::{self, CompanionPet};
use mighty_companion::plan::{self, PlanBlock};
use mighty_companion::prefs::{self, CompanionPreferences};
use mighty_companion::{animation, carousel};
use mighty_core::model::AgentActivity;
use mighty_run::presentation::strings as permission;
use mighty_run::questionnaire::{QuestionnaireProgress, UserQuestionnaire};
use mighty_run::strings::{get, get_with};

fn korean() {
    mighty_run::strings::set_language("ko");
}

fn activity(kind: &str, state: &str, summary: &str) -> AgentActivity {
    AgentActivity {
        id: "r".into(),
        provider: "claude".into(),
        kind: kind.into(),
        state: state.into(),
        summary: summary.into(),
        tool_name: None,
        output: None,
        duration_ms: None,
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

/// L504 "Companion animation and carousel" (`CompanionVerification.AnimationAndCarousel`).
#[test]
fn companion_animation_and_carousel() {
    assert!(pet::frame(0, 0.29, false) == 1 && pet::frame(0, 100.0, true) == 0, "durations/reduced motion");
    for row in 0..9 {
        for tick in 0..100 {
            assert!(pet::frame(row, f64::from(tick) / 30.0, false) < pet::ROW_FRAMES[row as usize], "frame bounds");
        }
    }
    assert_eq!(animation::row("running", Some(&activity("read", "running", "read")), false), 8, "review animation");
    assert_eq!(animation::row("running", Some(&activity("command", "waiting", "blocked")), false), 6, "waiting priority");
    assert!(animation::row("completed", None, true) == 4 && animation::row("error", None, false) == 5, "completion/failure animation");
    let active = strings(&["a", "b", "c"]);
    assert!(
        carousel::shown(Some("b"), &active, Some("a")) == Some("b") && carousel::shown(Some("gone"), &active, Some("a")) == Some("a"),
        "pin clears after completion"
    );
    assert!(
        carousel::step(Some("c"), &active, 1).as_deref() == Some("a")
            && carousel::step(Some("a"), &active, -1).as_deref() == Some("c")
            && carousel::step(None, &active, -1).as_deref() == Some("c"),
        "wrapping pages"
    );
    assert!(
        carousel::step(Some("a"), &strings(&["a"]), 1).is_none() && carousel::position(Some("b"), &active) == Some(2),
        "page visibility"
    );
}

/// L505 "Companion bubble resize edges and DPI anchors" (`CompanionLayoutVerification.EdgesAndAnchors`).
#[test]
fn companion_bubble_resize_edges_and_dpi_anchors() {
    assert!(
        layout::width(None) == 258.0 && layout::width(Some(f64::NAN)) == 258.0 && layout::height(None).is_none(),
        "Default and invalid preferences preserve automatic height."
    );
    assert_eq!(layout::hit(1.0, 1.0, 258.0, 180.0, true), Edges::LEFT | Edges::TOP, "Top-left corner resizes both axes.");
    assert!(
        layout::hit(1.0, 90.0, 258.0, 180.0, true) == Edges::LEFT && layout::hit(130.0, 1.0, 258.0, 180.0, true) == Edges::TOP,
        "Thin side and top strips leave central content interactive."
    );
    assert!(
        layout::hit(130.0, 90.0, 258.0, 180.0, true) == Edges::NONE && layout::hit(-1.0, 90.0, 258.0, 180.0, true) == Edges::NONE,
        "Content and outside release cannot become resize handles."
    );
    assert!(
        layout::hit(130.0, 1.0, 258.0, 180.0, false) == Edges::NONE && layout::hit(1.0, 90.0, 258.0, 180.0, false) == Edges::LEFT,
        "An approval card retains automatic height while its sides resize."
    );
    assert!(
        layout::hit(257.0, 179.0, 258.0, 180.0, true) == Edges::RIGHT && layout::hit(130.0, 179.0, 258.0, 180.0, true) == Edges::NONE,
        "The bubble's bottom meets the pet: no strip and no corner there."
    );
    assert!(
        layout::CHROME == 153.0
            && layout::panel_size(None, None, false) == (282.0, 330.0)
            && layout::panel_size(Some(300.0), Some(100.0), false) == (324.0, 330.0)
            && layout::panel_size(Some(300.0), Some(200.0), false) == (324.0, 353.0),
        "The window follows the bubble's width and a fixed height."
    );
    assert!(
        layout::panel_size(Some(220.0), Some(300.0), true) == (282.0, 494.0)
            && layout::approval_width(Some(220.0)) == 258.0
            && layout::approval_width(Some(400.0)) == 400.0,
        "An approval stands in the taller window and is never narrower than the default."
    );
    for dpi in [1.0, 1.25, 2.0, 4.0] {
        let drag = layout::resize(258.0, 180.0, -60.0 * dpi, -80.0 * dpi, dpi, Edges::LEFT | Edges::TOP);
        assert!(
            drag.width == 318.0 && drag.height == 260.0 && drag.move_x + drag.width == 258.0 && drag.move_y + drag.height == 180.0,
            "Opposite edges stay stationary across DPI scales."
        );
        let clamp = layout::resize(258.0, 180.0, -10000.0 * dpi, -10000.0 * dpi, dpi, Edges::LEFT | Edges::TOP);
        assert!(
            clamp.width == 640.0 && clamp.height == 480.0 && clamp.move_x + clamp.width == 258.0 && clamp.move_y + clamp.height == 180.0,
            "Clamping does not move the pinned edges."
        );
        let side = layout::resize(258.0, 180.0, 20.0 * dpi, 100.0 * dpi, dpi, Edges::RIGHT);
        assert!(
            side.width == 278.0 && side.height == 180.0 && side.move_y == 0.0 && !layout::vertical(Edges::RIGHT),
            "A sideways drag ignores vertical drift and keeps the automatic-height policy."
        );
    }
}

fn temp(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("mighty-companion-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn manifest(source: &Path, path: &str, version: i32) {
    let text = serde_json::json!({ "displayName": "한글 펫", "spritesheetPath": path, "spriteVersionNumber": version }).to_string();
    std::fs::write(source.join("pet.json"), text).unwrap();
}

/// L512 "Companion catalog boundaries" (`CompanionVerification.CatalogBoundaries`): a 33-byte header-only fixture checks the metadata,
/// not a decoder.
#[test]
fn companion_catalog_boundaries() {
    let root = temp("catalog");
    let mut png = vec![0u8; 33];
    png[..8].copy_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    png[8..12].copy_from_slice(&13u32.to_be_bytes());
    png[12..16].copy_from_slice(b"IHDR");
    png[16..20].copy_from_slice(&1536u32.to_be_bytes());
    png[20..24].copy_from_slice(&1872u32.to_be_bytes());
    png[24] = 8;
    png[25] = 6;
    let source = root.join("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("sheet.png"), &png).unwrap();
    manifest(&source, "sheet.png", 1);
    let loaded = CompanionPet::load(&source, "test").expect("valid manifest metadata");
    assert!(loaded.width == 1536 && loaded.height == 1872 && loaded.name == "한글 펫", "valid manifest metadata");
    for path in ["../outside.png", "/tmp/outside.png", "C:\\outside.png", "\\\\server\\image.png", "sheet.png:stream"] {
        manifest(&source, path, 1);
        let refused = CompanionPet::load(&source, "test").expect_err(&format!("escaped pet path accepted: {path}"));
        assert!(refused.starts_with("Invalid Codex pet:"), "{refused}");
    }
    manifest(&source, "sheet.png", 3);
    assert_eq!(CompanionPet::load(&source, "test").expect_err("wrong version accepted"), pet::invalid("version"));
    manifest(&source, "sheet.png", 1);
    png[25] = 2;
    std::fs::write(source.join("sheet.png"), &png).unwrap();
    assert_eq!(CompanionPet::load(&source, "test").expect_err("opaque sheet accepted"), pet::invalid("dimensions or transparency"));
    let profile = root.join("profile");
    std::fs::create_dir_all(&profile).unwrap();
    let saved = CompanionPreferences {
        reduced_motion: true,
        selected_pet: "local:pet".into(),
        left: Some(-100),
        top: Some(200),
        ..CompanionPreferences::default()
    };
    prefs::save(&profile, &saved).unwrap();
    assert_eq!(prefs::load(&profile), saved, "preference roundtrip");
    let installed = loaded.install(&profile).expect("import");
    assert!(installed.id.starts_with("local:") && installed.image == loaded.image, "import uses validated byte snapshot");
    let catalog = CompanionPet::catalog(&root.join("missing"), &profile, Some(&root.join("no-codex")));
    assert!(catalog.len() == 1 && catalog[0].image.is_empty(), "catalog does not retain all atlases");
    #[cfg(unix)]
    {
        let linked = root.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink(profile.join("pets"), linked.join("pets")).unwrap();
        assert_eq!(loaded.install(&linked).expect_err("linked import destination accepted"), pet::invalid("destination"));
    }
    let _ = std::fs::remove_dir_all(&root);
}

fn form(json: &str) -> UserQuestionnaire {
    UserQuestionnaire::parse(json).expect("fixture questionnaire did not parse")
}

const MULTI_FIRST: &str = r#"{"questions":[{"header":"대상","question":"무엇을?","multiSelect":true,"options":[{"label":"X","description":""},{"label":"Y","description":""}]},{"header":"범위","question":"어디까지?","multiSelect":false,"options":[{"label":"A","description":""},{"label":"B","description":""}]}]}"#;
const SINGLES: &str = r#"{"questions":[{"header":"하나","question":"첫째?","multiSelect":false,"options":[{"label":"A","description":""},{"label":"B","description":""}]},{"header":"둘","question":"둘째?","multiSelect":false,"options":[{"label":"C","description":""},{"label":"D","description":""}]}]}"#;
const LONE: &str = r#"{"questions":[{"header":"하나","question":"하나만?","multiSelect":false,"options":[{"label":"A","description":""},{"label":"B","description":""}]}]}"#;
const LONE_MULTI: &str = r#"{"questions":[{"header":"여럿","question":"여럿?","multiSelect":true,"options":[{"label":"A","description":""},{"label":"B","description":""}]}]}"#;

fn ids(questionnaire: &UserQuestionnaire, progress: &QuestionnaireProgress) -> String {
    bubble::question(questionnaire, progress)
        .iter()
        .map(|b| format!("{}{}", b.id, if b.disabled { "-off" } else { "" }))
        .collect::<Vec<_>>()
        .join(",")
}

/// L726 "companion bubble answers questions: tap, back, next and send" (`QuestionnaireProgressVerification.ThePetAnswersFromItsBubble`).
#[test]
fn companion_bubble_answers_questions_tap_back_next_and_send() {
    korean();
    // One single-choice question: a tap sends it; there is nothing to commit.
    let lone = form(LONE);
    let mut one = QuestionnaireProgress::new("one");
    assert!(ids(&lone, &one) == "open,questions,deny" && !bubble::needs_commit(&lone, &one), "a lone single-choice question needs no Send");
    assert!(bubble::choose("missing", &lone, &mut one).is_none(), "a label that was not offered sends nothing");
    let sent = bubble::choose("B", &lone, &mut one).expect("a tap answers it");
    assert!(
        lone.validate_answers(&sent).unwrap()["하나만?"] == "B" && bubble::picked("B", &lone, &one) && !bubble::picked("A", &lone, &one),
        "a tap answers it"
    );
    // One multi-choice question: taps toggle, Send answers.
    let lone_multi = form(LONE_MULTI);
    let mut many = QuestionnaireProgress::new("many");
    assert!(
        ids(&lone_multi, &many) == "open,questions,question-commit-off,deny" && bubble::commit(&lone_multi, &mut many).is_none(),
        "Send waits for a pick"
    );
    assert!(
        bubble::choose("B", &lone_multi, &mut many).is_none()
            && bubble::choose("A", &lone_multi, &mut many).is_none()
            && bubble::picked("A", &lone_multi, &many),
        "a multi-choice tap only toggles"
    );
    let sends: Vec<BubbleButton> = bubble::question(&lone_multi, &many).into_iter().filter(|b| b.id == bubble::NEXT).collect();
    assert!(
        ids(&lone_multi, &many) == "open,questions,question-commit,deny" && sends.len() == 1 && sends[0].label == get("composer.send.name"),
        "Send once something is picked"
    );
    let picks = bubble::commit(&lone_multi, &mut many).expect("Send answers");
    assert_eq!(lone_multi.validate_answers(&picks).unwrap()["여럿?"], "A, B", "Send answers with the picks in option order");
    // Several questions: nothing is sent from a tap, the last one waits for Send, Back shows the recorded pick.
    let singles = form(SINGLES);
    let mut two = QuestionnaireProgress::new("two");
    assert!(
        bubble::question_title(&singles, &two) == get_with("companion.question.titleProgress", &[("current", "1"), ("total", "2")])
            && bubble::question_title(&lone, &one) == get("phone.questionnaire.title"),
        "the title counts only several questions"
    );
    assert_eq!(ids(&singles, &two), "open,questions,deny", "the first single-choice question of several answers at a tap");
    assert!(bubble::choose("A", &singles, &mut two).is_none() && two.index == 1, "a tap records and moves on, sending nothing");
    assert_eq!(
        ids(&singles, &two),
        "open,questions,question-back,question-commit-off,deny",
        "the last question shows Back and a Send that waits for its answer"
    );
    assert!(
        bubble::choose("D", &singles, &mut two).is_none() && two.index == 1 && bubble::picked("D", &singles, &two),
        "the last tap is recorded, not sent"
    );
    two.back(&singles);
    assert!(
        two.index == 0 && bubble::picked("A", &singles, &two) && ids(&singles, &two) == "open,questions,deny",
        "Back shows the recorded pick"
    );
    bubble::choose("B", &singles, &mut two);
    let both = bubble::commit(&singles, &mut two).expect("Send sends every answer");
    let answers = singles.validate_answers(&both).unwrap();
    assert!(answers["첫째?"] == "B" && answers["둘째?"] == "D", "Send sends every answer");
    // A multi-choice question first: Next records its picks and moves on.
    let multi_first = form(MULTI_FIRST);
    let mut mixed = QuestionnaireProgress::new("mixed");
    bubble::choose("Y", &multi_first, &mut mixed);
    let nexts: Vec<BubbleButton> = bubble::question(&multi_first, &mixed).into_iter().filter(|b| b.id == bubble::NEXT).collect();
    assert_eq!(nexts.len(), 1, "one Next");
    let next = &nexts[0];
    assert!(
        next.prominent && !next.disabled && next.label == get("companion.question.next"),
        "Next under a multi-choice question that is not the last"
    );
    assert!(bubble::commit(&multi_first, &mut mixed).is_none() && mixed.index == 1, "Next moves on without sending");
}

fn button_ids(buttons: &[BubbleButton]) -> String {
    buttons.iter().map(|b| b.id.as_str()).collect::<Vec<_>>().join(",")
}

/// L727 "companion bubble buttons for an approval and a finished plan" (`CompanionVerification.BubbleButtons`).
#[test]
fn companion_bubble_buttons_for_an_approval_and_a_finished_plan() {
    korean();
    let approval = bubble::approval(true);
    assert_eq!(button_ids(&approval), "open,review,deny,allow", "an approval: open and the whole request, then Deny and Allow once");
    assert!(
        approval[0].leading
            && !approval[0].icon
            && approval[1].leading
            && approval[1].icon
            && !approval[2].leading
            && !approval[2].prominent
            && approval[3].prominent
            && !approval[3].disabled,
        "open at the left, Allow once prominent"
    );
    assert!(
        approval[2].label == permission::button_deny()
            && approval[3].label == permission::button_allow_once()
            && approval[0].label == get("companion.button.open"),
        "the pane card's own words"
    );
    let blocked = bubble::approval(false);
    assert!(
        button_ids(&blocked) == "open,deny,allow" && blocked[2].prominent && blocked[2].disabled,
        "a request that cannot be allowed keeps Allow once, off"
    );
    let plan = bubble::plan(false);
    assert!(
        button_ids(&plan) == "open,plan-review,plan-cancel,plan-revise,plan-approve-confirm,plan-approve" && plan.len() == 6,
        "a plan's six buttons, in the Mac's order"
    );
    assert!(
        plan[..2].iter().all(|b| b.icon && b.leading && !b.own_row)
            && plan[2..4].iter().all(|b| !b.icon && !b.leading && !b.own_row)
            && plan[4..].iter().all(|b| b.own_row),
        "two symbols at the left, Cancel and Request changes at the right, each approval on its own row"
    );
    assert!(
        plan.iter().filter(|b| b.prominent).count() == 1 && plan[5].prominent && plan.iter().all(|b| !b.disabled),
        "only the approval that edits by itself is prominent"
    );
    let words: Vec<String> = [
        "companion.button.open",
        "companion.plan.review",
        "plan.card.cancel",
        "plan.card.revise",
        "plan.card.approveConfirm",
        "plan.card.approveAuto",
    ]
    .iter()
    .map(|key| get(key))
    .collect();
    assert_eq!(plan.iter().map(|b| b.label.clone()).collect::<Vec<_>>(), words, "each button's own words");
    assert!(bubble::plan(true).iter().all(|b| b.disabled), "every button is off while an answer is on its way");
}

fn shape(block: &PlanBlock) -> String {
    format!(
        "{}{}{}:{}",
        block.kind,
        if block.level > 0 { block.level.to_string() } else { String::new() },
        block.marker.as_ref().map_or(String::new(), |m| format!("[{m}]")),
        block.text
    )
}

fn runs(block: &PlanBlock) -> String {
    let chars: Vec<char> = block.text.chars().collect();
    block
        .spans
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|s| format!("{}={}", s.style, chars[s.start..s.start + s.length].iter().collect::<String>()))
        .collect::<Vec<_>>()
        .join(",")
}

/// L728 "companion bubble draws the whole plan as blocks" (`CompanionVerification.PlanBlocks`).
#[test]
fn companion_bubble_draws_the_whole_plan_as_blocks() {
    let source = "# Ship the **pet**\n\nFirst line\nsecond `line` of *one* paragraph.\n\n1. Draw the plan\n2) Route its buttons\n   over two lines\n- [x] done\n- [ ] not `yet`\n  - nested_snake_case item\n\n> quoted\n> words\n\n```cs\nvar a = \"*not bold*\";\n```\n---\n| a | b |\n|---|---|\n\nSee [the doc](https://example.com/x) and ![a picture](p.png) \\*as is\\*";
    let blocks = plan::plan_blocks(source);
    let shapes: Vec<String> = blocks.iter().map(shape).collect();
    assert_eq!(
        shapes,
        [
            "heading1:Ship the pet",
            "paragraph:First line second line of one paragraph.",
            "item[1.]:Draw the plan",
            "item[2.]:Route its buttons over two lines",
            "item[☑]:done",
            "item[☐]:not yet",
            "item1[•]:nested_snake_case item",
            "quote:quoted words",
            "code:var a = \"*not bold*\";",
            "rule:",
            "code:| a | b |\n|---|---|",
            "paragraph:See the doc and a picture *as is*",
        ],
        "the plan's blocks"
    );
    assert!(
        runs(&blocks[0]) == "bold=pet"
            && runs(&blocks[1]) == "code=line,italic=one"
            && runs(&blocks[5]) == "code=yet"
            && blocks[6].spans.is_none()
            && blocks[8].spans.is_none(),
        "bold, italic and code runs; an underscore inside a word is the word's"
    );
    let nested = runs(&plan::plan_blocks("**a `b` *c***")[0]);
    assert!(nested.contains("code=b") && nested.contains("bold=a b c"), "runs inside runs: {nested}");
    // Nothing is dropped: an unclosed mark or fence stays as written, and a plan is never cut short.
    let stray = plan::plan_blocks("```\nopen");
    assert!(
        plan::plan_blocks("a * b ** c _ d ` e")[0].text == "a * b ** c _ d ` e"
            && stray.len() == 1
            && stray[0].kind == "code"
            && stray[0].text == "open",
        "stray marks are words"
    );
    let many = (1..=400).map(|i| format!("{i}. step {i}")).collect::<Vec<_>>().join("\n");
    let all = plan::plan_blocks(&many);
    assert!(all.len() == 400 && all[399].text == "step 400" && all[399].marker.as_deref() == Some("400."), "the whole plan, every step");
    assert!(
        plan::plan_blocks("# Learn C#")[0].text == "Learn C#"
            && plan::plan_blocks("## Closed ##")[0].text == "Closed"
            && plan::plan_blocks("#").is_empty(),
        "a heading keeps a # of its own words and drops only a closing run"
    );
    assert!(plan::plan_blocks("").is_empty() && plan::plan_blocks(" \n\n").is_empty(), "an empty plan draws nothing");
    assert!(!plan::plan_blocks(&"*".repeat(60_000)).is_empty(), "a plan of stray marks is still read");
    let huge = format!("# {}", "가".repeat(plan::MAXIMUM_PLAN_BYTES / 3 + 1));
    let literal = plan::plan_blocks(&huge);
    assert!(
        literal.len() == 1 && literal[0].kind == "paragraph" && literal[0].text == huge,
        "a source past the render bound is one literal paragraph"
    );
}
