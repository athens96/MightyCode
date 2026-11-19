//! The overlay over the fake pet window (W8-E): C#'s companion smoke's measurements that need no real window (the first place, the
//! window and bubble geometry, a press bound to its card, the menu, the resize strips and their anchors, a long plan's scroll), and the
//! scene of each animation row and each card kind compared, as JSON, with the checked-in `tests/golden/scenes.json` (laid out with the
//! platform's approximate measure, so it is the same on every OS). `MIGHTY_COMPANION_BLESS=1` writes the golden afresh.

use std::sync::Arc;

use mighty_companion::bubble;
use mighty_companion::card::{CompanionCard, CompanionOption, CompanionRequest};
use mighty_companion::layout::{self, Edges};
use mighty_companion::prefs::CompanionPreferences;
use mighty_companion::progress::{self, CompanionProgress, ProgressLine};
use mighty_companion::{Companion, CompanionAction};
use mighty_graph::flow::steps::ProgressState;
use mighty_platform::fake::FakePet;
use mighty_platform::pet::PetItem;
use mighty_platform::pet::{PetBitmap, PetScene};
use mighty_platform::surface::PetInput;
use mighty_platform::{LogicalPoint, LogicalRect, PetEvent, PetWindow};
use serde_json::{Value, json};

fn korean() {
    mighty_run::strings::set_language("ko");
}

fn open(pet: &FakePet) -> Companion {
    Companion::new(Arc::new(pet.clone()) as Arc<dyn PetWindow>, &CompanionPreferences::default())
}

fn task(key: &str, status: &str) -> CompanionCard {
    CompanionCard {
        key: key.into(),
        light: false,
        status: status.into(),
        workspace: "MightyClaude".into(),
        title: "Claude".into(),
        state: "응답 대기".into(),
        elapsed: Some("00:42".into()),
        request: Some("Make the pet's bubble follow the Mac".into()),
        work: "사용자 응답을 기다리는 중".into(),
        ..CompanionCard::default()
    }
}

fn approval_request(can_allow: bool) -> CompanionRequest {
    CompanionRequest {
        title: "승인 요청".into(),
        origin: "MightyClaude · Claude".into(),
        detail: "명령 실행 · Bash".into(),
        headline: Some("Run the Windows core tests before the commit".into()),
        code: Some("dotnet test native/windows/MightyCode.Core.Tests -c Release --no-build --logger trx".into()),
        boxed: true,
        buttons: bubble::approval(can_allow),
        ..CompanionRequest::default()
    }
}

fn question_request() -> CompanionRequest {
    CompanionRequest {
        question: true,
        title: "선택 요청 1/2".into(),
        origin: "MightyClaude · Claude".into(),
        headline: Some("Which layout should the pet's bubble follow?".into()),
        multi_select: true,
        options: vec![
            CompanionOption { label: "The Mac's".into(), description: "The same order, type and paddings".into(), picked: true },
            CompanionOption { label: "The earlier Windows card".into(), description: String::new(), picked: false },
        ],
        buttons: vec![
            bubble::BubbleButton { id: bubble::OPEN.into(), label: "열기".into(), icon: true, leading: true, ..Default::default() },
            bubble::BubbleButton { id: bubble::CUSTOM.into(), label: "답 쓰기".into(), icon: true, leading: true, ..Default::default() },
            bubble::BubbleButton { id: bubble::NEXT.into(), label: "다음".into(), prominent: true, ..Default::default() },
            bubble::BubbleButton { id: bubble::DENY.into(), label: "취소".into(), ..Default::default() },
        ],
        ..CompanionRequest::default()
    }
}

fn plan_request(plan: &str) -> CompanionRequest {
    CompanionRequest {
        title: "계획".into(),
        origin: "MightyClaude · Claude".into(),
        plan: Some(plan.into()),
        received: Some("08:05에 받음".into()),
        buttons: bubble::plan(false),
        ..CompanionRequest::default()
    }
}

fn line(text: &str, state: ProgressState, agent: Option<bool>, time: Option<&str>) -> ProgressLine {
    ProgressLine { text: text.into(), state, agent, time: time.map(str::to_string) }
}

