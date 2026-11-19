//! C# `PhaseModelPreferences` (PhaseModelPreferences.cs): Settings › 페이즈별 모델's per-provider settings, shared by the settings
//! screen (W6's NativeNode form draws what [`crate::phase_section`] gives) and every run entry point — the main model and effort a run
//! reads, a provider row or effort the screen sets, the versions and efforts its pickers offer, and a registered model name.

use mighty_core::model::{PhaseModelsSnapshot, RegisteredModelEntry};
use mighty_core::{model_label, normalize, wire};

use crate::model::{ModelCatalog, StartRunRequest};
use crate::phase_routing::{self, Phase};
use crate::phase_section::{self, PhaseModelEdit, PhaseModelTools};
use crate::{Result, RunError, providers, strings};

/// C# `new PhaseModelsSnapshot()`: every model knob "default", the plan mode effort "default", no effort chosen.
pub fn new_snapshot() -> PhaseModelsSnapshot {
    let default = || Some("default".to_string());
    PhaseModelsSnapshot {
        claude_main: default(),
        claude_opus_alias: default(),
        claude_sonnet_alias: default(),
        claude_haiku_alias: default(),
        claude_subagent_default: default(),
        codex_review_model: default(),
        codex_subagent_default: default(),
        codex_plan_mode_reasoning_effort: default(),
        claude_main_effort: None,
        codex_main_effort: None,
        codex_subagent_effort: None,
    }
}

/// C# `PhaseModelVersion(Value, Label)`: a row of a phase's model picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseModelVersion {
    pub value: String,
    pub label: String,
}

/// `PhaseModelPreferences.Normalize`.
pub fn normalized(value: Option<&PhaseModelsSnapshot>) -> Option<PhaseModelsSnapshot> {
    normalize::phase_models(value)
}

/// Claude Code's own model aliases: what `--model` takes as it is, and what each `ANTHROPIC_DEFAULT_<ALIAS>_MODEL` names.
pub const CLAUDE_ALIASES: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

/// A Claude alias (`sonnet`, `opus[1m]`, …) with its context suffix, or None for a model id.
pub fn claude_alias(value: &str) -> Option<&str> {
    let bare = value.split('[').next().unwrap_or(value);
    CLAUDE_ALIASES.iter().copied().find(|alias| *alias == bare)
}

/// The knobs Claude Code reads as environment variables (`ANTHROPIC_DEFAULT_OPUS_MODEL`, …) name a model, not an alias: an alias
/// there is sent to the API as it is and refused ("sonnet" is no model), and the sonnet knob naming `sonnet` sends `--model
/// sonnet` round in a circle. So an alias picked for one of them is saved as the model the CLI says it stands for now
/// (`ModelOption.resolved_model`, e.g. `claude-sonnet-5-5`); the main model keeps the alias, which `--model` takes as it is.
pub fn concrete_claude_models(config: &PhaseModelsSnapshot, catalog: &ModelCatalog) -> PhaseModelsSnapshot {
    let concrete = |value: &Option<String>| -> Option<String> {
        let value = value.as_deref()?;
        if claude_alias(value).is_none() {
            return Some(value.to_string());
        }
        let resolved = catalog
            .models
            .iter()
            .find(|m| m.value == value)
            .and_then(|m| m.resolved_model.clone())
            .filter(|id| !id.is_empty() && claude_alias(id).is_none());
        Some(resolved.unwrap_or_else(|| value.to_string()))
    };
    PhaseModelsSnapshot {
        claude_opus_alias: concrete(&config.claude_opus_alias),
        claude_sonnet_alias: concrete(&config.claude_sonnet_alias),
        claude_haiku_alias: concrete(&config.claude_haiku_alias),
        ..config.clone()
    }
}

/// `PhaseModelPreferences.EffectiveModel`: a Claude pane on its default model runs Settings › Models' main model.
pub fn effective_model(request: &StartRunRequest) -> String {
    if request.provider == "claude" && request.model == "default" {
        return normalized(request.phase_models.as_ref()).and_then(|p| p.claude_main).unwrap_or_else(|| "default".into());
    }
    request.model.clone()
}

