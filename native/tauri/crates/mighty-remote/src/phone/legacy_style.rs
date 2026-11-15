//! Older phones' Ouroboros payload and the phone's style choices (C# `MobileLegacyStyleAdapter`, `MobileRemoteSupport.StyleOptions`,
//! `LegacyStyleOptions`; `Core\Remote\MobileMighty.cs:471-478, 521-545`; plan §1.7 "The phone", W9-D). Owner: W9-D.
//!
//! Only the bundled `ouroboros` style is projected into the legacy `ouroboros` object an older phone reads (L477 "Mobile host: older
//! phones still get the Ouroboros payload"); every other style reaches the phone as `panel` and `styleId` only.

use mighty_styles::manifest::{StyleAction, StylePhase, strings, text};
use mighty_styles::registry::RegisteredStyle;
use serde_json::{Value, json};

use super::mighty::{CLI_STYLE, MIGHTY_STYLES, MobileOption, MobileStyleOption};

/// C# `MobileGuidedSkill(Skill, Title, Help)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileGuidedSkill {
    pub skill: String,
    pub title: String,
    pub help: String,
}

/// C# `MobileOuroboros(Phase, Ready, TakesText, Next, All)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileOuroboros {
    pub phase: String,
    pub ready: bool,
    pub takes_text: Vec<String>,
    pub next: Vec<MobileGuidedSkill>,
    pub all: Vec<MobileGuidedSkill>,
}

impl MobileOuroboros {
    /// As `Wire.Json` writes it.
    pub fn to_value(&self) -> Value {
        let skills = |list: &[MobileGuidedSkill]| -> Vec<Value> {
            list.iter().map(|s| json!({ "skill": s.skill, "title": s.title, "help": s.help })).collect()
        };
        json!({ "phase": self.phase, "ready": self.ready, "takesText": self.takes_text, "next": skills(&self.next), "all": skills(&self.all) })
    }
}

fn skill(a: &StyleAction) -> MobileGuidedSkill {
    MobileGuidedSkill { skill: a.id.clone(), title: a.title.clone(), help: a.help.clone() }
}

/// `MobileLegacyStyleAdapter.Payload(style, phase, ready)`: the legacy object for the bundled Ouroboros style, none for any other.
pub fn payload(style: &RegisteredStyle, phase: Option<&StylePhase>, ready: bool) -> Option<MobileOuroboros> {
    (style.id() == "ouroboros").then(|| ouroboros(style, phase, ready))
}

/// `MobileLegacyStyleAdapter.Ouroboros`: the phase (the first by order when none), the phase map's next skills, the whole catalogue.
pub fn ouroboros(style: &RegisteredStyle, phase: Option<&StylePhase>, ready: bool) -> MobileOuroboros {
    let manifest = &style.manifest;
    let next = manifest.rule("next");
    let ids = match phase {
        Some(p) if text(next, "kind") == Some("byPhase") => next.get("map").map(|m| strings(m, &p.id)).unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut ordered: Vec<&StylePhase> = manifest.phases.iter().collect();
    ordered.sort_by_key(|p| p.order);
    MobileOuroboros {
        phase: phase.map(|p| p.id.clone()).or_else(|| ordered.first().map(|p| p.id.clone())).unwrap_or_default(),
        ready,
        takes_text: manifest.actions.iter().filter(|a| a.takes_text).map(|a| a.id.clone()).collect(),
        next: ids.iter().filter_map(|id| manifest.action(id)).map(skill).collect(),
        all: manifest.actions.iter().map(skill).collect(),
    }
}

/// `MobileRemoteSupport.StyleOptions(styles)`: the CLI, then every style that can run, each with its source.
pub fn style_options(styles: &[RegisteredStyle]) -> Vec<MobileStyleOption> {
    std::iter::once(MobileStyleOption { id: CLI_STYLE.into(), label: CLI_STYLE.into(), source: None })
        .chain(styles.iter().filter(|s| s.runnable()).map(|s| MobileStyleOption {
            id: s.id().into(),
            label: s.manifest.name().into(),
            source: Some(s.source.clone()),
        }))
        .collect()
}

/// `MobileRemoteSupport.LegacyStyleOptions(styles)`: the old field's fixed two (CLI, Ouroboros), Ouroboros only when it can run.
pub fn legacy_style_options(styles: &[RegisteredStyle]) -> Vec<MobileOption> {
    let runnable: Vec<&RegisteredStyle> = styles.iter().filter(|s| s.runnable()).collect();
    std::iter::once(MobileOption::new(CLI_STYLE, "CLI"))
        .chain(
            MIGHTY_STYLES
                .iter()
                .skip(1)
                .filter_map(|id| runnable.iter().find(|s| s.id() == *id))
                .map(|s| MobileOption::new(s.id(), s.manifest.name())),
        )
        .collect()
}