/// A request's progress card: a step of each state, two left out, a sub-agent and a command in the background.
fn progress_card() -> CompanionProgress {
    CompanionProgress {
        title: "진행 상황".into(),
        heading: "요청 27".into(),
        count: Some("3/7".into()),
        state_word: "실행 중".into(),
        status: "running".into(),
        running: true,
        rows: vec![
            line("3. Read the Swift card", ProgressState::Done, None, Some("12초")),
            line("4. Port the model to the shared crate", ProgressState::Current, None, Some("1분 4초")),
            line("5. Draw it through the pet's display list", ProgressState::Pending, None, None),
            line("6. Stopped step", ProgressState::Stopped, None, Some("3초")),
            line("7. Failed step", ProgressState::Failed, None, Some("8초")),
        ],
        more: Some("외 2개".into()),
        background_title: "백그라운드".into(),
        background_count: "1/2".into(),
        background: vec![
            line("Review the diff", ProgressState::Current, Some(true), Some("20초")),
            line("cargo test -p mighty-companion", ProgressState::Done, Some(false), Some("41초")),
        ],
        background_more: None,
    }
}

fn with(card: CompanionCard, request: CompanionRequest) -> CompanionCard {
    CompanionCard { approval: Some(request), ..card }
}

/// Every event the fake window has for the companion, acted on in order; the actions it asked for.
fn pump(companion: &mut Companion, events: &mut tokio::sync::mpsc::UnboundedReceiver<PetEvent>) -> Vec<CompanionAction> {
    let mut actions = vec![];
    while let Ok(event) = events.try_recv() {
        if let Some(action) = companion.act(&event) {
            actions.push(action);
        }
    }
    actions
}

fn click(
    companion: &mut Companion,
    pet: &FakePet,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<PetEvent>,
    id: &str,
) -> Option<Vec<CompanionAction>> {
    let at = companion.target(id)?;
    pet.draw(&companion.scene()).unwrap();
    pet.send(PetInput::Click(at)).unwrap();
    Some(pump(companion, events))
}

fn act(card: &str, action: &str) -> CompanionAction {
    CompanionAction::Act { card: card.into(), action: action.into() }
}

/// C#'s smoke: the first place, the window and bubble geometry, presses bound to the card shown, the menu, and the resize strips.
#[test]
fn the_overlay_measures_as_the_csharp_smoke_does() {
    korean();
    for scale in [1.0, 1.5, 2.0] {
        let pet = FakePet::default();
        pet.set_scale(scale);
        pet.set_work_area(LogicalRect { x: 0.0, y: 0.0, width: 2560.0, height: 1400.0 });
        let mut companion = open(&pet);
        let mut events = pet.events().unwrap();
        companion.set_card(Some(CompanionCard { position: 1, count: 2, ..task("smoke-pet", "waiting") }), true);
        let placed = companion.placement();
        assert_eq!(placed.right, 2560 - (300.0 * scale).round() as i32 + (282.0 * scale).ceil() as i32, "the first place at {scale}");
        assert_eq!(placed.bottom, 1400 - (24.0 * scale).round() as i32, "24 above the bottom at {scale}");
        let geometry = companion.geometry();
        assert_eq!((geometry.width, geometry.height), (282.0, 330.0));
        assert_eq!((geometry.bubble.x, geometry.bubble.width, geometry.bubble.bottom()), (12.0, 258.0, 185.0));
        assert_eq!((geometry.pet.x, geometry.pet.y, geometry.pet.width, geometry.pet.height), (78.5, 187.0, 125.0, 135.0));
        let scene = companion.scene();
        assert_eq!(scene.pixel_size(), ((282.0 * scale).ceil() as u32, (330.0 * scale).ceil() as u32));
        // A press answers for the card it was made on.
        assert_eq!(click(&mut companion, &pet, &mut events, "next").unwrap(), vec![act("smoke-pet", "next")]);
        assert_eq!(click(&mut companion, &pet, &mut events, "toggle").unwrap(), vec![act("smoke-pet", "toggle")]);
        // The pet's menu: hide and open, in the Mac's order; choosing one acts and puts it away, a click beside it only puts it away.
        companion.set_keep_menu(true);
        let pet_center = companion.target("toggle").unwrap();
        pet.send(PetInput::RightClick(pet_center)).unwrap();
        pump(&mut companion, &mut events);
        let menu: Vec<String> = companion.menu().into_iter().map(|(id, _)| id).collect();
        assert_eq!(menu, ["hide", "open"]);
        assert_eq!(click(&mut companion, &pet, &mut events, "open").unwrap(), vec![act("smoke-pet", "open")]);
        assert!(companion.menu().is_empty());
        pet.send(PetInput::RightClick(pet_center)).unwrap();
        pump(&mut companion, &mut events);
        assert_eq!(companion.menu().len(), 2);
        assert_eq!(click(&mut companion, &pet, &mut events, "toggle").unwrap(), vec![], "a click beside the menu only puts it away");
        assert!(companion.menu().is_empty());
    }
}

