//! The Mac's checks on what crosses the remote wire (C# `RemoteWorkspaceValidation`, R\RemoteWorkspaceValidation.cs as the fix branch
//! has it: `AntigravityCard` judges a host's rules by `TextVerdict`, by their text alone; M/Remote/RemoteWorkspaceModels.swift
//! `RemoteWorkspaceValidation`; plan §1.6, D1). Owner: W8-D.

use mighty_core::model::PhaseModelsSnapshot;
use mighty_core::normalize::remote_path;
use mighty_core::wire;
use mighty_run::antigravity::permissions::{self as antigravity, MAXIMUM_ACTIONS, MAXIMUM_RULE_BYTES, PermissionRequest, RuleVerdict};
use mighty_run::model::{ModelCatalog, ProviderCapabilities, RuntimeInfo, StartRunRequest};
use mighty_run::providers;
use mighty_run::strings::get;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::remote::wire::{RemoteSharedWorkspace, RemoteWireInfo, protocol};

/// `EventTypes`: the run event types the Mac reads.
pub const EVENT_TYPES: [&str; 11] =
    ["log", "status", "resume", "activity", "usage", "graph", "permission", "plan", "todos", "background", "antigravityPermission"];

/// `Key(value)`: a connection key, 256 random bits in base64url (43 characters) or longer, at most 128.
pub fn key(value: Option<&str>) -> bool {
    value.is_some_and(|v| (43..=128).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
}

/// `KeysMatch(presented, expected)`: both sides hashed first, then compared in fixed time, so neither the length nor the first differing
/// byte shows in the time taken.
pub fn keys_match(presented: &str, expected: &str) -> bool {
    let (a, b) = (Sha256::digest(presented.as_bytes()), Sha256::digest(expected.as_bytes()));
    bool::from(a.as_slice().ct_eq(b.as_slice()))
}

fn utf16(value: &str) -> usize {
    value.encode_utf16().count()
}

/// `SharedWorkspace(value)`: an id, a name of 1-120 characters without a control character, a host's absolute path.
pub fn shared_workspace(value: &RemoteSharedWorkspace) -> bool {
    wire::identifier(&value.id)
        && (1..=120).contains(&utf16(&value.name))
        && !value.name.chars().any(char::is_control)
        && remote_path(Some(&value.path))
}

/// `PermissionMode(mode, provider)`: the modes every agent has, `auto` only for Claude, `onRequest` only for Codex.
pub fn permission_mode(mode: &str, provider: &str) -> bool {
    matches!(mode, "manual" | "plan" | "acceptEdits" | "fullAccess")
        || mode == "auto" && provider == "claude"
        || mode == "onRequest" && provider == "codex"
}

/// `AntigravityCard(card)`: ids, 1-16 actions, bounded text, and only well-formed rules `TextVerdict` does not refuse (by their text:
/// nothing on this computer's disk is asked for a host's path).
pub fn antigravity_card(card: &PermissionRequest) -> bool {
    wire::identifier(&card.id)
        && wire::identifier(&card.run_id)
        && (1..=MAXIMUM_ACTIONS).contains(&card.actions.len())
        && card.actions.iter().all(|action| {
            !action.action.is_empty()
                && action.action.len() <= 128
                && !action.display_name.is_empty()
                && action.display_name.len() <= 128
                && action.target.as_deref().unwrap_or_default().len() <= MAXIMUM_RULE_BYTES
                && action
                    .rule
                    .as_deref()
                    .is_none_or(|rule| antigravity::valid_rule(rule) && antigravity::text_verdict(rule) != RuleVerdict::Refused)
        })
}

/// What [`info`] refuses: the message is what the user reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidInfo(pub String);

impl std::fmt::Display for InvalidInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidInfo {}

/// `Info(value)`: a host's /info as the Mac checks it (`remote.connect.error.notMighty` for the host, `.badRuntime` for its CLIs).
pub fn info(value: &RemoteWireInfo) -> Result<(), InvalidInfo> {
    let mut ids: Vec<&str> = value.workspaces.iter().map(|w| w.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    let runtime = &value.runtime;
    if value.protocol != protocol::VERSION
        || !wire::identifier(&value.host_id)
        || !(1..=120).contains(&utf16(&value.host_name))
        || value.workspaces.len() > protocol::MAXIMUM_SHARED_WORKSPACES
        || !value.workspaces.iter().all(shared_workspace)
        || ids.len() != value.workspaces.len()
        || !matches!(runtime.platform.as_str(), "darwin" | "win32" | "linux")
        || utf16(&runtime.app_version) > 80
        || runtime.providers.len() > wire::PROVIDERS.len()
    {
        return Err(InvalidInfo(get("remote.connect.error.notMighty")));
    }
    for provider in &runtime.providers {
        let catalog = &provider.model_catalog;
        if !wire::PROVIDERS.contains(&provider.id.as_str())
            || utf16(&provider.name) > 120
            || utf16(&provider.detail) > 2_000
            || provider.version.as_deref().map_or(0, utf16) > 80
            // A longer list is cut before this (`bounded`), never refused: one long list would drop the whole host.
            || catalog.models.len() > maximum_models(&provider.id)
            || utf16(&catalog.detail) > 2_000
            // A newer host says why its latest read failed and when a kept list was read; an older one says neither.
            || catalog.read_failure.as_deref().map_or(0, utf16) > 2_000
            || catalog.listed_at.as_deref().map_or(0, utf16) > 64
            || !catalog.models.iter().all(|m| wire::model(&m.value) && utf16(&m.display_name) <= 160 && utf16(&m.description) <= 2_400)
            || !provider.capabilities.permission_modes.iter().all(|mode| permission_mode(mode, &provider.id))
        {
            return Err(InvalidInfo(get("remote.connect.error.badRuntime")));
        }
    }
    Ok(())
}

/// `MaximumModels(provider)`: its own default and as many of the CLI's as this app's own read of that CLI keeps.
pub fn maximum_models(provider: &str) -> usize {
    1 + providers::listed_model_limit(provider)
}

fn cut(catalog: &ModelCatalog, provider: &str) -> ModelCatalog {
    let mut cut = catalog.clone();
    cut.models.truncate(maximum_models(provider));
    cut
}

/// `Bounded(runtime)`: each provider's model list cut to its first [`maximum_models`]; a client keeps an older host's longer list to
/// that many instead of refusing its whole /info.
pub fn bounded(runtime: &RuntimeInfo) -> RuntimeInfo {
    let mut bounded = runtime.clone();
    // The older field, Claude's list again.
    bounded.model_catalog = runtime.model_catalog.as_ref().map(|c| cut(c, "claude"));
    for provider in &mut bounded.providers {
        provider.model_catalog = cut(&provider.model_catalog, &provider.id);
    }
    bounded
}

/// `PhaseModels(value)`: Settings › Models as the client sent it: every value a model name or "default", every effort a known one.
pub fn phase_models(value: &PhaseModelsSnapshot) -> bool {
    let model = |m: &Option<String>| {
        let m = m.as_deref().unwrap_or("default");
        m == "default" || wire::model(m)
    };
    let effort = |e: &Option<String>| {
        let e = e.as_deref().unwrap_or("default");
        e == "default" || wire::EFFORTS.contains(&e)
    };
    [
        &value.claude_main,
        &value.claude_opus_alias,
        &value.claude_sonnet_alias,
        &value.claude_haiku_alias,
        &value.claude_subagent_default,
        &value.codex_review_model,
        &value.codex_subagent_default,
    ]
    .into_iter()
    .all(model)
        && [&value.codex_plan_mode_reasoning_effort, &value.claude_main_effort, &value.codex_main_effort, &value.codex_subagent_effort]
            .into_iter()
            .all(effort)
}

/// `CapabilityProblem(request, caps)`: what the host's CLI cannot run (M/CoreValidation.validateCapabilities); None when it can.
pub fn capability_problem(request: &StartRunRequest, caps: &ProviderCapabilities) -> Option<String> {
    let s = request.settings();
    if !caps.attachments && request.attachment_list().is_some() {
        return Some(get("validation.run.attachmentsUnsupported"));
    }
    let modes = &caps.permission_modes;
    let supported = modes.contains(&s.permission_mode)
        && request.permission_mode_override.as_ref().is_none_or(|m| modes.contains(m))
        && (caps.effort || s.effort == "default")
        && (caps.max_turns || s.max_turns.is_none())
        && (caps.max_budget_usd || s.max_budget_usd.is_none())
        && (caps.fast_mode || !s.fast_mode)
        && (caps.web_search || s.web_search == "default")
        && (caps.network_access || !s.network_access)
        && (!s.network_access || request.provider == "codex" && matches!(s.permission_mode.as_str(), "acceptEdits" | "onRequest"))
        && (caps.resume || request.resume_id.is_none());
    (!supported).then(|| get("validation.run.settingsUnsupported"))
}