/// `PhaseModelPreferences.MainEffort`.
pub fn main_effort(request: &StartRunRequest) -> Option<String> {
    let phase = normalized(request.phase_models.as_ref())?;
    match request.provider.as_str() {
        "claude" => phase.claude_main_effort,
        "codex" => phase.codex_main_effort,
        _ => None,
    }
}

/// `PhaseModelPreferences.ResolveEffort`: a pane left on Auto takes the main effort when its model takes that level.
pub fn resolve_effort(request: &StartRunRequest, catalog: &ModelCatalog) -> StartRunRequest {
    let settings = request.settings();
    if settings.effort != "default" {
        return request.clone();
    }
    let Some(effort) = main_effort(request) else { return request.clone() };
    let levels = providers::efforts(&request.provider, &effective_model(request), catalog, Some(&request.registered_models));
    if levels.contains(&effort) {
        let mut resolved = request.clone();
        resolved.settings = Some(mighty_core::model::RunSettings { effort, ..settings });
        return resolved;
    }
    request.clone()
}

/// `PhaseModelPreferences.ApplyRow`: a provider's phase row set to `value` (only that provider's knobs, and the installed tools'
/// knobs of the phase); refused for a value that is no model name, another provider, or a phase that uses the pane's model.
pub fn apply_row(
    provider: &str,
    phase: Phase,
    value: &str,
    config: &PhaseModelsSnapshot,
    tools: &PhaseModelTools,
) -> Result<PhaseModelEdit> {
    if !wire::model(value) || !matches!(provider, "claude" | "codex") {
        return Err(RunError::Invalid("Invalid phase model.".into()));
    }
    let supported =
        if provider == "claude" { phase_routing::claude_row_state(phase, config) } else { phase_routing::codex_row_state(phase, config) };
    if supported.is_none() {
        return Err(RunError::Invalid("This phase uses the pane model.".into()));
    }
    Ok(PhaseModelEdit {
        config: if provider == "claude" {
            phase_routing::apply_claude_row(phase, value, config)
        } else {
            phase_routing::apply_codex_row(phase, value, config)
        },
        omc_agents: tools.omc_agents.as_ref().map(|agents| phase_routing::apply_omc_row(phase, value, agents)),
        ouroboros_keys: tools.ouroboros_keys.as_ref().map(|keys| phase_routing::apply_ouroboros_row(phase, value, keys)),
    })
}

/// `PhaseModelPreferences.SetEffort`: the effort of a provider's phase ("default" for the CLI's own); refused for a level that is not
/// one or a phase with no effort setting.
pub fn set_effort(config: &PhaseModelsSnapshot, provider: &str, phase: Phase, value: &str) -> Result<PhaseModelsSnapshot> {
    if value != "default" && !wire::EFFORTS.contains(&value) {
        return Err(RunError::Invalid("Invalid phase effort.".into()));
    }
    let effort = (value != "default").then(|| value.to_string());
    let mut next = config.clone();
    match (provider, phase) {
        ("claude", Phase::Execution) => next.claude_main_effort = effort,
        ("codex", Phase::Execution) => next.codex_main_effort = effort,
        ("codex", Phase::Planning) => next.codex_plan_mode_reasoning_effort = Some(value.to_string()),
        ("codex", Phase::Subagents) => next.codex_subagent_effort = effort,
        _ => return Err(RunError::Invalid("This phase has no effort setting.".into())),
    }
    Ok(next)
}