#[test]
fn the_bubble_follows_its_content_and_its_edges() {
    korean();
    let pet = FakePet::default();
    let mut companion = open(&pet);
    companion.set_card(Some(CompanionCard { request: Some("short".into()), work: "short".into(), ..task("compact", "running") }), true);
    let compact = companion.placement();
    let wordy = CompanionCard {
        title: "A long pane title ".repeat(6).trim().into(),
        request: Some("Requested task ".repeat(30).trim().into()),
        work: "Current work ".repeat(30).trim().into(),
        ..task("layout", "running")
    };
    companion.set_card(Some(wordy), true);
    let automatic = companion.placement();
    assert!(automatic.automatic_height && automatic.height != compact.height && automatic.height < 480, "the height follows the content");
    let scene = companion.scene();
    let texts = scene.texts();
    let title = texts.iter().find(|t| t.size == 11.0 && t.asked == 600).unwrap();
    let state = texts.iter().find(|t| t.size == 9.0 && t.asked == 400).unwrap();
    assert!(
        title.lines == 1 && title.trimmed && title.bounds.right() <= state.bounds.x - 14.0 + 0.01,
        "a long title ends before the state word"
    );
    let bubble = companion.geometry().bubble;
    for words in texts.iter().filter(|t| t.size == 11.0 && t.asked == 400) {
        assert!(
            words.lines == 2 && words.trimmed && words.bounds.bottom() <= bubble.bottom() - 12.0,
            "long words end on their second line"
        );
    }
    assert!(
        companion.resize_hit(1.0, f64::from(automatic.height) / 2.0) == Edges::LEFT
            && companion.resize_hit(f64::from(automatic.width) / 2.0, 1.0) == Edges::TOP
    );
    assert!(
        companion.resize_hit(f64::from(automatic.width) - 2.0, f64::from(automatic.height) - 2.0) == Edges::RIGHT
            && companion.resize_hit(f64::from(automatic.width) / 2.0, f64::from(automatic.height) - 2.0) == Edges::NONE
    );
    let moved = companion.smoke_resize(Edges::LEFT, -60.0, 0.0, false);
    let wider = companion.placement();
    assert!(
        wider.width == automatic.width + 60 && wider.right == automatic.right && wider.automatic_height,
        "a side drag keeps the opposite edge"
    );
    assert!(matches!(moved, Some(CompanionAction::Resized { width: Some(w), height: None, .. }) if w == f64::from(wider.width)));
    companion.smoke_resize(Edges::TOP, 0.0, -40.0, false);
    let taller = companion.placement();
    assert!(taller.height == wider.height + 40 && taller.bottom == wider.bottom && !taller.automatic_height, "a top drag pins the bottom");
    let lines: u32 = companion.scene().texts().iter().filter(|t| t.size == 11.0 && t.asked == 400).map(|t| t.lines).sum();
    assert!(lines > 4, "a taller bubble shows more of the request and the work");
    assert_eq!(companion.smoke_resize(Edges::RIGHT, 40.0, 0.0, true), None);
    assert_eq!(companion.placement(), taller, "a cancelled resize changes nothing");
    assert!(matches!(companion.reset_size(), CompanionAction::Resized { width: None, height: None, .. }));
    assert!(companion.placement().automatic_height && companion.placement().width == layout::DEFAULT_WIDTH as i32);
}

