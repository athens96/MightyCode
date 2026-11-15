//! A style's run permissions (C# `StyleRunPermissions.cs`): the install command wrapped for Windows PowerShell, the wire names a
//! style may grant, the auto-allow judged at each request against the style as it stands right then, and the run-start binding
//! (the grant list and the plan-mode launch, §1.17). Owner: W9-D.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use mighty_core::locale::Locale;
use mighty_core::model::RunSession;
use mighty_core::permission::ToolPermissionRequest;
use mighty_run::model::StartRunRequest;
use regex::Regex;

use crate::registry::StyleRegistry;
use crate::trust::TrustAccess;

static WIRE_PART: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[A-Za-z0-9_-]{1,64}$").expect("wire part"));

/// `InstallCommand(command, shell)`: bundled manifests use `&&`, which Windows PowerShell 5.1 lacks, so for it the command goes as one
/// literal argument to cmd (`& $env:ComSpec /d /s /c '…'`), shown so in the confirmation.
pub fn install_command(command: &str, shell: &str) -> String {
    let stem = Path::new(shell).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if stem == "powershell" { format!("& $env:ComSpec /d /s /c '{}'", command.replace('\'', "''")) } else { command.to_string() }
}

/// `ValidWireName(name)`: `ToolSearch`, or `mcp__plugin_<server>__<tool>` without a question or a wildcard.
pub fn valid_wire_name(name: &str) -> bool {
    if name == "ToolSearch" {
        return true;
    }
    let parts: Vec<&str> = name.split("__").collect();
    parts.len() == 3
        && parts[0] == "mcp"
        && parts[1].starts_with("plugin_")
        && !parts[1].ends_with('_')
        && WIRE_PART.is_match(parts[1])
        && parts[2] != "AskUserQuestion"
        && !parts[2].starts_with('_')
        && WIRE_PART.is_match(parts[2])
}

/// The pane is a local Claude pane in the Mighty view with a style chosen.
pub fn styled(pane: &RunSession) -> bool {
    pane.kind.as_deref() == Some("claude")
        && pane.provider.as_deref() == Some("claude")
        && pane.agent_view_mode.as_deref() == Some("mighty")
        && pane.mighty_style.is_some()
}

/// `AutoAllows(request, pane, profile, workspace)`: whether the pane's style allows this request by itself — the pane's choice, the
/// trust record and the file's bytes read again now, so a revoked or changed style, or a pane that left it, stops at once (§4.6).
pub fn auto_allows(
    ask: &ToolPermissionRequest,
    pane: Option<&RunSession>,
    profile: &Path,
    workspace: &Path,
    access: Arc<dyn TrustAccess>,
    locale: Locale,
) -> bool {
    if ask.state != "pending" || !ask.can_allow || ask.tool_name == "AskUserQuestion" || !valid_wire_name(&ask.tool_name) {
        return false;
    }
    let Some(pane) = pane.filter(|p| styled(p)) else { return false };
    let registry = StyleRegistry::load(profile, workspace, access, locale);
    registry
        .runnable(pane.mighty_style.as_deref(), pane.mighty_style_hash.as_deref())
        .is_some_and(|style| StyleRegistry::unchanged(style) && style.evaluator().auto_allowed(&ask.tool_name))
}

/// `Bind(request, selected, profile, workspace)`: the grants a run of a styled pane starts with and, for a plan-mode style, plan mode
/// for every new request (the pane's stored mode stays). An unstyled pane's request goes without grants.
pub fn bind(
    mut request: StartRunRequest,
    selected: &RunSession,
    profile: &Path,
    workspace: &Path,
    access: Arc<dyn TrustAccess>,
    locale: Locale,
) -> Result<StartRunRequest, String> {
    if !styled(selected) {
        request.style_auto_allow = None;
        return Ok(request);
    }
    if Some(&request.session_id) != selected.id.as_ref()
        || Some(&request.workspace_id) != selected.workspace_id.as_ref()
        || request.kind != "claude"
        || request.provider != "claude"
    {
        return Err("Style selection belongs to a different pane.".into());
    }
    let registry = StyleRegistry::load(profile, workspace, access, locale);
    let style = registry
        .runnable(selected.mighty_style.as_deref(), selected.mighty_style_hash.as_deref())
        .ok_or_else(|| locale.get("guidedPanel.approvalRequired"))?;
    if !StyleRegistry::unchanged(style) {
        return Err(locale.get("guidedPanel.approvalRequired"));
    }
    let grants = style.manifest.auto_allow();
    if grants.iter().any(|g| !valid_wire_name(g)) {
        return Err("Invalid style tool permission.".into());
    }
    if style.evaluator().launch_permission_mode() == Some("plan") {
        request.permission_mode_override = Some("plan".into());
    }
    request.style_auto_allow = Some(grants);
    Ok(request)
}

/// C# `DesktopService.ReceiveToolPermission`'s style half: a request the pane's style lists as its own is allowed once, without a card;
/// its settling is no news for the pane. Every other request, and one whose allowance did not go through, reaches the pane.
#[derive(Debug, Default)]
pub struct StyleAllowing {
    keys: Mutex<HashSet<String>>,
}

impl StyleAllowing {
    /// Whether the pane is shown `request`: `allows` judges it ([`auto_allows`]), `respond` answers it allowed (false when that failed).
    pub fn receive(&self, request: &ToolPermissionRequest, allows: impl FnOnce() -> bool, respond: impl FnOnce() -> bool) -> bool {
        let key = format!("{}|{}", request.run_id, request.id);
        let lock = || self.keys.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if request.state != "pending" {
            return !lock().remove(&key);
        }
        if allows() {
            lock().insert(key.clone());
            // Answered: its settling (now or later) finds the mark and is not shown. Not answered: nothing will settle it, so it
            // waits for the user like any other.
            if respond() {
                return false;
            }
            lock().remove(&key);
        }
        true
    }

    /// Whether `request` is one the style is answering (or judging): never shown as waiting meanwhile.
    pub fn holds(&self, run_id: &str, id: &str) -> bool {
        self.keys.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).contains(&format!("{run_id}|{id}"))
    }

    /// Marks `request` as the style's to judge or answer (its settling is then no news).
    pub fn hold(&self, run_id: &str, id: &str) {
        self.keys.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(format!("{run_id}|{id}"));
    }

    /// An allowance that did not go through: its mark taken away; true when it was still there (nothing settled it).
    pub fn release(&self, run_id: &str, id: &str) -> bool {
        self.keys.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&format!("{run_id}|{id}"))
    }
}