/// `PhaseModelPreferences.Versions`: a phase picker's rows — each catalogue alias (labelled as the latest when it stands for another
/// id) and the fixed version it stands for, the value saved now when no row has it, then the registered names; each value once.
pub fn versions(catalog: &ModelCatalog, registered: &[RegisteredModelEntry], current: &str) -> Vec<PhaseModelVersion> {
    let name = |entry: &RegisteredModelEntry| entry.name.clone().unwrap_or_default();
    let mut seen: std::collections::HashSet<String> = registered.iter().map(name).collect();
    seen.insert("default".into());
    seen.insert(phase_section::MIXED_SENTINEL.into());
    let mut values = Vec::new();
    for option in &catalog.models {
        let resolved = option.resolved_model.as_deref().filter(|id| !id.is_empty() && *id != option.value);
        if seen.insert(option.value.clone()) {
            let label = model_label::option(&option.label_option(), None);
            let label = match resolved {
                None => label,
                Some(_) => strings::get_with("settings.phaseModels.latestTemplate", &[("name", &label)]),
            };
            values.push(PhaseModelVersion { value: option.value.clone(), label });
        }
        if let Some(resolved) = resolved
            && seen.insert(resolved.to_string())
        {
            values.push(PhaseModelVersion { value: resolved.to_string(), label: model_label::text(resolved, None, None) });
        }
    }
    if seen.insert(current.to_string()) {
        values.push(PhaseModelVersion { value: current.to_string(), label: model_label::text(current, None, None) });
    }
    let mut listed = std::collections::HashSet::new();
    for entry in registered {
        let entry_name = name(entry);
        if listed.insert(entry_name.clone()) {
            values.push(PhaseModelVersion { label: model_label::text(&entry_name, None, None), value: entry_name });
        }
    }
    values
}

/// `PhaseModelPreferences.Efforts`: an effort picker's levels — Codex's the levels its models name (low, medium and high while it
/// names none), every other provider's all of them; the value saved now added when none has it.
pub fn efforts(provider: &str, catalog: &ModelCatalog, current: &str) -> Vec<String> {
    let advertised: std::collections::HashSet<&str> =
        catalog.models.iter().flat_map(|m| m.supported_effort_levels.iter().flatten().map(String::as_str)).collect();
    let mut values: Vec<String> = if provider == "codex" {
        if advertised.is_empty() {
            ["low", "medium", "high"].iter().map(|s| s.to_string()).collect()
        } else {
            wire::EFFORTS.iter().filter(|e| advertised.contains(*e)).map(|s| s.to_string()).collect()
        }
    } else {
        wire::EFFORTS.iter().map(|s| s.to_string()).collect()
    };
    if current != "default" && !values.iter().any(|v| v == current) {
        values.push(current.to_string());
    }
    values
}

/// `PhaseModelPreferences.Registration`: a name registered for a provider's pickers (trimmed, its levels in the catalogue's order),
/// or the reason it is refused as the settings screen says it.
pub fn registration(name: &str, existing: &[RegisteredModelEntry], supports_effort: bool, levels: &[&str]) -> Result<RegisteredModelEntry> {
    let trimmed = name.trim();
    let mut effort: Vec<&str> = Vec::new();
    for level in levels {
        if !effort.contains(level) {
            effort.push(level);
        }
    }
    let key = if trimmed.is_empty() {
        Some("settings.modelDefaults.error.empty")
    } else if trimmed == "default" {
        Some("settings.modelDefaults.error.reserved")
    } else if !wire::model(trimmed) {
        Some("settings.modelDefaults.error.invalid")
    } else if existing.iter().any(|e| e.name.as_deref() == Some(trimmed)) {
        Some("settings.modelDefaults.error.duplicate")
    } else if supports_effort && effort.is_empty() {
        Some("settings.modelDefaults.error.effortWithoutLevels")
    } else if effort.iter().any(|e| !wire::EFFORTS.contains(e)) {
        Some("settings.modelDefaults.error.unknownLevel")
    } else {
        None
    };
    if let Some(key) = key {
        return Err(RunError::Invalid(strings::get(key)));
    }
    let levels: Vec<Option<String>> =
        if supports_effort { wire::EFFORTS.iter().filter(|e| effort.contains(e)).map(|e| Some(e.to_string())).collect() } else { vec![] };
    Ok(RegisteredModelEntry { name: Some(trimmed.to_string()), supports_effort, supported_effort_levels: Some(levels) })
}