#[test]
fn requests_stand_in_the_taller_window_and_answer_for_themselves() {
    korean();
    let pet = FakePet::default();
    let mut companion = open(&pet);
    let mut events = pet.events().unwrap();
    for (key, request) in
        [("approval", approval_request(true)), ("question", question_request()), ("plan", plan_request("# Ship\n\n1. Draw\n2. Route"))]
    {
        companion.set_card(Some(with(task(key, "waiting"), request)), true);
        let geometry = companion.geometry();
        assert!(
            geometry.width == 282.0 && geometry.height == layout::TALL_HEIGHT && geometry.bubble.bottom() == layout::TALL_HEIGHT - 145.0,
            "{key} in the taller window"
        );
    }
    companion.set_card(Some(with(task("approval", "waiting"), approval_request(true))), true);
    let mut clicked = vec![];
    for id in ["allow", "review", "deny", "open"] {
        clicked.extend(click(&mut companion, &pet, &mut events, id).unwrap());
    }
    assert_eq!(clicked, ["allow", "review", "deny", "open"].map(|a| act("approval", a)));
    companion.set_card(Some(with(task("approval", "waiting"), approval_request(false))), true);
    assert!(companion.target("allow").is_none() && companion.target("review").is_none(), "a request that cannot be allowed takes no press");
    assert!(companion.scene().texts().iter().any(|t| t.value == mighty_run::presentation::strings::button_allow_once()));
    companion.set_card(Some(with(task("question", "waiting"), question_request())), true);
    let mut clicked = vec![];
    for id in ["choice-1", "question-commit", "questions", "deny", "open"] {
        clicked.extend(click(&mut companion, &pet, &mut events, id).unwrap());
    }
    assert_eq!(clicked, ["choice-1", "question-commit", "questions", "deny", "open"].map(|a| act("question", a)));
    // A long plan scrolls inside a bubble that fits the taller window, its answers under it.
    let steps: Vec<String> = (1..=60).map(|i| format!("{i}. Long plan step {i}")).collect();
    companion.set_card(Some(with(task("plan-long", "waiting"), plan_request(&format!("# A long plan\n\n{}", steps.join("\n"))))), true);
    let scrolling = companion.plan_box();
    assert!(
        companion.placement().height as f64 <= layout::TALL_HEIGHT - layout::CHROME
            && scrolling.extent > scrolling.area.height
            && scrolling.scroll == 0.0
    );
    let center = LogicalPoint { x: scrolling.area.x + scrolling.area.width / 2.0, y: scrolling.area.y + scrolling.area.height / 2.0 };
    pet.draw(&companion.scene()).unwrap();
    pet.send(PetInput::Wheel { at: center, delta: -120 }).unwrap();
    pump(&mut companion, &mut events);
    let once = companion.plan_box().scroll;
    assert!(once > 0.0 && once < scrolling.extent - scrolling.area.height, "one notch moves the plan a little");
    for _ in 0..60 {
        pet.send(PetInput::Wheel { at: center, delta: -120 }).unwrap();
    }
    pump(&mut companion, &mut events);
    let end = companion.plan_box();
    assert!((end.scroll - (end.extent - end.area.height)).abs() < 0.01, "the wheel reaches the plan's last step");
    let texts: Vec<String> = companion.scene().texts().iter().map(|t| t.value.clone()).collect();
    assert!(texts.contains(&"Long plan step 60".to_string()) && !texts.contains(&"Long plan step 1".to_string()));
    let mut clicked = vec![];
    for id in ["plan-approve", "plan-cancel"] {
        clicked.extend(click(&mut companion, &pet, &mut events, id).unwrap());
    }
    assert_eq!(clicked, [act("plan-long", "plan-approve"), act("plan-long", "plan-cancel")]);
}

