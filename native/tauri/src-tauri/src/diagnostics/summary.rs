//! MC-54: the settings summary of a diagnostics bundle: which features are on and how many workspaces and panes of each kind there
//! are, never a name, a path, an address, a key or what anyone wrote.

use std::collections::BTreeMap;

use mighty_core::model::AppSnapshot;
use serde_json::{Value, json};

/// The summary of `snapshot` (`settings-summary.json`).
pub fn settings_summary(snapshot: &AppSnapshot) -> Value {
    let workspaces = snapshot.workspaces();
    let count = |test: &dyn Fn(&mighty_core::model::Workspace) -> bool| workspaces.iter().filter(|w| test(w)).count();
    let mut providers: BTreeMap<String, usize> = BTreeMap::new();
    let mut statuses: BTreeMap<String, usize> = BTreeMap::new();
    for pane in snapshot.sessions() {
        *providers.entry(pane.provider().to_string()).or_default() += 1;
        *statuses.entry(pane.status().to_string()).or_default() += 1;
    }
    let phone = snapshot.mobile_remote.clone().unwrap_or_default();
    json!({
        "workspaces": {
            "count": workspaces.len(),
            "remote": count(&|w| w.remote.is_some()),
            "ssh": count(&|w| w.ssh.is_some()),
            "sandboxed": count(&|w| w.sandbox.as_ref().is_some_and(|s| s.enabled)),
            "paneWorktreeMode": count(&|w| w.pane_worktree_mode.as_deref().is_some_and(|m| !m.is_empty() && m != "off")),
            "issueTrackers": count(&|w| w.issue_trackers.as_ref().is_some_and(|t| !t.is_empty())),
            "mightywork": count(&|w| w.mightywork_project.as_deref().is_some_and(|p| !p.is_empty())),
        },
        "panes": {
            "count": snapshot.sessions().len(),
            "byProvider": providers,
            "byStatus": statuses,
        },
        "features": {
            "statusLine": snapshot.status_line_enabled,
            "completionNotifications": snapshot.completion_notifications_enabled,
            "autoUpdateClis": snapshot.auto_update_clis,
            "autoUpdatePlugins": snapshot.auto_update_plugins,
            "autoLoginClis": snapshot.auto_login_clis,
            "agentChecklist": snapshot.agent_checklist_on(),
            "crossWorkspaceLookup": snapshot.cross_workspace_lookup_on(),
            "claudeDirectUsageLookup": snapshot.claude_direct_usage_lookup_enabled,
            "appUpdateAutoCheck": snapshot.app_update_auto_check,
            "customUpdateAddress": snapshot.app_update_manifest_url_override.as_deref().is_some_and(|u| !u.is_empty()),
            "phaseModels": snapshot.phase_models.is_some(),
            "modelDefaults": snapshot.model_defaults.is_some(),
        },
        "phone": {
            "enabled": phone.enabled,
            "direct": phone.direct_enabled,
            "legacyPhones": phone.allow_legacy_phones,
            "sharedOnly": phone.phone_shared_only,
            "customRelay": phone.relay_url.as_deref().is_some_and(|u| !u.is_empty()),
        },
        "display": {
            "language": snapshot.language_preference,
            "theme": snapshot.theme,
            "layout": snapshot.layout,
            "sidebarCollapsed": snapshot.sidebar_collapsed,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_core::json::Wire as _;

    #[test]
    fn the_summary_counts_and_switches_but_names_nothing() {
        let text = r#"{"version":1,"workspaces":[
            {"id":"w1","name":"Secret Project","path":"/Users/alice/secret","createdAt":"2026-01-01T00:00:00Z","sandbox":{"enabled":true}},
            {"id":"w2","name":"Box","path":"/srv","createdAt":"2026-01-01T00:00:00Z","ssh":{"machine":"m","dir":"/srv","host":"box.example.com"}}],
          "sessions":[{"id":"s1","workspaceId":"w1","title":"Fix the login","kind":"claude","status":"idle","model":"default","provider":"claude",
            "settings":{"effort":"default","permissionMode":"manual","maxTurns":null,"maxBudgetUsd":null},"logs":[],"createdAt":"2026-01-01T00:00:00Z","draft":"my draft"}],
          "mobileRemote":{"enabled":true,"relayUrl":"wss://relay.example.com","directEnabled":false,"directPort":43139},
          "appUpdateManifestUrlOverride":"https://example.com/manifest.json"}"#;
        let snapshot = AppSnapshot::read(&serde_json::from_str(text).unwrap()).unwrap();
        let summary = settings_summary(&snapshot);
        assert_eq!(summary["workspaces"]["count"], 2);
        assert_eq!(summary["workspaces"]["ssh"], 1);
        assert_eq!(summary["workspaces"]["sandboxed"], 1);
        assert_eq!(summary["panes"]["count"], 1);
        assert_eq!(summary["panes"]["byProvider"]["claude"], 1);
        assert_eq!(summary["phone"]["enabled"], true);
        assert_eq!(summary["phone"]["customRelay"], true);
        assert_eq!(summary["features"]["customUpdateAddress"], true);
        let written = summary.to_string();
        for private in
            ["Secret Project", "/Users/alice", "box.example.com", "Fix the login", "my draft", "relay.example.com", "manifest.json"]
        {
            assert!(!written.contains(private), "{private} must not be in the summary: {written}");
        }
    }
}
