//! C# `StyleSurfacesVerification.cs`, ported by name (L526–L528): the approval card and the reset, metadata and feature state, and a
//! trusted request's projection with a locked registry.

mod common;

use std::collections::BTreeMap;

use common::*;
use mighty_styles::evaluator::StyleStateWidget;
use mighty_styles::presentation::{self, StyleApprovalDecision};
use mighty_styles::{RegisteredStyle, decoder};
use serde_json::Value;

/// L526 "StyleSurfacesVerification ApprovalAndReset".
#[test]
fn style_surfaces_verification_approval_and_reset() {
    let style = bundled("ouroboros");
    let sections = presentation::approval(&korean(), &style);
    let automatic = sections.iter().find(|s| s.id == "autoAllow").expect("autoAllow");
    assert!(
        !automatic.foldable
            && automatic.lines.len() == 17
            && automatic.lines.iter().any(|l| l == "mcp__plugin_ouroboros_ouroboros__ouroboros_interview"),
        "Approval must expose every exact automatic permission outside collapsed details."
    );
    assert_eq!(sections.iter().take(4).map(|s| s.id.as_str()).collect::<Vec<_>>(), ["origin", "autoAllow", "install", "enter"]);
    assert!(sections.iter().find(|s| s.id == "enter").expect("enter").lines.iter().any(|l| l == "/ouroboros:interview {text}"));
    let mut decision = StyleApprovalDecision::new(automatic.lines.len());
    assert!(!decision.press(false) && !decision.confirming, "An unshown permission block cannot start approval.");
    assert!(!decision.press(true) && decision.confirming, "Automatic permission approval needs a second explicit press.");
    assert!(!decision.press(false) && decision.press(true), "The confirming press still requires a visible permission block.");
    assert!(StyleApprovalDecision::new(0).press(true), "A style without automatic permissions needs one explicit approval.");
    assert!(!StyleApprovalDecision::new(1).press(true), "Confirmation cannot carry into a fresh preview.");
    let evaluator = style.evaluator();
    let completed = evaluator.current_phase(&["/ouroboros:evaluate"], None, None);
    assert!(completed.map(|p| p.id.as_str()) == Some("evaluate") && !evaluator.at_start(completed), "Fixture reached a later phase.");
    let reset = evaluator.effective_phase(completed, true);
    assert!(
        evaluator.at_start(reset)
            && evaluator.visible_actions(reset, None, false, false, true).iter().map(|a| a.id.as_str()).eq(["interview", "auto"]),
        "Reset exposes the entry phase and start actions."
    );
    assert_eq!(evaluator.rewrite_action("a new goal", reset, false, false, true, true), Some("interview"));
    assert!(
        evaluator.rewrite_action("/help", reset, false, false, true, true).is_none()
            && evaluator.rewrite_action("a goal", reset, true, false, true, true).is_none()
    );
    assert_eq!(evaluator.effective_phase(completed, false), completed, "Cancel reset restores the current phase.");
}

/// L527 "StyleSurfacesVerification MetadataAndFeatureState".
#[test]
fn style_surfaces_verification_metadata_and_feature_state() {
    let catalogue = fixture_style("deck");
    let peek = catalogue.manifest.action("peek").expect("peek");
    let probe = catalogue.manifest.action("probe").expect("probe");
    assert!(
        peek.scope.as_deref() == Some("지금 상태")
            && probe.scope.as_deref() == Some("질문 하나")
            && probe.flags == ["userInvoked"]
            && probe.glyph.as_deref() == Some("🔍"),
        "Scope, glyph and each action flag survive manifest decoding."
    );
    assert_eq!(peek.flags, ["userInvoked", "readOnly"]);
    let state = BTreeMap::from([("acme.thing".to_string(), "complete".to_string())]);
    assert!(catalogue.evaluator().recommended_action(&state) == Some("reset") && catalogue.evaluator().draws_group_map());
    assert_eq!(presentation::progress(&StyleStateWidget::bar("progressBar", 7, None)), Some((0.0, "7".to_string())));
    assert_eq!(presentation::progress(&StyleStateWidget::bar("progressBar", 7, Some(4))), Some((1.0, "4/4".to_string())));
    assert!(!presentation::inline("a\u{202e}b\u{0}c").contains('\u{202e}'), "Inline widget text removes display-direction controls.");
}

/// L528 "StyleSurfacesVerification TrustedRequestProjectionAndLockedRegistry".
#[test]
fn style_surfaces_verification_trusted_request_projection_and_locked_registry() {
    let root = Temp::new();
    let registry = load(&root.0, &root.0);
    let bundled = registry.styles.iter().find(|s| s.id() == "ouroboros").expect("ouroboros").clone();
    let mut json: Value = serde_json::from_slice(&bundled.bytes).expect("json");
    json["id"] = "custom-flow".into();
    json["name"] = "Custom".into();
    json["autoAllow"] = serde_json::json!([]);
    json["actions"][0]["requestTitle"] = "Custom interview".into();
    let bytes = serde_json::to_vec(&json).expect("bytes");
    let modified =
        RegisteredStyle::decoded(decoder::decode(&bytes, "user", None).expect("decode"), "user", &bundled.path, None, "approved", &bytes);
    assert_eq!(
        presentation::request_prefix(std::slice::from_ref(&modified), "/ouroboros:interview goal").as_deref(),
        Some("Custom interview")
    );
    let pending = RegisteredStyle { approval: "pending".into(), ..modified.clone() };
    assert_eq!(
        presentation::request_prefix(&[pending], "/ouroboros:interview goal"),
        None,
        "An unapproved style cannot influence a request's identity."
    );
    let interview = bundled.manifest.phase("interview").expect("interview").title.clone();
    assert_eq!(
        presentation::request_prefix(&[modified, bundled.clone()], "/ouroboros:interview goal"),
        Some(interview),
        "Bundled-first recognition."
    );
    let evaluate = bundled.manifest.phase("evaluate").expect("evaluate").title.clone();
    assert_eq!(
        presentation::request_prefix(std::slice::from_ref(&bundled), "ooo qa"),
        Some(evaluate),
        "Aliases project their phase titles."
    );
    std::fs::create_dir_all(root.join("styles")).expect("styles");
    std::fs::write(root.join("styles").join("custom.json"), &bytes).expect("custom");
    std::fs::create_dir_all(root.join("style-trust")).expect("trust");
    std::fs::write(root.join("style-trust").join("approvals.json"), "{\"version\":1,\"records\":null}").expect("approvals");
    let locked = load(&root.0, &root.0);
    assert!(
        locked.trust_locked && locked.rejections.iter().any(|r| r.code == "E_TRUST_LOCKED"),
        "Malformed approval storage is visible as a locked trust store."
    );
    assert!(
        locked.styles.iter().find(|s| s.id() == "custom-flow").expect("custom").approval == "pending"
            && locked.styles.iter().filter(|s| s.source == "bundled").all(|s| s.runnable()),
        "Locked storage keeps custom previews visible without granting permission or disabling bundled styles."
    );
}