#[test]
fn a_carry_moves_the_window_and_saves_where_it_was_let_go() {
    korean();
    let pet = FakePet::default();
    let mut companion = open(&pet);
    let mut events = pet.events().unwrap();
    companion.set_card(Some(task("carry", "running")), false);
    pet.draw(&companion.scene()).unwrap();
    let start = companion.placement();
    let center = companion.target("toggle").unwrap();
    pet.send(PetInput::Press(center)).unwrap();
    pet.send(PetInput::Drag(LogicalPoint { x: center.x - 200.0, y: center.y - 100.0 })).unwrap();
    pet.send(PetInput::Release(LogicalPoint { x: center.x - 200.0, y: center.y - 100.0 })).unwrap();
    let actions = pump(&mut companion, &mut events);
    let moved = companion.placement();
    assert!(moved.right == start.right - 200 && moved.bottom == start.bottom - 100);
    assert!(matches!(actions.as_slice(), [CompanionAction::Moved { .. }]), "{actions:?}");
    assert!(pet.calls().iter().any(|call| matches!(call, mighty_platform::fake::PetCall::Placed(_))), "the window moved with the pointer");
}

fn atlas() -> PetBitmap {
    PetBitmap { key: "mighty-raccoon@golden".into(), width: 1536, height: 1872, bgra: Arc::new(vec![0; 1536 * 1872 * 4]) }
}

fn snapshot(scene: &PetScene) -> Value {
    serde_json::to_value(scene).unwrap()
}

/// The golden: each animation row's scene (with the atlas and with the paw) and each card kind's.
#[test]
fn every_row_and_card_kind_draws_its_golden_scene() {
    korean();
    let pet = FakePet::default();
    let mut companion = open(&pet);
    let mut rows = vec![];
    companion.set_atlas(Some(atlas()));
    for row in 0..9 {
        companion.set_frame(row, mighty_companion::pet::ROW_FRAMES[row as usize] - 1, false);
        rows.push(snapshot(&companion.scene()));
    }
    companion.set_atlas(None);
    companion.set_frame(0, 0, false);
    let paw = snapshot(&companion.scene());
    let mut cards = serde_json::Map::new();
    let kinds: Vec<(&str, CompanionCard)> = vec![
        ("waiting", task("waiting", "waiting")),
        (
            "running",
            CompanionCard {
                state: "실행 중".into(),
                work: "Reading crates/mighty-companion/src/lib.rs".into(),
                ..task("running", "running")
            },
        ),
        ("completed", CompanionCard { state: "완료".into(), request: None, ..task("completed", "completed") }),
        ("error", CompanionCard { state: "오류".into(), elapsed: None, ..task("error", "error") }),
        ("idle", CompanionCard { state: "대기".into(), workspace: String::new(), request: None, ..task("idle", "idle") }),
        ("pager", CompanionCard { position: 2, count: 3, pending: true, ..task("pager", "running") }),
        ("approval", with(task("approval", "waiting"), approval_request(true))),
        (
            "approvalBlocked",
            with(
                task("approvalBlocked", "waiting"),
                CompanionRequest { error: Some("보내지 못했습니다".into()), ..approval_request(false) },
            ),
        ),
        ("question", with(task("question", "waiting"), question_request())),
        (
            "plan",
            with(
                task("plan", "waiting"),
                plan_request(
                    "# Ship the pet's plan bubble\n\n1. Draw the plan in the bubble\n2. Route its **buttons**\n- [x] done\n> quoted\n\n```\ncode\n```\n---",
                ),
            ),
        ),
        (
            "planSending",
            with(task("planSending", "waiting"), CompanionRequest { busy: true, buttons: bubble::plan(true), ..plan_request("# Sending") }),
        ),
        ("light", CompanionCard { light: true, ..task("light", "waiting") }),
        ("progress", CompanionCard { state: "실행 중".into(), progress: Some(progress_card()), ..task("progress", "running") }),
        (
            "progressFolded",
            CompanionCard {
                state: "실행 중".into(),
                progress: Some(progress_card()),
                progress_collapsed: true,
                ..task("progressFolded", "running")
            },
        ),
    ];
    for (name, card) in kinds {
        companion.set_card(Some(card), true);
        cards.insert(name.into(), snapshot(&companion.scene()));
    }
    companion.set_card(Some(task("menu", "waiting")), true);
    companion.set_keep_menu(true);
    let mut events = pet.events().unwrap();
    let center = companion.target("toggle").unwrap();
    pet.draw(&companion.scene()).unwrap();
    pet.send(PetInput::RightClick(center)).unwrap();
    pump(&mut companion, &mut events);
    cards.insert("menu".into(), snapshot(&companion.scene()));
    let golden = json!({ "rows": rows, "paw": paw, "cards": cards });
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join("scenes.json");
    let written = serde_json::to_string_pretty(&golden).unwrap() + "\n";
    if std::env::var_os("MIGHTY_COMPANION_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &written).unwrap();
    }
    let checked_in: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("tests/golden/scenes.json (MIGHTY_COMPANION_BLESS=1 writes it)"))
            .unwrap();
    for (name, scene) in golden["cards"].as_object().unwrap() {
        assert_eq!(scene, &checked_in["cards"][name], "the {name} card's scene");
    }
    assert_eq!(golden["rows"], checked_in["rows"], "the rows' scenes");
    assert_eq!(golden["paw"], checked_in["paw"], "the paw's scene");
}

/// The Mac's progress card over the task bubble: the window grows over the bubble by the card (the pet's feet stay), its heading folds
/// it, a line opens the pane, it turns its own spinner while the request runs, and it is never over a request.
#[test]
fn the_progress_card_stands_over_the_task_bubble_and_folds() {
    korean();
    let pet = FakePet::default();
    pet.set_work_area(LogicalRect { x: 0.0, y: 0.0, width: 2560.0, height: 1400.0 });
    let mut companion = open(&pet);
    let mut events = pet.events().unwrap();
    let plain = CompanionCard { state: "실행 중".into(), ..task("progress", "running") };
    companion.set_card(Some(plain.clone()), true);
    let before = companion.geometry();
    let feet = companion.placement().bottom;
    assert_eq!(companion.progress_rect(), None, "no card without a progress");
    let card = progress_card();
    companion.set_card(Some(CompanionCard { progress: Some(card.clone()), ..plain.clone() }), true);
    let after = companion.geometry();
    let area = companion.progress_rect().expect("the card");
    assert_eq!(after.bubble.height, before.bubble.height, "the bubble keeps its height");
    assert_eq!((area.x, area.width), (after.bubble.x, after.bubble.width), "as wide as the bubble");
    assert_eq!(area.bottom() + progress::GAP, after.bubble.y, "over the bubble by its gap");
    assert_eq!(area.height + progress::GAP, card.height(false));
    assert_eq!(after.height, (after.bubble.height + layout::CHROME + card.height(false)).max(layout::BASE_HEIGHT));
    assert_eq!(companion.placement().bottom, feet, "the pet's feet stay where they stood");
    let scene = companion.scene();
    assert!(scene.items.iter().any(|item| matches!(item, PetItem::Group { role, .. } if role == "progress")));
    let spinners = scene.items.iter().filter(|item| matches!(item, PetItem::Arc { .. })).count();
    assert_eq!(spinners, 2, "the bubble's presence and the card's state each turn");
    assert_eq!(click(&mut companion, &pet, &mut events, "progress-fold"), Some(vec![act("progress", "progress-fold")]));
    let opens = companion.target("open").expect("the card's lines");
    assert!(opens.y > area.y + progress::HEADER_HEIGHT && opens.y < area.bottom());
    assert_eq!(click(&mut companion, &pet, &mut events, "open"), Some(vec![act("progress", "open")]));
    // Folded to its heading: shorter, and nothing under it to press.
    companion.set_card(Some(CompanionCard { progress: Some(card.clone()), progress_collapsed: true, ..plain.clone() }), true);
    assert_eq!(companion.progress_rect().map(|a| a.height), Some(progress::HEADER_HEIGHT));
    assert_eq!(companion.target("open"), None);
    assert_eq!(companion.placement().bottom, feet);
    // A clock that ticks redraws the card in place.
    let mut ticked = card.clone();
    ticked.rows[1].time = Some("1분 5초".into());
    companion.set_card(Some(CompanionCard { progress: Some(ticked), ..plain.clone() }), true);
    assert_eq!(companion.progress_rect(), Some(area));
    // A request takes the bubble: no card over it; a bubble put away has none either.
    companion
        .set_card(Some(CompanionCard { progress: Some(card.clone()), ..with(task("progress", "waiting"), approval_request(true)) }), true);
    assert_eq!(companion.progress_rect(), None);
    companion.set_card(Some(CompanionCard { progress: Some(card), ..plain }), false);
    assert_eq!(companion.progress_rect(), None);
}
