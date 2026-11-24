//! The saved state, `workspace-state.json` version 1: C# `AppSnapshot`, `Workspace`, `RunSession`, `RunSettings`, `LogEntry` and the
//! records they hold (MightyCode.Core/Models.cs, AgentActivity.cs, AgentImages.cs, AgentRunTiming.cs, SessionUsage.cs,
//! ClaudePlanMode.cs, ShellBlockRuns.cs, PaneLayout.cs, Remote/MobileRelayHost.cs), read and written exactly as `Wire.Json` reads and
//! writes them (`json.rs`). The graph records are in `graph.rs`.
//!
//! Each field is the C# property, in the C# declaration order (the order `Wire.Json` writes). A C# reference-typed property is an
//! `Option` (None is C# null, left out when written); an absent property takes the C# initializer's value when read. C# keeps no
//! field it does not model (no `JsonExtensionData` anywhere in Core), so neither does this: a save drops unknown fields as C# does.
//! The converters C# puts on a field (lenient ones load damage as nothing) are kept field by field.

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::Value;

use crate::graph::{GraphBlockSize, MightyGraphRun};
use crate::json::{self, JsonOut, Obj, R, Wire, fail};
use crate::time::{self, DateTimeOffset};
use crate::wire::new_id;

/// A `Dictionary<string, string>` as read: each key once, a null value kept.
pub type StringMap = IndexMap<String, Option<String>>;

fn some(text: &str) -> impl FnOnce() -> Option<String> + '_ {
    move || Some(text.to_string())
}

// ── AppSnapshot ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Two C# properties are left out on purpose, 0.5 having no in-app browser: `agentWebOpenChoices` (where an agent's link opened)
/// and `browserEngineEnabled`. An older version's file holding them loads as with any unknown field, and a save drops them.
#[derive(Debug, Clone, PartialEq)]
pub struct AppSnapshot {
    pub version: i32,
    pub workspaces: Option<Vec<Workspace>>,
    pub sessions: Option<Vec<Arc<RunSession>>>,
    pub active_workspace_id: Option<String>,
    pub active_session_id: Option<String>,
    /// Null opens the active workspace only; `[]` means all closed (`WorkspaceDisclosureConverter`: anything but an array of strings
    /// reads as null).
    pub expanded_workspace_ids: Option<Vec<String>>,
    pub layout: Option<String>,
    pub pane_layouts: Option<IndexMap<String, Option<PaneLayoutNode>>>,
    pub pane_layout_modes: Option<StringMap>,
    pub pane_layout_active_session_ids: Option<StringMap>,
    pub theme: Option<String>,
    pub sidebar_width: f64,
    pub sidebar_collapsed: bool,
    pub trusted_status_lines: Option<StringMap>,
    pub status_line_enabled: bool,
    pub completion_notifications_enabled: bool,
    /// `LenientNullableBoolConverter`: anything but true or false reads as null.
    pub auto_update_clis: Option<bool>,
    pub auto_update_plugins: Option<bool>,
    pub auto_login_clis: Option<bool>,
    pub background_work_open: Option<bool>,
    pub last_create_location: Option<String>,
    pub antigravity_migrated: Option<bool>,
    /// 에이전트 창's "에이전트에게 진행 체크리스트 요청": a request asks Antigravity, Codex, opencode and Hermes to keep a plan (on
    /// unless false; [`AppSnapshot::agent_checklist_on`]). Saved as `agentChecklist`; 0.5.4's `antigravityChecklist` is read too.
    pub agent_checklist: Option<bool>,
    /// 에이전트 창's "다른 워크스페이스 온디맨드 참조": attaches the read-only `mighty-workspace` MCP server so agents can query other
    /// local workspaces on demand (on unless false; [`AppSnapshot::cross_workspace_lookup_on`]). Saved as `crossWorkspaceLookup`.
    pub cross_workspace_lookup: Option<bool>,
    pub reference_bubble_width: Option<f64>,
    pub reference_bubble_height: Option<f64>,
    pub claude_direct_usage_lookup_enabled: bool,
    pub app_update_auto_check: bool,
    pub app_update_last_checked_at: Option<String>,
    pub app_update_manifest_url_override: Option<String>,
    pub language_preference: Option<String>,
    pub settings_pane: Option<String>,
    pub mobile_remote: Option<MobileRemoteSettings>,
    pub model_defaults: Option<ModelDefaultsConfig>,
    pub phase_models: Option<PhaseModelsSnapshot>,
}

impl Default for AppSnapshot {
    /// `new AppSnapshot()`.
    fn default() -> Self {
        AppSnapshot {
            version: 1,
            workspaces: Some(vec![]),
            sessions: Some(vec![]),
            active_workspace_id: None,
            active_session_id: None,
            expanded_workspace_ids: None,
            layout: Some("grid".into()),
            pane_layouts: None,
            pane_layout_modes: None,
            pane_layout_active_session_ids: None,
            theme: Some("dark".into()),
            sidebar_width: 252.0,
            sidebar_collapsed: false,
            trusted_status_lines: None,
            status_line_enabled: true,
            completion_notifications_enabled: true,
            auto_update_clis: None,
            auto_update_plugins: None,
            auto_login_clis: None,
            background_work_open: None,
            last_create_location: None,
            antigravity_migrated: None,
            agent_checklist: None,
            cross_workspace_lookup: None,
            reference_bubble_width: None,
            reference_bubble_height: None,
            claude_direct_usage_lookup_enabled: false,
            app_update_auto_check: true,
            app_update_last_checked_at: None,
            app_update_manifest_url_override: None,
            language_preference: Some("system".into()),
            settings_pane: Some("general".into()),
            mobile_remote: Some(MobileRemoteSettings::default()),
            model_defaults: None,
            phase_models: None,
        }
    }
}

/// `LenientNullableBoolConverter.Read`.
fn lenient_bool(value: Option<&Value>) -> Option<bool> {
    value.and_then(Value::as_bool)
}

/// `LenientNullableDoubleConverter.Read`: a finite number, else null.
fn lenient_double(value: Option<&Value>) -> Option<f64> {
    value.filter(|v| v.is_number()).and_then(Value::as_f64).filter(|n| n.is_finite())
}

/// `LenientJsonConverter<string>.Read`: a string, else null.
fn lenient_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

/// `LenientListConverter<string>.Read`: an array's strings (a null or any other element left out); anything else is null.
fn lenient_strings(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        Some(Value::Array(items)) => Some(items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect()),
        _ => None,
    }
}

/// `LenientNullableDoubleConverter.Write`: a number when finite, else a null literal (a null is never handed to it).
fn write_lenient_double(out: &mut JsonOut, name: &str, value: Option<f64>) {
    if let Some(number) = value {
        out.name(name);
        if number.is_finite() { out.float(number) } else { out.null() }
    }
}

impl Wire for AppSnapshot {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "the state")?;
        let expanded = match o.get("expandedWorkspaceIds") {
            Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
                Some(items.iter().filter_map(Value::as_str).map(str::to_string).collect())
            }
            _ => None,
        };
        Ok(AppSnapshot {
            version: json::int32(o.get("version"), 1)?,
            workspaces: match json::list(o.get("workspaces"), Workspace::read)? {
                // A null workspace never reaches a reader: the load leaves out every workspace that is no object.
                Some(items) => Some(items.into_iter().flatten().collect()),
                None if o.get("workspaces").is_none() => Some(vec![]),
                None => None,
            },
            sessions: match json::list(o.get("sessions"), RunSession::read)? {
                Some(items) => Some(items.into_iter().flatten().map(Arc::new).collect()),
                None if o.get("sessions").is_none() => Some(vec![]),
                None => None,
            },
            active_workspace_id: json::opt_string(o.get("activeWorkspaceId"))?,
            active_session_id: json::opt_string(o.get("activeSessionId"))?,
            expanded_workspace_ids: expanded,
            layout: json::string(o.get("layout"), some("grid"))?,
            pane_layouts: json::dictionary(o.get("paneLayouts"), PaneLayoutNode::read)?,
            pane_layout_modes: json::string_map(o.get("paneLayoutModes"))?,
            pane_layout_active_session_ids: json::string_map(o.get("paneLayoutActiveSessionIds"))?,
            theme: json::string(o.get("theme"), some("dark"))?,
            sidebar_width: json::float64(o.get("sidebarWidth"), 252.0)?,
            sidebar_collapsed: json::boolean(o.get("sidebarCollapsed"), false)?,
            trusted_status_lines: json::string_map(o.get("trustedStatusLines"))?,
            status_line_enabled: json::boolean(o.get("statusLineEnabled"), true)?,
            completion_notifications_enabled: json::boolean(o.get("completionNotificationsEnabled"), true)?,
            auto_update_clis: lenient_bool(o.get("autoUpdateCLIs")),
            auto_update_plugins: json::opt_boolean(o.get("autoUpdatePlugins"))?,
            auto_login_clis: lenient_bool(o.get("autoLoginCLIs")),
            background_work_open: lenient_bool(o.get("backgroundWorkOpen")),
            last_create_location: lenient_string(o.get("lastCreateLocation")),
            antigravity_migrated: lenient_bool(o.get("antigravityMigrated")),
            agent_checklist: lenient_bool(o.get("agentChecklist")).or_else(|| lenient_bool(o.get("antigravityChecklist"))),
            cross_workspace_lookup: lenient_bool(o.get("crossWorkspaceLookup")),
            reference_bubble_width: lenient_double(o.get("referenceBubbleWidth")),
            reference_bubble_height: lenient_double(o.get("referenceBubbleHeight")),
            claude_direct_usage_lookup_enabled: json::boolean(o.get("claudeDirectUsageLookupEnabled"), false)?,
            app_update_auto_check: json::boolean(o.get("appUpdateAutoCheck"), true)?,
            app_update_last_checked_at: json::opt_string(o.get("appUpdateLastCheckedAt"))?,
            app_update_manifest_url_override: json::opt_string(o.get("appUpdateManifestUrlOverride"))?,
            language_preference: json::string(o.get("languagePreference"), some("system"))?,
            settings_pane: json::string(o.get("settingsPane"), some("general"))?,
            mobile_remote: match o.get("mobileRemote") {
                None => Some(MobileRemoteSettings::default()),
                value => json::record(value)?,
            },
            model_defaults: json::record(o.get("modelDefaults"))?,
            phase_models: json::record(o.get("phaseModels"))?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        self.write_with(out, true, true);
    }
}

impl AppSnapshot {
    /// Whether an agent pane's requests ask its agent to keep a progress checklist: on unless switched off, so a state file that
    /// never named it (every one before the switch) asks.
    pub fn agent_checklist_on(&self) -> bool {
        self.agent_checklist != Some(false)
    }

    /// Whether local agent runs get the read-only `mighty-workspace` MCP server for on-demand cross-workspace lookup: on unless false.
    pub fn cross_workspace_lookup_on(&self) -> bool {
        self.cross_workspace_lookup != Some(false)
    }

    /// Written with or without its `sessions` (the bridge's state text leaves them out), and with or without the graph agents'
    /// output files (`WebDelta.PageSnapshot`: the page never gets them).
    pub fn write_with(&self, out: &mut JsonOut, sessions: bool, output_files: bool) {
        self.write_sessions(out, if sessions { Sessions::Whole { output_files } } else { Sessions::None });
    }

    /// The split state's index (`workspace-state.json` with `"sessionFiles": 1`): every pane without its body, which its own file
    /// holds, with the revision of the body written for it (`body_revision`).
    pub fn write_split_index(&self, out: &mut JsonOut, body_revision: &dyn Fn(&str) -> u64) {
        self.write_sessions(out, Sessions::Index(body_revision));
    }

    fn write_sessions(&self, out: &mut JsonOut, sessions: Sessions<'_>) {
        out.begin_object();
        out.prop_int("version", self.version as i64);
        if matches!(sessions, Sessions::Index(_)) {
            out.prop_int(SPLIT_MARKER, 1);
        }
        out.prop_list("workspaces", self.workspaces.as_ref());
        if let Some(list) = &self.sessions {
            match sessions {
                Sessions::None => {}
                Sessions::Whole { output_files } => {
                    out.name("sessions");
                    out.begin_array();
                    for session in list {
                        session.write_with(out, true, output_files);
                    }
                    out.end_array();
                }
                Sessions::Index(revision) => {
                    out.name("sessions");
                    out.begin_array();
                    for session in list {
                        session.write_index(out, revision(session.id()));
                    }
                    out.end_array();
                }
            }
        }
        out.prop_str("activeWorkspaceId", self.active_workspace_id.as_deref());
        out.prop_str("activeSessionId", self.active_session_id.as_deref());
        out.prop_strings("expandedWorkspaceIds", self.expanded_workspace_ids.as_ref());
        out.prop_str("layout", self.layout.as_deref());
        if let Some(layouts) = &self.pane_layouts {
            out.name("paneLayouts");
            out.begin_object();
            for (key, node) in layouts {
                out.name(key);
                match node {
                    Some(node) => node.write(out),
                    None => out.null(),
                }
            }
            out.end_object();
        }
        out.prop_string_map("paneLayoutModes", self.pane_layout_modes.as_ref());
        out.prop_string_map("paneLayoutActiveSessionIds", self.pane_layout_active_session_ids.as_ref());
        out.prop_str("theme", self.theme.as_deref());
        out.prop_float("sidebarWidth", self.sidebar_width);
        out.prop_bool("sidebarCollapsed", self.sidebar_collapsed);
        out.prop_string_map("trustedStatusLines", self.trusted_status_lines.as_ref());
        out.prop_bool("statusLineEnabled", self.status_line_enabled);
        out.prop_bool("completionNotificationsEnabled", self.completion_notifications_enabled);
        out.prop_opt_bool("autoUpdateCLIs", self.auto_update_clis);
        out.prop_opt_bool("autoUpdatePlugins", self.auto_update_plugins);
        out.prop_opt_bool("autoLoginCLIs", self.auto_login_clis);
        out.prop_opt_bool("backgroundWorkOpen", self.background_work_open);
        out.prop_str("lastCreateLocation", self.last_create_location.as_deref());
        out.prop_opt_bool("antigravityMigrated", self.antigravity_migrated);
        out.prop_opt_bool("agentChecklist", self.agent_checklist);
        out.prop_opt_bool("crossWorkspaceLookup", self.cross_workspace_lookup);
        write_lenient_double(out, "referenceBubbleWidth", self.reference_bubble_width);
        write_lenient_double(out, "referenceBubbleHeight", self.reference_bubble_height);
        out.prop_bool("claudeDirectUsageLookupEnabled", self.claude_direct_usage_lookup_enabled);
        out.prop_bool("appUpdateAutoCheck", self.app_update_auto_check);
        out.prop_str("appUpdateLastCheckedAt", self.app_update_last_checked_at.as_deref());
        out.prop_str("appUpdateManifestUrlOverride", self.app_update_manifest_url_override.as_deref());
        out.prop_str("languagePreference", self.language_preference.as_deref());
        out.prop_str("settingsPane", self.settings_pane.as_deref());
        out.prop("mobileRemote", self.mobile_remote.as_ref());
        out.prop("modelDefaults", self.model_defaults.as_ref());
        out.prop("phaseModels", self.phase_models.as_ref());
        out.end_object();
    }

    pub fn workspaces(&self) -> &[Workspace] {
        self.workspaces.as_deref().unwrap_or(&[])
    }
    pub fn sessions(&self) -> &[Arc<RunSession>] {
        self.sessions.as_deref().unwrap_or(&[])
    }
    pub fn theme(&self) -> &str {
        self.theme.as_deref().unwrap_or("")
    }
    pub fn layout(&self) -> &str {
        self.layout.as_deref().unwrap_or("")
    }
    pub fn language_preference(&self) -> &str {
        self.language_preference.as_deref().unwrap_or("")
    }
    pub fn workspace(&self, id: &str) -> Option<&Workspace> {
        self.workspaces().iter().find(|w| w.id() == id)
    }
    pub fn session(&self, id: &str) -> Option<&Arc<RunSession>> {
        self.sessions().iter().find(|s| s.id() == id)
    }
    /// MC-58: the pane that is the bot `text` names (`@reviewer` or `reviewer`); a mirror's panes are the host's and never answer.
    pub fn bot_pane(&self, text: &str) -> Option<&Arc<RunSession>> {
        PaneBot::handle_of(text)?;
        self.sessions().iter().find(|s| s.mirror.is_none() && s.bot.as_ref().is_some_and(|b| b.answers_to(text)))
    }
    /// MC-58: the pane `text` names: a pane id, or a bot's `@handle`.
    pub fn pane_or_bot(&self, text: &str) -> Option<&Arc<RunSession>> {
        if text.trim_start().starts_with('@') { self.bot_pane(text) } else { self.session(text) }
    }
    /// MC-58: the panes that are bots, in the snapshot's order (a mirror's left out).
    pub fn bots(&self) -> impl Iterator<Item = &Arc<RunSession>> {
        self.sessions().iter().filter(|s| s.mirror.is_none() && s.bot.is_some())
    }
    /// The ids of a workspace's panes, in the snapshot's order.
    pub fn pane_ids(&self, workspace_id: &str) -> Vec<String> {
        self.sessions().iter().filter(|s| s.workspace_id() == workspace_id).map(|s| s.id().to_string()).collect()
    }
    pub fn sessions_mut(&mut self) -> &mut Vec<Arc<RunSession>> {
        self.sessions.get_or_insert_with(Vec::new)
    }
    pub fn workspaces_mut(&mut self) -> &mut Vec<Workspace> {
        self.workspaces.get_or_insert_with(Vec::new)
    }

    /// Whether every non-session field written by `write_with(..., false, false)` is identical (ordered maps and bit-exact floats),
    /// guaranteeing byte-identical JSON output when `true`.
    pub fn same_non_session_fields(&self, other: &AppSnapshot) -> bool {
        let AppSnapshot {
            version,
            workspaces,
            sessions: _,
            active_workspace_id,
            active_session_id,
            expanded_workspace_ids,
            layout,
            pane_layouts,
            pane_layout_modes,
            pane_layout_active_session_ids,
            theme,
            sidebar_width,
            sidebar_collapsed,
            trusted_status_lines,
            status_line_enabled,
            completion_notifications_enabled,
            auto_update_clis,
            auto_update_plugins,
            auto_login_clis,
            background_work_open,
            last_create_location,
            antigravity_migrated,
            agent_checklist,
            cross_workspace_lookup,
            reference_bubble_width,
            reference_bubble_height,
            claude_direct_usage_lookup_enabled,
            app_update_auto_check,
            app_update_last_checked_at,
            app_update_manifest_url_override,
            language_preference,
            settings_pane,
            mobile_remote,
            model_defaults,
            phase_models,
        } = self;
        *version == other.version
            && active_workspace_id == &other.active_workspace_id
            && active_session_id == &other.active_session_id
            && layout == &other.layout
            && theme == &other.theme
            && sidebar_width.to_bits() == other.sidebar_width.to_bits()
            && *sidebar_collapsed == other.sidebar_collapsed
            && *status_line_enabled == other.status_line_enabled
            && *completion_notifications_enabled == other.completion_notifications_enabled
            && auto_update_clis == &other.auto_update_clis
            && auto_update_plugins == &other.auto_update_plugins
            && auto_login_clis == &other.auto_login_clis
            && background_work_open == &other.background_work_open
            && last_create_location == &other.last_create_location
            && antigravity_migrated == &other.antigravity_migrated
            && agent_checklist == &other.agent_checklist
            && cross_workspace_lookup == &other.cross_workspace_lookup
            && same_opt_f64(*reference_bubble_width, other.reference_bubble_width)
            && same_opt_f64(*reference_bubble_height, other.reference_bubble_height)
            && *claude_direct_usage_lookup_enabled == other.claude_direct_usage_lookup_enabled
            && *app_update_auto_check == other.app_update_auto_check
            && app_update_last_checked_at == &other.app_update_last_checked_at
            && app_update_manifest_url_override == &other.app_update_manifest_url_override
            && language_preference == &other.language_preference
            && settings_pane == &other.settings_pane
            && expanded_workspace_ids == &other.expanded_workspace_ids
            && workspaces == &other.workspaces
            && same_ordered_map(pane_layouts.as_ref(), other.pane_layouts.as_ref(), |a, b| match (a, b) {
                (None, None) => true,
                (Some(x), Some(y)) => same_layout_node(x, y),
                _ => false,
            })
            && same_ordered_map(pane_layout_modes.as_ref(), other.pane_layout_modes.as_ref(), PartialEq::eq)
            && same_ordered_map(pane_layout_active_session_ids.as_ref(), other.pane_layout_active_session_ids.as_ref(), PartialEq::eq)
            && same_ordered_map(trusted_status_lines.as_ref(), other.trusted_status_lines.as_ref(), PartialEq::eq)
            && mobile_remote == &other.mobile_remote
            && same_opt(model_defaults.as_ref(), other.model_defaults.as_ref(), same_model_defaults)
            && phase_models == &other.phase_models
    }

    /// Whether the state holds anything of the remote mirror (a mirror workspace or a mirrored pane).
    pub fn has_mirrors(&self) -> bool {
        self.workspaces().iter().any(|w| w.remote.as_ref().is_some_and(|r| r.mirror)) || self.sessions().iter().any(|s| s.mirror.is_some())
    }

    /// C# `RemoteMirrorSnapshots.WithoutMirrors`: the snapshot as it is saved, without the remote mirror's workspaces, their panes and
    /// what refers to them (they live only while connected; the host's transcripts never land in this app's state file). A snapshot
    /// without mirrors comes back as it is.
    pub fn without_mirrors(&self) -> AppSnapshot {
        if !self.has_mirrors() {
            return self.clone();
        }
        let mirrored: std::collections::HashSet<&str> =
            self.workspaces().iter().filter(|w| w.remote.as_ref().is_some_and(|r| r.mirror)).map(Workspace::id).collect();
        let gone: std::collections::HashSet<&str> =
            self.sessions().iter().filter(|s| s.mirror.is_some() || mirrored.contains(s.workspace_id())).map(|s| s.id()).collect();
        let workspaces: Vec<Workspace> = self.workspaces().iter().filter(|w| !mirrored.contains(w.id())).cloned().collect();
        fn without<T: Clone>(map: Option<&IndexMap<String, T>>, mirrored: &std::collections::HashSet<&str>) -> Option<IndexMap<String, T>> {
            map.map(|m| m.iter().filter(|(k, _)| !mirrored.contains(k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
        }
        AppSnapshot {
            sessions: Some(self.sessions().iter().filter(|s| !gone.contains(s.id())).cloned().collect()),
            pane_layouts: without(self.pane_layouts.as_ref(), &mirrored),
            pane_layout_modes: without(self.pane_layout_modes.as_ref(), &mirrored),
            pane_layout_active_session_ids: without(self.pane_layout_active_session_ids.as_ref(), &mirrored),
            expanded_workspace_ids: self
                .expanded_workspace_ids
                .as_ref()
                .map(|ids| ids.iter().filter(|id| !mirrored.contains(id.as_str())).cloned().collect()),
            active_workspace_id: match &self.active_workspace_id {
                Some(active) if mirrored.contains(active.as_str()) => workspaces.first().map(|w| w.id().to_string()),
                other => other.clone(),
            },
            active_session_id: match &self.active_session_id {
                Some(pane) if gone.contains(pane.as_str()) => None,
                other => other.clone(),
            },
            workspaces: Some(workspaces),
            ..self.clone()
        }
    }

    /// C# `RemoteMirrorSnapshots.OwnSessionCount`: this PC's own panes, the ones the 128-pane limit (and the state file) counts; mirrored
    /// panes are the host's.
    pub fn own_session_count(&self) -> usize {
        self.sessions().iter().filter(|s| s.mirror.is_none()).count()
    }
}

// ── Workspace ───────────────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    pub id: Option<String>,
    pub name: Option<String>,
    pub path: Option<String>,
    pub created_at: Option<String>,
    pub model_defaults: Option<ModelDefaultsConfig>,
    /// Set when the folder is on another computer.
    pub remote: Option<RemoteWorkspaceReference>,
    /// Where the workspace's agent panes work (`PaneWorktree.cs`): ask (None, older saves), `further` or `every`. Lenient: anything but
    /// a string reads as None.
    pub pane_worktree_mode: Option<String>,
    /// Run once in each new pane worktree right after it is made (`npm ci`, …); None is none. Lenient.
    pub worktree_setup_command: Option<String>,
    /// Files the workspace folder has but git leaves out of a worktree (`.env`, …), copied into each new pane worktree. Lenient list.
    pub worktree_copy_paths: Option<Vec<String>>,
    /// Pane worktrees kept after their pane closed with work in them; damage drops that record only.
    pub kept_pane_worktrees: Option<Vec<KeptPaneWorktree>>,
    /// Optional MightyWork project key mapped to this workspace (e.g. `"MC"`).
    pub mightywork_project: Option<String>,
    /// Optional MightyWork MCP endpoint URL override for this workspace.
    pub mightywork_url: Option<String>,
    /// Optional GitHub `owner/repo` mapped to this workspace.
    pub github_repo: Option<String>,
    /// Whether starting/PR/merging a linked task automatically updates its MightyWork stage (`None` = true).
    pub mightywork_auto_sync: Option<bool>,
    /// MC-31: the issue trackers this workspace uses beyond MightyWork and GitHub (Linear, Jira, GitLab), one record each. Never a
    /// token: tokens live only in the OS keychain. Lenient list.
    pub issue_trackers: Option<Vec<IssueTrackerConfig>>,
    /// MC-33: set when the folder is on an SSH machine (nothing installed there). `path` is then a local empty folder of the app's own
    /// (`<data>/ssh/workspaces/<id>`), so every local reader sees an empty folder; panes reach the remote folder through ssh.
    pub ssh: Option<SshWorkspaceRef>,
    /// MC-34: the OS sandbox of this workspace's agent runs (Mac Seatbelt, the network allow list, the reviewer). None is off.
    pub sandbox: Option<SandboxConfig>,
    /// MC-59: the app's own home of the independent bots (`<data>/bots`): each of its panes is a bot working in a folder of its own
    /// (`working_folder`); the sidebar lists them under 봇, never as a workspace. Only true is kept.
    pub bot_home: Option<bool>,
}

/// MC-33: which SSH machine and remote folder a workspace opens.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SshWorkspaceRef {
    /// The machine's id in the app's SSH machine list.
    pub machine: String,
    /// The remote folder: absolute or under `~`.
    pub dir: String,
    /// The machine's name as it was when opened (`devbox`, `me@box:2222`), shown when the machine is gone.
    pub host: String,
}

impl Wire for SshWorkspaceRef {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "an ssh reference")?;
        Ok(SshWorkspaceRef {
            machine: lenient_string(o.get("machine")).unwrap_or_default(),
            dir: lenient_string(o.get("dir")).unwrap_or_default(),
            host: lenient_string(o.get("host")).unwrap_or_default(),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("machine", Some(&self.machine));
        out.prop_str("dir", Some(&self.dir));
        out.prop_str("host", Some(&self.host));
        out.end_object();
    }
}

impl Default for Workspace {
    /// `new Workspace()`: a new id and the time now.
    fn default() -> Self {
        Workspace {
            id: Some(new_id()),
            name: Some("Workspace".into()),
            path: Some(String::new()),
            created_at: Some(time::now_o()),
            model_defaults: None,
            remote: None,
            pane_worktree_mode: None,
            worktree_setup_command: None,
            worktree_copy_paths: None,
            kept_pane_worktrees: None,
            mightywork_project: None,
            mightywork_url: None,
            github_repo: None,
            mightywork_auto_sync: None,
            issue_trackers: None,
            ssh: None,
            sandbox: None,
            bot_home: None,
        }
    }
}

impl Wire for Workspace {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a workspace")?;
        Ok(Workspace {
            id: json::string(o.get("id"), || Some(new_id()))?,
            name: json::string(o.get("name"), some("Workspace"))?,
            path: json::string(o.get("path"), some(""))?,
            created_at: json::string(o.get("createdAt"), || Some(time::now_o()))?,
            model_defaults: json::record(o.get("modelDefaults"))?,
            remote: json::record(o.get("remote"))?,
            pane_worktree_mode: lenient_string(o.get("paneWorktreeMode")),
            worktree_setup_command: lenient_string(o.get("worktreeSetupCommand")),
            worktree_copy_paths: lenient_strings(o.get("worktreeCopyPaths")),
            kept_pane_worktrees: lenient_list(o.get("keptPaneWorktrees")),
            mightywork_project: lenient_string(o.get("mightyworkProject")),
            mightywork_url: lenient_string(o.get("mightyworkUrl")),
            github_repo: lenient_string(o.get("githubRepo")),
            mightywork_auto_sync: lenient_bool(o.get("mightyworkAutoSync")),
            issue_trackers: lenient_list(o.get("issueTrackers")),
            ssh: lenient_record(o.get("ssh")),
            sandbox: lenient_record(o.get("sandbox")),
            bot_home: (lenient_bool(o.get("botHome")) == Some(true)).then_some(true),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", self.id.as_deref());
        out.prop_str("name", self.name.as_deref());
        out.prop_str("path", self.path.as_deref());
        out.prop_str("createdAt", self.created_at.as_deref());
        out.prop("modelDefaults", self.model_defaults.as_ref());
        out.prop("remote", self.remote.as_ref());
        out.prop_str("paneWorktreeMode", self.pane_worktree_mode.as_deref());
        out.prop_str("worktreeSetupCommand", self.worktree_setup_command.as_deref());
        out.prop_strings("worktreeCopyPaths", self.worktree_copy_paths.as_ref());
        out.prop_list("keptPaneWorktrees", self.kept_pane_worktrees.as_ref());
        out.prop_str("mightyworkProject", self.mightywork_project.as_deref());
        out.prop_str("mightyworkUrl", self.mightywork_url.as_deref());
        out.prop_str("githubRepo", self.github_repo.as_deref());
        out.prop_opt_bool("mightyworkAutoSync", self.mightywork_auto_sync);
        out.prop_list("issueTrackers", self.issue_trackers.as_ref());
        out.prop("ssh", self.ssh.as_ref());
        out.prop("sandbox", self.sandbox.as_ref());
        out.prop_opt_bool("botHome", self.bot_home.filter(|home| *home));
        out.end_object();
    }
}

impl Workspace {
    /// MC-33: the folder is on an SSH machine.
    pub fn is_ssh(&self) -> bool {
        self.ssh.is_some()
    }

    /// The folder is on this computer: neither another Mighty Code's (`remote`) nor an SSH machine's.
    pub fn is_local(&self) -> bool {
        self.remote.is_none() && self.ssh.is_none()
    }

    /// MC-59: whether this is the independent bots' home.
    pub fn is_bot_home(&self) -> bool {
        self.bot_home == Some(true)
    }

    pub fn mightywork_auto_sync_on(&self) -> bool {
        self.mightywork_auto_sync != Some(false)
    }

    /// MC-34: the workspace's sandbox when it is turned on (an SSH or remote workspace never has one: its runs are not on this computer).
    pub fn sandbox_on(&self) -> Option<&SandboxConfig> {
        self.sandbox.as_ref().filter(|s| s.enabled && self.is_local())
    }

    /// MC-31: the workspace's record for `tracker` (`"linear"`, `"jira"`, `"gitlab"`), when it has one.
    pub fn issue_tracker(&self, tracker: &str) -> Option<&IssueTrackerConfig> {
        self.issue_trackers.as_ref()?.iter().find(|c| c.tracker == tracker)
    }
}

/// MC-31: one external issue tracker a workspace uses (Linear, Jira Cloud or GitLab): which team or project, the default filter and
/// the write-back switches. Its token is never here (the OS keychain only).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IssueTrackerConfig {
    /// `"linear"`, `"jira"` or `"gitlab"`.
    pub tracker: String,
    pub enabled: bool,
    /// The Linear team key (`ENG`), the Jira project key (`PROJ`) or the GitLab project path (`group/app`).
    pub project: Option<String>,
    /// The Jira site (`https://team.atlassian.net`) or the GitLab address (`https://gitlab.com`).
    pub site: Option<String>,
    /// The Jira account's email (Basic auth's user name; not a secret).
    pub email: Option<String>,
    /// Default filter: assigned to me (None = true).
    pub filter_mine: Option<bool>,
    /// Default filter: not done (None = true).
    pub filter_open: Option<bool>,
    /// Write back: starting work moves the issue to in progress (None = false).
    pub write_start: Option<bool>,
    /// Write back: a pull request moves the issue to in review (None = false).
    pub write_pr: Option<bool>,
    /// Write back: a merge moves the issue to done (None = false).
    pub write_merge: Option<bool>,
    /// Write back: a pull request's link is posted on the issue (None = true).
    pub comment_pr: Option<bool>,
}

impl IssueTrackerConfig {
    pub fn filter_mine_on(&self) -> bool {
        self.filter_mine != Some(false)
    }
    pub fn filter_open_on(&self) -> bool {
        self.filter_open != Some(false)
    }
    pub fn write_start_on(&self) -> bool {
        self.write_start == Some(true)
    }
    pub fn write_pr_on(&self) -> bool {
        self.write_pr == Some(true)
    }
    pub fn write_merge_on(&self) -> bool {
        self.write_merge == Some(true)
    }
    pub fn comment_pr_on(&self) -> bool {
        self.comment_pr != Some(false)
    }
}

impl Wire for IssueTrackerConfig {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "an issue tracker")?;
        let tracker = lenient_string(o.get("tracker")).unwrap_or_default();
        if !matches!(tracker.as_str(), "linear" | "jira" | "gitlab") {
            return json::fail("issue tracker must be linear, jira or gitlab");
        }
        Ok(IssueTrackerConfig {
            tracker,
            enabled: lenient_bool(o.get("enabled")).unwrap_or(false),
            project: lenient_string(o.get("project")),
            site: lenient_string(o.get("site")),
            email: lenient_string(o.get("email")),
            filter_mine: lenient_bool(o.get("filterMine")),
            filter_open: lenient_bool(o.get("filterOpen")),
            write_start: lenient_bool(o.get("writeStart")),
            write_pr: lenient_bool(o.get("writePr")),
            write_merge: lenient_bool(o.get("writeMerge")),
            comment_pr: lenient_bool(o.get("commentPr")),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("tracker", Some(&self.tracker));
        out.prop_bool("enabled", self.enabled);
        out.prop_str("project", self.project.as_deref());
        out.prop_str("site", self.site.as_deref());
        out.prop_str("email", self.email.as_deref());
        out.prop_opt_bool("filterMine", self.filter_mine);
        out.prop_opt_bool("filterOpen", self.filter_open);
        out.prop_opt_bool("writeStart", self.write_start);
        out.prop_opt_bool("writePr", self.write_pr);
        out.prop_opt_bool("writeMerge", self.write_merge);
        out.prop_opt_bool("commentPr", self.comment_pr);
        out.end_object();
    }
}

// ── the sandbox of a workspace's runs (MC-34) ───────────────────────────────────────────────────────────────────────────

/// MC-34: a workspace's sandbox settings. Everything is off until the person turns it on; the switches inside default to the safe side.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SandboxConfig {
    /// Agent runs start under the OS sandbox (the Mac's Seatbelt; Windows' experimental one since MC-42).
    pub enabled: bool,
    /// Outbound connections only through the app's allow-list proxy (None = true).
    pub limit_network: Option<bool>,
    /// The person's own allowed hosts beside the agent's API hosts: `host` (port 443), `host:port`, `*.host`.
    pub allowed_hosts: Vec<String>,
    /// `.mighty.yaml`, `AGENTS.md`, `CLAUDE.md` and the agents' project settings stay read-only (None = true).
    pub protect_instructions: Option<bool>,
    /// More workspace files kept read-only, relative to the workspace folder.
    pub read_only_paths: Vec<String>,
    /// A read-only reviewer agent judges what crosses the boundary (None = false).
    pub reviewer: Option<bool>,
    /// The reviewer's agent (`claude`, `codex`; None = claude) and model (None = the agent's cheap one).
    pub reviewer_agent: Option<String>,
    pub reviewer_model: Option<String>,
}

impl SandboxConfig {
    pub fn limit_network_on(&self) -> bool {
        self.limit_network != Some(false)
    }
    pub fn protect_instructions_on(&self) -> bool {
        self.protect_instructions != Some(false)
    }
    pub fn reviewer_on(&self) -> bool {
        self.reviewer == Some(true)
    }
}

impl Wire for SandboxConfig {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a sandbox")?;
        Ok(SandboxConfig {
            enabled: lenient_bool(o.get("enabled")).unwrap_or(false),
            limit_network: lenient_bool(o.get("limitNetwork")),
            allowed_hosts: lenient_strings(o.get("allowedHosts")).unwrap_or_default(),
            protect_instructions: lenient_bool(o.get("protectInstructions")),
            read_only_paths: lenient_strings(o.get("readOnlyPaths")).unwrap_or_default(),
            reviewer: lenient_bool(o.get("reviewer")),
            reviewer_agent: lenient_string(o.get("reviewerAgent")),
            reviewer_model: lenient_string(o.get("reviewerModel")),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_bool("enabled", self.enabled);
        out.prop_opt_bool("limitNetwork", self.limit_network);
        out.prop_strings("allowedHosts", Some(&self.allowed_hosts));
        out.prop_opt_bool("protectInstructions", self.protect_instructions);
        out.prop_strings("readOnlyPaths", Some(&self.read_only_paths));
        out.prop_opt_bool("reviewer", self.reviewer);
        out.prop_str("reviewerAgent", self.reviewer_agent.as_deref());
        out.prop_str("reviewerModel", self.reviewer_model.as_deref());
        out.end_object();
    }
}

// ── a pane worktree's branch (MC-31) ───────────────────────────────────────────────────────────────────────────────────────

/// MC-31: the prefixes an issue's worktree branch may start with (`lin/ENG-123-fix-login`, `jira/PROJ-7-…`, `gl/12-…`). A pane whose
/// session id is `<prefix>-<rest>` with one of these may work on the branch `<prefix>/<rest>` instead of `mighty/<session id>`: the
/// branch still follows from the id alone, so no two panes can claim one branch, and `mighty/…` never reads as an issue branch.
pub const ISSUE_BRANCH_PREFIXES: [&str; 3] = ["lin", "jira", "gl"];

/// MC-31: the issue branch the session id `id` names (`lin-ENG-1-x` → `lin/ENG-1-x`), or None when it names none.
pub fn issue_pane_branch(id: &str) -> Option<String> {
    let (prefix, rest) = id.split_once('-')?;
    (ISSUE_BRANCH_PREFIXES.contains(&prefix) && !rest.is_empty()).then(|| format!("{prefix}/{rest}"))
}

/// MC-31: whether `branch` is one the pane `id`'s own worktree may work on: `mighty/<id>`, or the issue branch its id names.
pub fn is_pane_branch(branch: &str, id: &str) -> bool {
    branch.strip_prefix("mighty/") == Some(id) || issue_pane_branch(id).as_deref() == Some(branch)
}

/// MC-31: the session id whose own branch `branch` is (`mighty/<id>`, or `<prefix>/<rest>` for `<prefix>-<rest>`), or None.
pub fn pane_session_of_branch(branch: &str) -> Option<String> {
    if let Some(id) = branch.strip_prefix("mighty/") {
        return Some(id.to_string());
    }
    let (prefix, rest) = branch.split_once('/')?;
    (ISSUE_BRANCH_PREFIXES.contains(&prefix) && !rest.is_empty() && !rest.contains('/')).then(|| format!("{prefix}-{rest}"))
}

// ── PaneWorktree ────────────────────────────────────────────────────────────────────────────────────────────────────────

/// External task or issue linked to an agent pane / worktree (`"mightywork"` or `"github"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedTask {
    pub source: String,
    pub project: Option<String>,
    pub key: String,
    pub title: Option<String>,
    pub url: Option<String>,
}

impl Wire for LinkedTask {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a linked task")?;
        let source = lenient_string(o.get("source")).unwrap_or_default();
        let key = lenient_string(o.get("key")).unwrap_or_default();
        if source.is_empty() || key.is_empty() {
            return json::fail("linked task requires source and key");
        }
        Ok(LinkedTask {
            source,
            project: lenient_string(o.get("project")),
            key,
            title: lenient_string(o.get("title")),
            url: lenient_string(o.get("url")),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("source", Some(&self.source));
        out.prop_str("project", self.project.as_deref());
        out.prop_str("key", Some(&self.key));
        out.prop_str("title", self.title.as_deref());
        out.prop_str("url", self.url.as_deref());
        out.end_object();
    }
}

/// C# `PaneWorktree` (Core/PaneWorktree.cs): a workspace's agent pane that works in its own git worktree, branch `mighty/<session id>`
/// at the workspace branch's head, in the same place as a delegated child's. Its rules (normalize, environment) are in `normalize.rs`
/// and mighty-delegation's `pane_worktree`.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneWorktree {
    /// The worktree's top folder, `<root>\<session id>`.
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    /// The workspace folder's branch and head when the worktree was made: the branch a merge goes into.
    pub base_branch: Option<String>,
    pub base_commit: Option<String>,
    /// The top folder of the workspace's checkout, where the branch lives and where merge and discard run git.
    pub checkout: Option<String>,
    /// The `PORT` every run of the pane gets.
    pub port: i32,
    pub setup_command: Option<String>,
    /// `running`, `done` or `failed`; None without a setup command.
    pub setup_status: Option<String>,
    pub setup_exit_code: Option<i32>,
    /// The commit the workspace branch moved to by the last merge from the pane; None before one.
    pub merged_commit: Option<String>,
    /// The workspace branch's head before that merge: where an undo puts it back.
    pub pre_merge_commit: Option<String>,
    /// The folder the pane's runs work in: the worktree, or the folder in it that matches a workspace that is a subfolder of its
    /// repository.
    pub working_folder: Option<String>,
    /// Whether the pane's agent was told it works in this worktree (`PaneWorktreeNote`); only true is kept.
    pub note_given: Option<bool>,
    /// Optional task/issue linked to this worktree (`"mightywork"` or `"github"`).
    pub linked_task: Option<LinkedTask>,
    /// The first port of the worktree's port block (`MIGHTY_PORT`, `MC-25`); None when no block is assigned.
    pub port_base: Option<i32>,
    /// The ports in that block (`MIGHTY_PORT_COUNT`).
    pub port_count: Option<i32>,
}

/// The variables a worktree's scripts, tasks, run actions and agent get (`MC-25`): its port block (when one is assigned), its name,
/// the repository's main checkout and its own top folder.
pub fn worktree_variables(block: Option<(i32, i32)>, name: &str, root: &str, worktree: &str) -> Vec<(String, String)> {
    let mut values = Vec::with_capacity(5);
    if let Some((base, count)) = block.filter(|(base, count)| *base > 0 && *count > 0) {
        values.push(("MIGHTY_PORT".to_string(), base.to_string()));
        values.push(("MIGHTY_PORT_COUNT".to_string(), count.to_string()));
    }
    values.push(("MIGHTY_WORKTREE_NAME".to_string(), name.to_string()));
    values.push(("MIGHTY_ROOT_PATH".to_string(), root.to_string()));
    values.push(("MIGHTY_WORKTREE_PATH".to_string(), worktree.to_string()));
    values
}

impl Default for PaneWorktree {
    /// `new PaneWorktree()`: empty paths and names, port 0.
    fn default() -> Self {
        PaneWorktree {
            worktree_path: Some(String::new()),
            branch: Some(String::new()),
            base_branch: Some(String::new()),
            base_commit: Some(String::new()),
            checkout: Some(String::new()),
            port: 0,
            setup_command: None,
            setup_status: None,
            setup_exit_code: None,
            merged_commit: None,
            pre_merge_commit: None,
            working_folder: None,
            note_given: None,
            linked_task: None,
            port_base: None,
            port_count: None,
        }
    }
}

impl PaneWorktree {
    /// `PaneWorktree.Running`, `Done`, `Failed`.
    pub const RUNNING: &'static str = "running";
    pub const DONE: &'static str = "done";
    pub const FAILED: &'static str = "failed";

    pub fn worktree_path(&self) -> &str {
        self.worktree_path.as_deref().unwrap_or("")
    }
    pub fn branch(&self) -> &str {
        self.branch.as_deref().unwrap_or("")
    }
    pub fn base_branch(&self) -> &str {
        self.base_branch.as_deref().unwrap_or("")
    }
    pub fn base_commit(&self) -> &str {
        self.base_commit.as_deref().unwrap_or("")
    }
    pub fn checkout(&self) -> &str {
        self.checkout.as_deref().unwrap_or("")
    }

    /// `RunFolder`: the folder the pane's runs work in.
    pub fn run_folder(&self) -> &str {
        self.working_folder.as_deref().unwrap_or(self.worktree_path())
    }

    /// `Environment(workspaceFolder)`: the values every run of the pane (and its setup command) gets: its PORT, the workspace folder it
    /// was made from and its own worktree, in C#'s order; then its port block and names ([`worktree_variables`], `MC-25`).
    pub fn environment(&self, workspace_folder: &str) -> IndexMap<String, String> {
        let mut values = IndexMap::from([
            ("PORT".to_string(), self.port.to_string()),
            ("MIGHTY_WORKSPACE_FOLDER".to_string(), workspace_folder.to_string()),
            ("MIGHTY_WORKTREE".to_string(), self.run_folder().to_string()),
        ]);
        values.extend(worktree_variables(self.port_block(), &self.name(), self.checkout(), self.worktree_path()));
        values
    }

    /// The worktree's port block as `(first port, count)`; None when none is assigned.
    pub fn port_block(&self) -> Option<(i32, i32)> {
        match (self.port_base, self.port_count) {
            (Some(base), Some(count)) if base > 0 && count > 0 => Some((base, count)),
            _ => None,
        }
    }

    /// `MIGHTY_WORKTREE_NAME`: the worktree's folder name (the pane's id).
    pub fn name(&self) -> String {
        let path = self.worktree_path().trim_end_matches(['/', '\\']);
        path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
    }

    /// `PaneWorktreeNote.Text`: what the pane's agent is told once, with its first request (rule 3).
    pub fn note(&self) -> String {
        let (folder, branch, base, port) = (self.run_folder(), self.branch(), self.base_branch(), self.port);
        // MC-25: a worktree with a port block names it too, for servers that need more than one port.
        let block = self
            .port_block()
            .map(|(first, count)| {
                format!(
                    " This worktree also holds the ports {first} to {} (MIGHTY_PORT and MIGHTY_PORT_COUNT, also set).",
                    first + count - 1
                )
            })
            .unwrap_or_default();
        format!(
            "[Mighty Code] This pane works in its own git worktree ({folder}) on the branch {branch}, made from {base} so that the other panes of this workspace keep working undisturbed. Dev servers you start here use PORT={port} (already set in the environment); start them on that port.{block} When you are done, commit your work on {branch}, leaving no uncommitted changes and no new files outside a commit. The person who sent this merges your branch into {base} from the app. Do not merge, push, rebase or switch branches."
        )
    }

    /// `PaneWorktreeNote.Applies`: whether a request carries the note: the first one that is no slash command.
    pub fn note_applies(&self, input: &str) -> bool {
        self.note_given != Some(true) && !input.trim_start().starts_with('/')
    }

    /// `PaneWorktreeNote.Attach`.
    pub fn with_note(&self, input: &str) -> String {
        format!("{input}\n\n{}", self.note())
    }

    /// `PaneWorktreeNote.ConflictPrompt`: the request asking the pane's agent to bring its base in and resolve the conflict the merge
    /// refused.
    pub fn conflict_prompt(&self) -> String {
        let (branch, base) = (self.branch(), self.base_branch());
        format!(
            "[Mighty Code] Merging your branch {branch} into {base} from the app was refused: the two conflict. In this worktree, bring {base} into your branch with `git merge {base}`, resolve every conflict so that the changes on both sides are kept (ask when a choice is unclear), run the checks that apply, and commit the merge on {branch}, leaving no uncommitted changes. The person merges your branch from the app again afterwards. Do not push, rebase or switch branches."
        )
    }
}

impl Wire for PaneWorktree {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a pane worktree")?;
        Ok(PaneWorktree {
            worktree_path: json::string(o.get("worktreePath"), some(""))?,
            branch: json::string(o.get("branch"), some(""))?,
            base_branch: json::string(o.get("baseBranch"), some(""))?,
            base_commit: json::string(o.get("baseCommit"), some(""))?,
            checkout: json::string(o.get("checkout"), some(""))?,
            port: json::int32(o.get("port"), 0)?,
            setup_command: json::opt_string(o.get("setupCommand"))?,
            setup_status: json::opt_string(o.get("setupStatus"))?,
            setup_exit_code: json::opt_int32(o.get("setupExitCode"))?,
            merged_commit: json::opt_string(o.get("mergedCommit"))?,
            pre_merge_commit: json::opt_string(o.get("preMergeCommit"))?,
            working_folder: json::opt_string(o.get("workingFolder"))?,
            note_given: json::opt_boolean(o.get("noteGiven"))?,
            linked_task: lenient_record(o.get("linkedTask")),
            port_base: json::opt_int32(o.get("portBase")).ok().flatten(),
            port_count: json::opt_int32(o.get("portCount")).ok().flatten(),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("worktreePath", self.worktree_path.as_deref());
        out.prop_str("branch", self.branch.as_deref());
        out.prop_str("baseBranch", self.base_branch.as_deref());
        out.prop_str("baseCommit", self.base_commit.as_deref());
        out.prop_str("checkout", self.checkout.as_deref());
        out.prop_int("port", self.port as i64);
        out.prop_str("setupCommand", self.setup_command.as_deref());
        out.prop_str("setupStatus", self.setup_status.as_deref());
        out.prop_opt_int("setupExitCode", self.setup_exit_code.map(i64::from));
        out.prop_str("mergedCommit", self.merged_commit.as_deref());
        out.prop_str("preMergeCommit", self.pre_merge_commit.as_deref());
        out.prop_str("workingFolder", self.working_folder.as_deref());
        out.prop_opt_bool("noteGiven", self.note_given);
        out.prop("linkedTask", self.linked_task.as_ref());
        out.prop_opt_int("portBase", self.port_base.map(i64::from));
        out.prop_opt_int("portCount", self.port_count.map(i64::from));
        out.end_object();
    }
}

/// MC-28: where a pane forked from (`새 worktree로 갈라내기`, `여기서 갈라내기`): the source pane and the request the fork took the
/// conversation up to, the source's title then (for when it is closed), and what the first request still has to do with the
/// conversation. Lenient: damage loads as None.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ForkedFrom {
    pub pane_id: String,
    /// The graph run (request) the fork took the conversation up to; None for the whole conversation.
    pub request_id: Option<String>,
    pub title: Option<String>,
    /// How the conversation came along: `cli` (the agent's own fork of the source session) or `handoff` (a summary in the first
    /// request).
    pub mode: Option<String>,
    /// While true the pane's `resume_id` is still the source's session: the first run forks it (Claude `--fork-session`, Codex
    /// `exec fork`) and never resumes it. Cleared when the run names its own session. Only true is kept.
    pub fork_pending: Option<bool>,
    /// The summary the first request carries in front of what was typed (always made, so a CLI fork that cannot run falls back to
    /// it); None once sent or once the CLI fork took.
    pub handoff: Option<String>,
}

/// MC-28: the most a handoff summary keeps, in characters.
pub const FORK_HANDOFF_LIMIT: usize = 12_000;

impl ForkedFrom {
    pub const CLI: &'static str = "cli";
    pub const HANDOFF: &'static str = "handoff";

    pub fn pending(&self) -> bool {
        self.fork_pending == Some(true)
    }
}

impl Wire for ForkedFrom {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a fork link")?;
        let Some(pane_id) = json::opt_string(o.get("paneId"))?.filter(|id| !id.is_empty()) else {
            return fail("a fork link without its pane");
        };
        Ok(ForkedFrom {
            pane_id,
            request_id: lenient_string(o.get("requestId")),
            title: lenient_string(o.get("title")),
            mode: lenient_string(o.get("mode")),
            fork_pending: (lenient_bool(o.get("forkPending")) == Some(true)).then_some(true),
            handoff: lenient_string(o.get("handoff")),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("paneId", Some(&self.pane_id));
        out.prop_str("requestId", self.request_id.as_deref());
        out.prop_str("title", self.title.as_deref());
        out.prop_str("mode", self.mode.as_deref());
        out.prop_opt_bool("forkPending", self.fork_pending.filter(|p| *p));
        out.prop_str("handoff", self.handoff.as_deref());
        out.end_object();
    }
}

/// MC-58: the bot an agent pane is (`bot`): the name other panes, the command line, schedules and other agents call it by
/// (`@handle`), and the standing instructions a new conversation of the pane starts with. Lenient: damage loads as None.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PaneBot {
    /// Letters of any script (한글 included) and digits, with `-`, `_` and `.` between them, 1 to [`BOT_HANDLE_LIMIT`] characters,
    /// starting with a letter or a digit, kept as written (composed); one pane per name in a profile, compared without case.
    pub handle: String,
    /// What the bot is for and how it works, put in front of a new conversation's first request; at most
    /// [`BOT_INSTRUCTIONS_LIMIT`] bytes.
    pub instructions: Option<String>,
    /// MC-61: who chose the name: `Some(true)` the app (a friendly name in the app's language), `Some(false)` the user; None a name
    /// saved before this mark, which the app names again unless it carries instructions ([`crate::normalize::name_bots`]).
    pub auto: Option<bool>,
    /// MC-64: asleep: its schedules and triggers skip and other agents' and the CLI's requests are refused until it wakes (written
    /// only when true).
    pub asleep: Option<bool>,
    /// MC-64: what the bot may spend a day, in US dollars: its schedules and triggers skip and `ask_bot` is refused once it has.
    pub budget_per_day: Option<f64>,
}

/// MC-58: the longest bot handle, in characters.
pub const BOT_HANDLE_LIMIT: usize = 32;
/// MC-58: the most a bot's instructions keep, in bytes.
pub const BOT_INSTRUCTIONS_LIMIT: usize = 16 * 1024;

impl PaneBot {
    /// Whether `handle` is a bot name as written (no `@`): letters of any script, digits (and a decomposed 한글's marks), `-`, `_`
    /// and `.`, starting with a letter or a digit, at most [`BOT_HANDLE_LIMIT`] characters.
    pub fn valid_handle(handle: &str) -> bool {
        let mut chars = handle.chars();
        chars.next().is_some_and(char::is_alphanumeric)
            && handle.chars().count() <= BOT_HANDLE_LIMIT
            && handle.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.') || is_mark(c))
    }

    /// The name `text` gives: `@리뷰어`, `리뷰어` or `@Reviewer`, surrounding space aside, composed (NFC) and otherwise as written;
    /// None when it is not one.
    pub fn handle_of(text: &str) -> Option<String> {
        use unicode_normalization::UnicodeNormalization as _;
        let handle: String = text.trim().trim_start_matches('@').nfc().collect();
        Self::valid_handle(&handle).then_some(handle)
    }

    /// What two names are compared by: composed and without case (`Reviewer` and `reviewer` are one bot).
    pub fn key(handle: &str) -> String {
        use unicode_normalization::UnicodeNormalization as _;
        handle.trim().trim_start_matches('@').nfc().collect::<String>().to_lowercase()
    }

    /// MC-64: whether it sleeps.
    pub fn is_asleep(&self) -> bool {
        self.asleep == Some(true)
    }

    /// Whether this bot answers to `text` (`@name` or `name`).
    pub fn answers_to(&self, text: &str) -> bool {
        Self::handle_of(text).is_some_and(|name| Self::key(&name) == Self::key(&self.handle))
    }
}

/// A combining mark (a decomposed 한글 syllable's vowel or final, an accent): kept inside a name, never first.
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1160..=0x11FF | 0x302E..=0x302F | 0xD7B0..=0xD7FF)
}

impl Wire for PaneBot {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a bot")?;
        let Some(handle) = json::opt_string(o.get("handle"))?.and_then(|h| PaneBot::handle_of(&h)) else {
            return fail("a bot without a handle");
        };
        Ok(PaneBot {
            handle,
            instructions: lenient_string(o.get("instructions")),
            auto: lenient_bool(o.get("auto")),
            asleep: lenient_bool(o.get("asleep")).filter(|on| *on),
            budget_per_day: lenient_double(o.get("budgetPerDay")).filter(|b| *b > 0.0),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("handle", Some(&self.handle));
        out.prop_str("instructions", self.instructions.as_deref());
        out.prop_opt_bool("auto", self.auto);
        out.prop_opt_bool("asleep", self.asleep.filter(|on| *on));
        if let Some(budget) = self.budget_per_day {
            out.prop_float("budgetPerDay", budget);
        }
        out.end_object();
    }
}

/// MC-28: another agent's opinion on the pane's work (`/advisor`): who answered, with what model and cost, and what. Cards of one
/// committee share `group` and show side by side. Lenient: damage loads as None.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpinionCard {
    pub id: String,
    pub provider: String,
    pub model: Option<String>,
    pub cost_usd: Option<f64>,
    pub tokens: Option<i64>,
    /// The person's own question, when they asked one.
    pub question: Option<String>,
    pub body: String,
    /// `running`, `done` or `failed` (the body then says why).
    pub state: String,
    pub created_at: Option<String>,
    pub group: Option<String>,
}

/// MC-28: how many opinion cards a pane keeps (the newest), and how long each body may be.
pub const OPINION_CARDS_KEPT: usize = 6;
pub const OPINION_BODY_LIMIT: usize = 8_000;

impl OpinionCard {
    pub const RUNNING: &'static str = "running";
    pub const DONE: &'static str = "done";
    pub const FAILED: &'static str = "failed";
}

impl Wire for OpinionCard {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "an opinion card")?;
        let Some(id) = json::opt_string(o.get("id"))?.filter(|id| !id.is_empty()) else {
            return fail("an opinion card without its id");
        };
        Ok(OpinionCard {
            id,
            provider: lenient_string(o.get("provider")).unwrap_or_default(),
            model: lenient_string(o.get("model")),
            cost_usd: json::opt_float64(o.get("costUsd")).ok().flatten(),
            tokens: json::opt_int64(o.get("tokens")).ok().flatten(),
            question: lenient_string(o.get("question")),
            body: lenient_string(o.get("body")).unwrap_or_default(),
            state: lenient_string(o.get("state")).unwrap_or_else(|| OpinionCard::DONE.into()),
            created_at: lenient_string(o.get("createdAt")),
            group: lenient_string(o.get("group")),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("provider", Some(&self.provider));
        out.prop_str("model", self.model.as_deref());
        out.prop_opt_float("costUsd", self.cost_usd);
        out.prop_opt_int("tokens", self.tokens);
        out.prop_str("question", self.question.as_deref());
        out.prop_str("body", Some(&self.body));
        out.prop_str("state", Some(&self.state));
        out.prop_str("createdAt", self.created_at.as_deref());
        out.prop_str("group", self.group.as_deref());
        out.end_object();
    }
}

/// MC-29 (비교 실행): one request run by several agents (or one agent several times) side by side, each in its own delegated child
/// worktree from the same base commit, kept on the pane that started it so the compare view opens again after a restart.
/// Lenient: damage loads as None.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompareRun {
    /// The group id (`compareId`).
    pub id: String,
    /// The request every child got (at most [`COMPARE_REQUEST_LIMIT`] characters kept).
    pub request: String,
    /// The commit every child started from, and the branch it was on.
    pub base_commit: String,
    pub base_branch: Option<String>,
    /// The test chosen to run in each child once it reports: its task id (`run:<name>` or `mighty:<name>`) and name.
    pub test: Option<String>,
    pub test_name: Option<String>,
    pub created_at: Option<String>,
    pub children: Vec<CompareChild>,
}

/// MC-29: one child of a compare run, in the order the rows were picked.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompareChild {
    /// The delegated child's session id (its branch is `mighty/<id>`); for a row that never made a child, an id of its own.
    pub id: String,
    pub provider: String,
    pub model: Option<String>,
    /// Why the child could not be started (refused, or its worktree, pane or first run failed); None while it is fine.
    pub failure: Option<String>,
    pub started_at: Option<String>,
    /// When it was first seen done (reported, ended or failed), for its elapsed time.
    pub finished_at: Option<String>,
    /// What its runs cost, summed from the usage ledger's records of its pane.
    pub cost_usd: Option<f64>,
    /// Its port block (`MIGHTY_PORT`, `MIGHTY_PORT_COUNT`).
    pub port_base: Option<i32>,
    pub port_count: Option<i32>,
    /// The chosen test: `running`, `passed` or `failed`; None before it ran.
    pub test_state: Option<String>,
    pub test_exit_code: Option<i32>,
    /// The end of the test's output (at most [`COMPARE_TEST_OUTPUT_LIMIT`] characters).
    pub test_output: Option<String>,
    /// MC-52: what became of an SSH workspace's child (a pane worktree on its machine, not a delegated child): `merged` or
    /// `discarded`, kept here as delegation keeps a local child's state. None while it is open, and always for a local child.
    pub outcome: Option<String>,
}

/// MC-29: a delegated child's link to its compare run, kept on the child's pane: the group and parent, and its port block (its runs
/// get `PORT`, `MIGHTY_PORT`, `MIGHTY_PORT_COUNT` and the worktree's names, as a pane worktree's do).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompareMember {
    pub group: String,
    pub parent_id: String,
    pub port_base: Option<i32>,
    pub port_count: Option<i32>,
}

/// MC-29: the most rows a compare run takes, the groups a pane keeps (the newest), and how much of the request and a test's output.
pub const COMPARE_MAXIMUM_ROWS: usize = 4;
pub const COMPARE_RUNS_KEPT: usize = 5;
pub const COMPARE_REQUEST_LIMIT: usize = 4_000;
pub const COMPARE_TEST_OUTPUT_LIMIT: usize = 2_000;

impl CompareChild {
    pub const TEST_RUNNING: &'static str = "running";
    pub const TEST_PASSED: &'static str = "passed";
    pub const TEST_FAILED: &'static str = "failed";
    /// MC-52: [`CompareChild::outcome`]'s words.
    pub const MERGED: &'static str = "merged";
    pub const DISCARDED: &'static str = "discarded";
}

impl CompareMember {
    /// The values every run of the child gets: `PORT` (the block's first port) with the block and the worktree's names
    /// ([`worktree_variables`]); `folder` is the child's working folder, `root` the workspace folder. None without a block.
    pub fn environment(&self, folder: &str, root: &str) -> Option<Vec<(String, String)>> {
        let (base, count) = match (self.port_base, self.port_count) {
            (Some(base), Some(count)) if base > 0 && count > 0 => (base, count),
            _ => return None,
        };
        let trimmed = folder.trim_end_matches(['/', '\\']);
        let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
        let mut values = vec![("PORT".to_string(), base.to_string())];
        values.extend(worktree_variables(Some((base, count)), name, root, folder));
        Some(values)
    }
}

impl Wire for CompareRun {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a compare run")?;
        let Some(id) = json::opt_string(o.get("id"))?.filter(|id| !id.is_empty()) else {
            return fail("a compare run without its id");
        };
        Ok(CompareRun {
            id,
            request: lenient_string(o.get("request")).unwrap_or_default(),
            base_commit: lenient_string(o.get("baseCommit")).unwrap_or_default(),
            base_branch: lenient_string(o.get("baseBranch")),
            test: lenient_string(o.get("test")),
            test_name: lenient_string(o.get("testName")),
            created_at: lenient_string(o.get("createdAt")),
            children: lenient_list(o.get("children")).unwrap_or_default(),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("request", Some(&self.request));
        out.prop_str("baseCommit", Some(&self.base_commit));
        out.prop_str("baseBranch", self.base_branch.as_deref());
        out.prop_str("test", self.test.as_deref());
        out.prop_str("testName", self.test_name.as_deref());
        out.prop_str("createdAt", self.created_at.as_deref());
        out.prop_list("children", Some(&self.children));
        out.end_object();
    }
}

impl Wire for CompareChild {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a compare child")?;
        let Some(id) = json::opt_string(o.get("id"))?.filter(|id| !id.is_empty()) else {
            return fail("a compare child without its id");
        };
        Ok(CompareChild {
            id,
            provider: lenient_string(o.get("provider")).unwrap_or_default(),
            model: lenient_string(o.get("model")),
            failure: lenient_string(o.get("failure")),
            started_at: lenient_string(o.get("startedAt")),
            finished_at: lenient_string(o.get("finishedAt")),
            cost_usd: lenient_double(o.get("costUsd")),
            port_base: json::opt_int32(o.get("portBase")).ok().flatten(),
            port_count: json::opt_int32(o.get("portCount")).ok().flatten(),
            test_state: lenient_string(o.get("testState")),
            test_exit_code: json::opt_int32(o.get("testExitCode")).ok().flatten(),
            test_output: lenient_string(o.get("testOutput")),
            outcome: lenient_string(o.get("outcome")),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("provider", Some(&self.provider));
        out.prop_str("model", self.model.as_deref());
        out.prop_str("failure", self.failure.as_deref());
        out.prop_str("startedAt", self.started_at.as_deref());
        out.prop_str("finishedAt", self.finished_at.as_deref());
        out.prop_opt_float("costUsd", self.cost_usd);
        out.prop_opt_int("portBase", self.port_base.map(i64::from));
        out.prop_opt_int("portCount", self.port_count.map(i64::from));
        out.prop_str("testState", self.test_state.as_deref());
        out.prop_opt_int("testExitCode", self.test_exit_code.map(i64::from));
        out.prop_str("testOutput", self.test_output.as_deref());
        out.prop_str("outcome", self.outcome.as_deref());
        out.end_object();
    }
}

impl Wire for CompareMember {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a compare member")?;
        let (Some(group), Some(parent_id)) =
            (json::opt_string(o.get("group"))?.filter(|g| !g.is_empty()), json::opt_string(o.get("parentId"))?.filter(|p| !p.is_empty()))
        else {
            return fail("a compare member without its group");
        };
        Ok(CompareMember {
            group,
            parent_id,
            port_base: json::opt_int32(o.get("portBase")).ok().flatten(),
            port_count: json::opt_int32(o.get("portCount")).ok().flatten(),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("group", Some(&self.group));
        out.prop_str("parentId", Some(&self.parent_id));
        out.prop_opt_int("portBase", self.port_base.map(i64::from));
        out.prop_opt_int("portCount", self.port_count.map(i64::from));
        out.end_object();
    }
}

/// C# `KeptPaneWorktree`: a pane worktree kept after its pane closed with work still unmerged (or children still on its branch).
#[derive(Debug, Clone, PartialEq)]
pub struct KeptPaneWorktree {
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub provider: Option<String>,
    pub worktree: Option<PaneWorktree>,
    pub kept_at: Option<String>,
}

impl Default for KeptPaneWorktree {
    /// `new KeptPaneWorktree()`: the time now, Claude, an empty worktree.
    fn default() -> Self {
        KeptPaneWorktree {
            session_id: Some(String::new()),
            title: Some(String::new()),
            provider: Some("claude".into()),
            worktree: Some(PaneWorktree::default()),
            kept_at: Some(time::now_o()),
        }
    }
}

impl KeptPaneWorktree {
    pub fn session_id(&self) -> &str {
        self.session_id.as_deref().unwrap_or("")
    }
    pub fn title(&self) -> &str {
        self.title.as_deref().unwrap_or("")
    }
    pub fn provider(&self) -> &str {
        self.provider.as_deref().unwrap_or("")
    }
}

impl Wire for KeptPaneWorktree {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a kept pane worktree")?;
        Ok(KeptPaneWorktree {
            session_id: json::string(o.get("sessionId"), some(""))?,
            title: json::string(o.get("title"), some(""))?,
            provider: json::string(o.get("provider"), some("claude"))?,
            worktree: match o.get("worktree") {
                None => Some(PaneWorktree::default()),
                value => json::record(value)?,
            },
            kept_at: json::string(o.get("keptAt"), || Some(time::now_o()))?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("sessionId", self.session_id.as_deref());
        out.prop_str("title", self.title.as_deref());
        out.prop_str("provider", self.provider.as_deref());
        out.prop("worktree", self.worktree.as_ref());
        out.prop_str("keptAt", self.kept_at.as_deref());
        out.end_object();
    }
}

impl Workspace {
    pub fn id(&self) -> &str {
        self.id.as_deref().unwrap_or("")
    }
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or("")
    }
    pub fn path(&self) -> &str {
        self.path.as_deref().unwrap_or("")
    }
    pub fn remote(&self) -> Option<&RemoteWorkspaceReference> {
        self.remote.as_ref()
    }
}

/// C# `RemoteWorkspaceReference(ConnectionId, WorkspaceId, HostName) { Version = 2 }`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteWorkspaceReference {
    pub connection_id: Option<String>,
    pub workspace_id: Option<String>,
    pub host_name: Option<String>,
    pub version: i32,
    /// `[JsonIgnore] Mirror`: a workspace the remote mirror shows (docs/remote-workspaces.md "그 컴퓨터의 창 그대로"): its panes are the
    /// host's own, read from its list. Never read or written: such a workspace lives only while the app runs
    /// ([`AppSnapshot::without_mirrors`]).
    pub mirror: bool,
}

impl Wire for RemoteWorkspaceReference {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a remote reference")?;
        Ok(RemoteWorkspaceReference {
            connection_id: json::opt_string(o.get("connectionId"))?,
            workspace_id: json::opt_string(o.get("workspaceId"))?,
            host_name: json::opt_string(o.get("hostName"))?,
            version: json::int32(o.get("version"), 2)?,
            mirror: false,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("connectionId", self.connection_id.as_deref());
        out.prop_str("workspaceId", self.workspace_id.as_deref());
        out.prop_str("hostName", self.host_name.as_deref());
        out.prop_int("version", self.version as i64);
        out.end_object();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelDefaultsConfig {
    pub claude: Option<ProviderModeDefaults>,
    pub codex: Option<ProviderModeDefaults>,
}

impl Wire for ModelDefaultsConfig {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "model defaults")?;
        let side = |name: &str| match o.get(name) {
            None => Ok(Some(ProviderModeDefaults::default())),
            value => json::record(value),
        };
        Ok(ModelDefaultsConfig { claude: side("claude")?, codex: side("codex")? })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop("claude", self.claude.as_ref());
        out.prop("codex", self.codex.as_ref());
        out.end_object();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderModeDefaults {
    pub mode_defaults: Option<StringMap>,
    /// Never normalized: a null element stays and is written back as null.
    pub registered_models: Option<Vec<Option<RegisteredModelEntry>>>,
}

impl Default for ProviderModeDefaults {
    fn default() -> Self {
        ProviderModeDefaults { mode_defaults: Some(IndexMap::new()), registered_models: Some(vec![]) }
    }
}

impl Wire for ProviderModeDefaults {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "provider defaults")?;
        Ok(ProviderModeDefaults {
            mode_defaults: match o.get("modeDefaults") {
                None => Some(IndexMap::new()),
                value => json::string_map(value)?,
            },
            registered_models: match o.get("registeredModels") {
                None => Some(vec![]),
                value => json::list(value, RegisteredModelEntry::read)?,
            },
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_string_map("modeDefaults", self.mode_defaults.as_ref());
        if let Some(list) = &self.registered_models {
            out.name("registeredModels");
            write_nullable_list(out, list);
        }
        out.end_object();
    }
}

/// A list whose null elements are written as null.
pub fn write_nullable_list<T: Wire>(out: &mut JsonOut, list: &[Option<T>]) {
    out.begin_array();
    for item in list {
        match item {
            Some(item) => item.write(out),
            None => out.null(),
        }
    }
    out.end_array();
}

pub fn write_nullable_strings(out: &mut JsonOut, list: &[Option<String>]) {
    out.begin_array();
    for item in list {
        match item {
            Some(item) => out.str(item),
            None => out.null(),
        }
    }
    out.end_array();
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredModelEntry {
    pub name: Option<String>,
    pub supports_effort: bool,
    pub supported_effort_levels: Option<Vec<Option<String>>>,
}

impl Wire for RegisteredModelEntry {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a registered model")?;
        Ok(RegisteredModelEntry {
            name: json::opt_string(o.get("name"))?,
            supports_effort: json::boolean(o.get("supportsEffort"), false)?,
            supported_effort_levels: json::string_list(o.get("supportedEffortLevels"))?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("name", self.name.as_deref());
        out.prop_bool("supportsEffort", self.supports_effort);
        if let Some(levels) = &self.supported_effort_levels {
            out.name("supportedEffortLevels");
            write_nullable_strings(out, levels);
        }
        out.end_object();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhaseModelsSnapshot {
    pub claude_main: Option<String>,
    pub claude_opus_alias: Option<String>,
    pub claude_sonnet_alias: Option<String>,
    pub claude_haiku_alias: Option<String>,
    pub claude_subagent_default: Option<String>,
    pub codex_review_model: Option<String>,
    pub codex_subagent_default: Option<String>,
    pub codex_plan_mode_reasoning_effort: Option<String>,
    pub claude_main_effort: Option<String>,
    pub codex_main_effort: Option<String>,
    pub codex_subagent_effort: Option<String>,
}

impl Wire for PhaseModelsSnapshot {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "phase models")?;
        let model = |name: &str| json::string(o.get(name), some("default"));
        Ok(PhaseModelsSnapshot {
            claude_main: model("claudeMain")?,
            claude_opus_alias: model("claudeOpusAlias")?,
            claude_sonnet_alias: model("claudeSonnetAlias")?,
            claude_haiku_alias: model("claudeHaikuAlias")?,
            claude_subagent_default: model("claudeSubagentDefault")?,
            codex_review_model: model("codexReviewModel")?,
            codex_subagent_default: model("codexSubagentDefault")?,
            codex_plan_mode_reasoning_effort: model("codexPlanModeReasoningEffort")?,
            claude_main_effort: json::opt_string(o.get("claudeMainEffort"))?,
            codex_main_effort: json::opt_string(o.get("codexMainEffort"))?,
            codex_subagent_effort: json::opt_string(o.get("codexSubagentEffort"))?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("claudeMain", self.claude_main.as_deref());
        out.prop_str("claudeOpusAlias", self.claude_opus_alias.as_deref());
        out.prop_str("claudeSonnetAlias", self.claude_sonnet_alias.as_deref());
        out.prop_str("claudeHaikuAlias", self.claude_haiku_alias.as_deref());
        out.prop_str("claudeSubagentDefault", self.claude_subagent_default.as_deref());
        out.prop_str("codexReviewModel", self.codex_review_model.as_deref());
        out.prop_str("codexSubagentDefault", self.codex_subagent_default.as_deref());
        out.prop_str("codexPlanModeReasoningEffort", self.codex_plan_mode_reasoning_effort.as_deref());
        out.prop_str("claudeMainEffort", self.claude_main_effort.as_deref());
        out.prop_str("codexMainEffort", self.codex_main_effort.as_deref());
        out.prop_str("codexSubagentEffort", self.codex_subagent_effort.as_deref());
        out.end_object();
    }
}

/// C# `MobileRemoteSettings(Enabled = false, RelayURL = "", AllowLegacyPhones = true, DirectEnabled = true, DirectPort = 43139)`, but
/// the legacy apps off by default.
#[derive(Debug, Clone, PartialEq)]
pub struct MobileRemoteSettings {
    pub enabled: bool,
    pub relay_url: Option<String>,
    /// Apps without a device token may connect with the legacy key alone (Settings › 휴대폰 › 고급, with its warning). Off by default:
    /// read from `legacyPhones`, written there only when on, so the `allowLegacyPhones: true` every older version wrote (its default
    /// for everyone) does not turn it on.
    pub allow_legacy_phones: bool,
    /// `allowLegacyPhones` as C# and the older versions wrote it (their default true), written back as read so the file stays the one
    /// they write; it decides nothing any more ([`MobileRemoteSettings::allow_legacy_phones`] does). The switch sets both.
    pub legacy_phones_as_saved: bool,
    pub direct_enabled: bool,
    pub direct_port: i32,
    /// 휴대폰에서 볼 워크스페이스: false every workspace of this computer (the default), true only those 이 컴퓨터 공유 shares now.
    /// Saved as `phoneSharedOnly` only when on (C# and Swift never write it; anything but true reads as off).
    pub phone_shared_only: bool,
}

/// C# `MobileDirect.DefaultPort`.
pub const MOBILE_DIRECT_PORT: i32 = 43139;

impl Default for MobileRemoteSettings {
    fn default() -> Self {
        MobileRemoteSettings {
            enabled: false,
            relay_url: Some(String::new()),
            allow_legacy_phones: false,
            legacy_phones_as_saved: true,
            direct_enabled: true,
            direct_port: MOBILE_DIRECT_PORT,
            phone_shared_only: false,
        }
    }
}

impl Wire for MobileRemoteSettings {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "mobile remote settings")?;
        Ok(MobileRemoteSettings {
            enabled: json::boolean(o.get("enabled"), false)?,
            relay_url: json::string(o.get("relayURL"), some(""))?,
            allow_legacy_phones: o.get("legacyPhones").and_then(Value::as_bool).unwrap_or(false),
            legacy_phones_as_saved: json::boolean(o.get("allowLegacyPhones"), true)?,
            direct_enabled: json::boolean(o.get("directEnabled"), true)?,
            direct_port: json::int32(o.get("directPort"), MOBILE_DIRECT_PORT)?,
            phone_shared_only: o.get("phoneSharedOnly").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_bool("enabled", self.enabled);
        out.prop_str("relayURL", self.relay_url.as_deref());
        out.prop_bool("allowLegacyPhones", self.legacy_phones_as_saved);
        if self.allow_legacy_phones {
            out.prop_bool("legacyPhones", true);
        }
        out.prop_bool("directEnabled", self.direct_enabled);
        out.prop_int("directPort", self.direct_port as i64);
        if self.phone_shared_only {
            out.prop_bool("phoneSharedOnly", true);
        }
        out.end_object();
    }
}

// ── RunSession ──────────────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct RunSession {
    pub id: Option<String>,
    pub workspace_id: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub settings: Option<RunSettings>,
    pub logs: Option<Vec<LogEntry>>,
    pub resume_id: Option<String>,
    pub created_at: Option<String>,
    pub draft: Option<String>,
    pub run_timing: Option<AgentRunTiming>,
    pub session_usage: Option<SessionUsage>,
    /// `[JsonIgnore]`: the activity a running pane shows now, never saved.
    pub current_activity: Option<AgentActivity>,
    pub agent_view_mode: Option<String>,
    pub mighty_style: Option<String>,
    pub mighty_style_hash: Option<String>,
    pub mighty_style_since: Option<String>,
    pub graph_runs: Option<Vec<MightyGraphRun>>,
    pub graph_result_size: Option<GraphBlockSize>,
    pub graph_plan_size: Option<GraphBlockSize>,
    pub graph_block_sizes: Option<IndexMap<String, Option<GraphBlockSize>>>,
    /// Null when read without the field (a pane saved before the 실행 split); 1 on a pane made new (`GraphBlockPreferences.SplitLayout`).
    pub graph_sizes_layout: Option<i32>,
    pub graph_view_mode: Option<String>,
    /// Written only when true (the setter turns false into null).
    pub flow_log_collapsed: Option<bool>,
    pub workspace_profile_key: Option<String>,
    pub owner_session_id: Option<String>,
    pub title_mode: Option<String>,
    pub plan_history: Option<Vec<PlanRecord>>,
    pub todo_progress: Option<TodoProgress>,
    pub background_work: Option<BackgroundWork>,
    pub parent_session_id: Option<String>,
    pub working_folder: Option<String>,
    /// Present only on a workspace's agent pane that works in its own worktree (`PaneWorktree.cs`). Lenient: damage loads as None.
    pub pane_worktree: Option<PaneWorktree>,
    pub linked_task: Option<LinkedTask>,
    pub ask_target: Option<String>,
    /// MC-28: the pane this one was forked from (`forkedFrom`).
    pub forked_from: Option<ForkedFrom>,
    /// MC-58: the bot this agent pane is (`bot`).
    pub bot: Option<PaneBot>,
    /// MC-28: other agents' opinions on the pane's work (`/advisor`), the newest last.
    pub opinions: Option<Vec<OpinionCard>>,
    /// MC-29: the compare runs this pane started (`compareRuns`), the newest last.
    pub compare_runs: Option<Vec<CompareRun>>,
    /// MC-29: this delegated child's compare run (`compareMember`).
    pub compare_member: Option<CompareMember>,
    /// `shellRuns` on the wire.
    pub shell_block_runs: Option<Vec<ShellBlockRun>>,
    /// `[JsonIgnore] Mirror`: the host pane a remote mirror pane shows (C# `RemoteMirrorReference`, W9-J): never read, written or saved.
    pub mirror: Option<MirrorReference>,
    /// The pane's body ([`BODY_FIELDS`]) is in its file and not read yet: what is held of it is the summary's last entries
    /// ([`RunSession::body_digest`]). Never written: a pane read whole has it false.
    pub body_on_disk: bool,
    /// The summary the split state's index keeps of the pane's body, as read with the index: what lists read of a pane whose body is
    /// on disk. Never written as such: the index's summary is made from the body each save.
    pub stored_digest: Option<Arc<SessionDigest>>,
    /// Monotonic revision of the pane's split body file (`bodyRevision`), kept in memory across hot/cold transitions and never emitted
    /// by `Wire::write` or `write_page` (C# parity).
    pub body_revision: Option<u64>,
}

/// What lists need of a pane's body without the body (the phone's and the mirror's pane lists, the dashboard card, the review's kept
/// originals): its log's length, its last entries (each text cut short) and the ids of the requests its diagrams keep. The split
/// state's index keeps one per pane (`digest`), so a pane whose body is not read yet still shows and keeps what it holds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionDigest {
    pub log_count: i64,
    pub tail: Vec<LogEntry>,
    /// The diagrams' request ids and the ids they were made from (`FileOriginalStore.KeptRuns`).
    pub run_ids: Vec<String>,
}

/// How many of the log's last entries a summary keeps, and how much of each text.
pub const DIGEST_TAIL: usize = 8;
pub const DIGEST_TEXT: usize = 1000;

/// Clamps a transcript entry for inclusion in a [`SessionDigest`] (shortened text, stripped tool output and images).
pub fn clamp_for_digest(entry: &LogEntry) -> LogEntry {
    LogEntry {
        text: entry.text.as_deref().map(|t| crate::wire::clean(t, DIGEST_TEXT)),
        activity: entry.activity.clone().map(|a| AgentActivity { output: None, ..a }),
        images: None,
        ..entry.clone()
    }
}

impl SessionDigest {
    /// The summary of `pane`'s body as it is. Both the latest `"user"` request and the latest `"assistant"` reply come along in
    /// chronological order when older than `logs[from..]`, so cold panes still title themselves, infer run timing, and answer
    /// cross-workspace activity summaries without disk I/O.
    pub fn of(pane: &RunSession) -> SessionDigest {
        let logs = pane.logs();
        let from = logs.len().saturating_sub(DIGEST_TAIL);
        let latest_before = |kind: &str| -> Option<usize> {
            let in_tail_non_empty = logs[from..].iter().any(|e| e.kind() == kind && !e.text().trim().is_empty());
            if in_tail_non_empty {
                return None;
            }
            if let Some(at) = logs[..from].iter().rposition(|e| e.kind() == kind && !e.text().trim().is_empty()) {
                return Some(at);
            }
            (!logs[from..].iter().any(|e| e.kind() == kind)).then(|| logs[..from].iter().rposition(|e| e.kind() == kind)).flatten()
        };
        let mut earlier: Vec<usize> = [latest_before("user"), latest_before("assistant")].into_iter().flatten().collect();
        earlier.sort_unstable();
        earlier.dedup();
        let tail = earlier.into_iter().map(|at| &logs[at]).chain(logs[from..].iter()).map(clamp_for_digest).collect();
        SessionDigest { log_count: logs.len() as i64, tail, run_ids: kept_run_ids(pane.graph_runs()) }
    }

    pub fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_int("logCount", self.log_count);
        out.prop_list("tail", Some(&self.tail));
        out.prop_strings("runIds", Some(&self.run_ids));
        out.end_object();
    }

    /// The index's summary; none for one that does not read (the pane's body is then read when it is needed).
    pub fn read(value: &Value) -> Option<SessionDigest> {
        let object = value.as_object()?;
        Some(SessionDigest {
            log_count: object.get("logCount").and_then(Value::as_i64).unwrap_or(0).max(0),
            tail: object.get("tail").and_then(|t| json::skipping_list::<LogEntry>(Some(t)).ok().flatten()).unwrap_or_default(),
            run_ids: object
                .get("runIds")
                .and_then(Value::as_array)
                .map(|ids| ids.iter().filter_map(Value::as_str).filter(|id| crate::wire::identifier(id)).map(str::to_string).collect())
                .unwrap_or_default(),
        })
    }
}

/// The ids of the requests `runs` keep and of the runs they were made from.
pub fn kept_run_ids(runs: &[MightyGraphRun]) -> Vec<String> {
    let mut ids: Vec<String> = vec![];
    for id in runs.iter().flat_map(|r| [r.id.clone(), r.source_run_id.clone()]).flatten() {
        if crate::wire::identifier(&id) && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// C# `RemoteMirrorReference(ConnectionId, HostSessionId, HostWorkspaceId, HostName)`: what ties a client pane to the host pane it
/// mirrors. Never saved: mirrored panes live only while the app runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorReference {
    pub connection_id: String,
    pub host_session_id: String,
    pub host_workspace_id: String,
    pub host_name: String,
}

impl Default for RunSession {
    /// `new RunSession()`: a new id, the time now, the current graph size layout.
    fn default() -> Self {
        RunSession {
            id: Some(new_id()),
            workspace_id: Some(String::new()),
            title: Some("Claude".into()),
            kind: Some("claude".into()),
            status: Some("idle".into()),
            model: Some("default".into()),
            provider: Some("claude".into()),
            settings: Some(RunSettings::default()),
            logs: Some(vec![]),
            resume_id: None,
            created_at: Some(time::now_o()),
            draft: Some(String::new()),
            run_timing: None,
            session_usage: None,
            current_activity: None,
            agent_view_mode: None,
            mighty_style: None,
            mighty_style_hash: None,
            mighty_style_since: None,
            graph_runs: None,
            graph_result_size: None,
            graph_plan_size: None,
            graph_block_sizes: None,
            graph_sizes_layout: Some(crate::graph::SPLIT_LAYOUT),
            graph_view_mode: None,
            flow_log_collapsed: None,
            workspace_profile_key: None,
            owner_session_id: None,
            title_mode: None,
            plan_history: None,
            todo_progress: None,
            background_work: None,
            parent_session_id: None,
            working_folder: None,
            pane_worktree: None,
            linked_task: None,
            ask_target: None,
            forked_from: None,
            bot: None,
            opinions: None,
            compare_runs: None,
            compare_member: None,
            shell_block_runs: None,
            mirror: None,
            body_on_disk: false,
            stored_digest: None,
            body_revision: None,
        }
    }
}

/// `MightyTimeline.NormalizeMode`.
pub fn graph_view_mode(value: Option<&str>) -> Option<String> {
    value.filter(|v| matches!(*v, "diagram" | "timeline" | "structure")).map(str::to_string)
}

/// `LenientListConverter<T>.Read`: an array whose elements that fail to read (or are null) are left out; anything else is null.
fn lenient_list<T: Wire>(value: Option<&Value>) -> Option<Vec<T>> {
    match value {
        Some(Value::Array(items)) => Some(items.iter().filter(|item| !item.is_null()).filter_map(|item| T::read(item).ok()).collect()),
        _ => None,
    }
}

/// `LenientJsonConverter<T>.Read`: the record, or null when it fails to read.
fn lenient_record<T: Wire>(value: Option<&Value>) -> Option<T> {
    value.filter(|v| !v.is_null()).and_then(|v| T::read(v).ok())
}

impl Wire for RunSession {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a pane")?;
        Ok(RunSession {
            id: json::string(o.get("id"), || Some(new_id()))?,
            workspace_id: json::string(o.get("workspaceId"), some(""))?,
            title: json::string(o.get("title"), some("Claude"))?,
            kind: json::string(o.get("kind"), some("claude"))?,
            status: json::string(o.get("status"), some("idle"))?,
            model: json::string(o.get("model"), some("default"))?,
            provider: json::string(o.get("provider"), some("claude"))?,
            settings: match o.get("settings") {
                None => Some(RunSettings::default()),
                value => json::record(value)?,
            },
            logs: match o.get("logs") {
                None => Some(vec![]),
                // A null entry is never read: Normalize leaves it out first.
                value => json::skipping_list(value)?,
            },
            resume_id: json::opt_string(o.get("resumeId"))?,
            created_at: json::string(o.get("createdAt"), || Some(time::now_o()))?,
            draft: json::string(o.get("draft"), some(""))?,
            run_timing: match o.get("runTiming") {
                None | Some(Value::Null) => None,
                Some(v) => AgentRunTiming::from_json(v),
            },
            session_usage: match o.get("sessionUsage") {
                None | Some(Value::Null) => None,
                Some(v) => SessionUsage::from_json(v),
            },
            current_activity: None,
            agent_view_mode: o
                .get("agentViewMode")
                .and_then(Value::as_str)
                .filter(|v| matches!(*v, "default" | "mighty"))
                .map(str::to_string),
            mighty_style: json::opt_string(o.get("mightyStyle"))?,
            mighty_style_hash: json::opt_string(o.get("mightyStyleHash"))?,
            mighty_style_since: json::opt_string(o.get("mightyStyleSince"))?,
            graph_runs: json::strict_list(o.get("graphRuns"), "graph run")?,
            graph_result_size: json::record(o.get("graphResultSize"))?,
            graph_plan_size: json::record(o.get("graphPlanSize"))?,
            graph_block_sizes: json::dictionary(o.get("graphBlockSizes"), GraphBlockSize::read)?,
            // IJsonOnDeserializing clears it first: absent reads as null; GraphSizesLayoutConverter takes a whole number only.
            graph_sizes_layout: o.get("graphSizesLayout").and_then(json::as_integer).and_then(|n| i32::try_from(n).ok()),
            graph_view_mode: graph_view_mode(o.get("graphViewMode").and_then(Value::as_str)),
            flow_log_collapsed: (lenient_bool(o.get("flowLogCollapsed")) == Some(true)).then_some(true),
            workspace_profile_key: json::opt_string(o.get("workspaceProfileKey"))?,
            owner_session_id: json::opt_string(o.get("ownerSessionId"))?,
            title_mode: json::opt_string(o.get("titleMode"))?,
            plan_history: lenient_list(o.get("planHistory")),
            todo_progress: lenient_record(o.get("todoProgress")),
            background_work: lenient_record(o.get("backgroundWork")),
            parent_session_id: lenient_string(o.get("parentSessionId")),
            working_folder: lenient_string(o.get("workingFolder")),
            pane_worktree: lenient_record(o.get("paneWorktree")),
            linked_task: lenient_record(o.get("linkedTask")),
            ask_target: lenient_string(o.get("askTarget")),
            forked_from: lenient_record(o.get("forkedFrom")),
            bot: lenient_record(o.get("bot")),
            opinions: lenient_list(o.get("opinions")),
            compare_runs: lenient_list(o.get("compareRuns")),
            compare_member: lenient_record(o.get("compareMember")),
            shell_block_runs: lenient_list(o.get("shellRuns")),
            mirror: None,
            body_on_disk: false,
            stored_digest: None,
            body_revision: None,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        self.write_with(out, true, true);
    }
}

/// The split state's index names itself so (`"sessionFiles": 1`): its panes' bodies are in `sessions/<id>.json`.
pub const SPLIT_MARKER: &str = "sessionFiles";

/// How a state write holds its panes.
enum Sessions<'a> {
    None,
    Whole { output_files: bool },
    Index(&'a dyn Fn(&str) -> u64),
}

/// Which of a pane's fields one write holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneFields {
    /// Every field (the state as one file, the page).
    All,
    /// Every field but `logs` and `graphRuns` (the delta's fields).
    WithoutLists,
    /// The pane's entry in the split state's index: every field but the body's ([`BODY_FIELDS`]).
    Index,
    /// The pane's body file: only [`BODY_FIELDS`].
    Body,
}

impl PaneFields {
    fn body(self) -> bool {
        self != PaneFields::Index
    }
    fn lists(self) -> bool {
        matches!(self, PaneFields::All | PaneFields::Body)
    }
}

/// The fields a pane's body file holds in the split state (`sessions/<id>.json`): the ones that grow with its requests. Everything
/// else, the draft included, stays in the index.
pub const BODY_FIELDS: [&str; 6] = ["logs", "graphRuns", "planHistory", "todoProgress", "backgroundWork", "shellRuns"];

impl RunSession {
    /// Written with or without its two lists (the delta's fields leave `logs` and `graphRuns` out), and with or without the graph
    /// agents' output files (`WebDelta.PageRuns`). Never emits `bodyRevision` or `digest` (C# page parity).
    pub fn write_with(&self, out: &mut JsonOut, lists: bool, output_files: bool) {
        self.write_fields(out, if lists { PaneFields::All } else { PaneFields::WithoutLists }, output_files, None);
    }

    /// Writes the pane for the web page (`for_page == true`), omitting `outputFile`, `bodyRevision`, and `digest` for C# parity.
    pub fn write_page(&self, out: &mut JsonOut) {
        self.write_with(out, true, false);
    }

    /// The pane's entry in the split state's index: its fields but the body's, and the revision of the body file written for it.
    pub fn write_index(&self, out: &mut JsonOut, body_revision: u64) {
        self.write_fields(out, PaneFields::Index, true, Some(body_revision));
    }

    /// The pane's body file: `{version, sessionId, bodyRevision}` and its [`BODY_FIELDS`].
    pub fn write_body(&self, out: &mut JsonOut, body_revision: u64) {
        self.write_fields(out, PaneFields::Body, true, Some(body_revision));
    }

    fn write_fields(&self, out: &mut JsonOut, fields: PaneFields, output_files: bool, body_revision: Option<u64>) {
        out.begin_object();
        if fields == PaneFields::Body {
            out.prop_int("version", 1);
            out.prop_str("sessionId", self.id.as_deref());
            out.prop_int("bodyRevision", body_revision.unwrap_or(0) as i64);
            self.write_body_fields(out, fields, output_files);
            out.end_object();
            return;
        }
        out.prop_str("id", self.id.as_deref());
        out.prop_str("workspaceId", self.workspace_id.as_deref());
        out.prop_str("title", self.title.as_deref());
        out.prop_str("kind", self.kind.as_deref());
        out.prop_str("status", self.status.as_deref());
        out.prop_str("model", self.model.as_deref());
        out.prop_str("provider", self.provider.as_deref());
        out.prop("settings", self.settings.as_ref());
        if fields == PaneFields::All {
            out.prop_list("logs", self.logs.as_ref());
        }
        out.prop_str("resumeId", self.resume_id.as_deref());
        out.prop_str("createdAt", self.created_at.as_deref());
        out.prop_str("draft", self.draft.as_deref());
        out.prop("runTiming", self.run_timing.as_ref());
        out.prop("sessionUsage", self.session_usage.as_ref());
        if let Some(mode) = &self.agent_view_mode {
            // AgentViewModeConverter writes anything but its two words as null.
            out.name("agentViewMode");
            if matches!(mode.as_str(), "default" | "mighty") { out.str(mode) } else { out.null() }
        }
        out.prop_str("mightyStyle", self.mighty_style.as_deref());
        out.prop_str("mightyStyleHash", self.mighty_style_hash.as_deref());
        out.prop_str("mightyStyleSince", self.mighty_style_since.as_deref());
        if fields == PaneFields::All
            && let Some(runs) = &self.graph_runs
        {
            out.name("graphRuns");
            out.begin_array();
            for run in runs {
                run.write_with(out, output_files);
            }
            out.end_array();
        }
        out.prop("graphResultSize", self.graph_result_size.as_ref());
        out.prop("graphPlanSize", self.graph_plan_size.as_ref());
        if let Some(sizes) = &self.graph_block_sizes {
            out.name("graphBlockSizes");
            out.begin_object();
            for (key, size) in sizes {
                out.name(key);
                match size {
                    Some(size) => size.write(out),
                    None => out.null(),
                }
            }
            out.end_object();
        }
        out.prop_opt_int("graphSizesLayout", self.graph_sizes_layout.map(i64::from));
        if let Some(mode) = &self.graph_view_mode {
            out.name("graphViewMode");
            match graph_view_mode(Some(mode)) {
                Some(mode) => out.str(&mode),
                None => out.null(),
            }
        }
        out.prop_opt_bool("flowLogCollapsed", self.flow_log_collapsed);
        out.prop_str("workspaceProfileKey", self.workspace_profile_key.as_deref());
        out.prop_str("ownerSessionId", self.owner_session_id.as_deref());
        out.prop_str("titleMode", self.title_mode.as_deref());
        if fields.body() {
            out.prop_list("planHistory", self.plan_history.as_ref());
            out.prop("todoProgress", self.todo_progress.as_ref());
            out.prop("backgroundWork", self.background_work.as_ref());
        }
        out.prop_str("parentSessionId", self.parent_session_id.as_deref());
        out.prop_str("workingFolder", self.working_folder.as_deref());
        out.prop("paneWorktree", self.pane_worktree.as_ref());
        out.prop("linkedTask", self.linked_task.as_ref());
        out.prop_str("askTarget", self.ask_target.as_deref());
        out.prop("forkedFrom", self.forked_from.as_ref());
        out.prop("bot", self.bot.as_ref());
        out.prop_list("opinions", self.opinions.as_ref());
        out.prop_list("compareRuns", self.compare_runs.as_ref());
        out.prop("compareMember", self.compare_member.as_ref());
        if fields.body() {
            out.prop_list("shellRuns", self.shell_block_runs.as_ref());
        }
        if let (PaneFields::Index, Some(revision)) = (fields, body_revision) {
            out.prop_int("bodyRevision", revision as i64);
            out.name("digest");
            self.body_digest().write(out);
        }
        out.end_object();
    }

    /// The body's fields in the order the whole pane writes them.
    fn write_body_fields(&self, out: &mut JsonOut, fields: PaneFields, output_files: bool) {
        if fields.lists() {
            out.prop_list("logs", self.logs.as_ref());
            if let Some(runs) = &self.graph_runs {
                out.name("graphRuns");
                out.begin_array();
                for run in runs {
                    run.write_with(out, output_files);
                }
                out.end_array();
            }
        }
        out.prop_list("planHistory", self.plan_history.as_ref());
        out.prop("todoProgress", self.todo_progress.as_ref());
        out.prop("backgroundWork", self.background_work.as_ref());
        out.prop_list("shellRuns", self.shell_block_runs.as_ref());
    }

    pub fn id(&self) -> &str {
        self.id.as_deref().unwrap_or("")
    }
    pub fn workspace_id(&self) -> &str {
        self.workspace_id.as_deref().unwrap_or("")
    }
    pub fn title(&self) -> &str {
        self.title.as_deref().unwrap_or("")
    }
    pub fn kind(&self) -> &str {
        self.kind.as_deref().unwrap_or("")
    }
    pub fn status(&self) -> &str {
        self.status.as_deref().unwrap_or("")
    }
    pub fn model(&self) -> &str {
        self.model.as_deref().unwrap_or("")
    }
    pub fn provider(&self) -> &str {
        self.provider.as_deref().unwrap_or("")
    }
    pub fn created_at(&self) -> Option<&str> {
        self.created_at.as_deref()
    }
    pub fn started_at(&self) -> Option<DateTimeOffset> {
        self.run_timing.filter(AgentRunTiming::is_valid).map(|t| t.started_at)
    }
    pub fn logs(&self) -> &[LogEntry] {
        self.logs.as_deref().unwrap_or(&[])
    }
    pub fn graph_runs(&self) -> &[MightyGraphRun] {
        self.graph_runs.as_deref().unwrap_or(&[])
    }

    /// Builds a cold (`body_on_disk: true`) copy of this pane, replacing the full body fields with `SessionDigest`'s tail while
    /// preserving `body_revision`, `graph_block_sizes`, and all other non-body fields.
    pub fn evicted_to_disk(&self) -> RunSession {
        let digest = match (&self.stored_digest, self.body_on_disk) {
            (Some(stored), true) => (**stored).clone(),
            _ => SessionDigest::of(self),
        };
        RunSession {
            logs: Some(digest.tail.clone()),
            graph_runs: None,
            plan_history: None,
            todo_progress: None,
            background_work: None,
            shell_block_runs: None,
            body_on_disk: true,
            stored_digest: Some(Arc::new(digest)),
            ..self.clone()
        }
    }

    /// The summary of the pane's body: the saved one while the body is on disk, else made from the body.
    pub fn body_digest(&self) -> SessionDigest {
        match (&self.stored_digest, self.body_on_disk) {
            (Some(digest), true) => (**digest).clone(),
            _ => SessionDigest::of(self),
        }
    }

    /// The ids of the requests the pane's diagrams keep and of the runs they were made from, read from the summary while the body is
    /// on disk (what the review's kept originals hold on to).
    pub fn kept_run_ids(&self) -> Vec<String> {
        match (&self.stored_digest, self.body_on_disk) {
            (Some(digest), true) => digest.run_ids.clone(),
            _ => kept_run_ids(self.graph_runs()),
        }
    }

    /// How many entries the pane's log holds, its summary's count while the body is on disk.
    pub fn log_count(&self) -> usize {
        match (&self.stored_digest, self.body_on_disk) {
            (Some(digest), true) => digest.log_count.max(0) as usize,
            _ => self.logs().len(),
        }
    }

    /// Whether every non-list field written by `write_with(..., false, false)` is identical (ordered maps and bit-exact floats),
    /// guaranteeing byte-identical JSON field maps when `true`.
    pub fn same_non_list_fields(&self, other: &RunSession) -> bool {
        let RunSession {
            id,
            workspace_id,
            title,
            kind,
            status,
            model,
            provider,
            settings,
            logs: _,
            resume_id,
            created_at,
            draft,
            run_timing,
            session_usage,
            current_activity: _,
            agent_view_mode,
            mighty_style,
            mighty_style_hash,
            mighty_style_since,
            graph_runs: _,
            graph_result_size,
            graph_plan_size,
            graph_block_sizes,
            graph_sizes_layout,
            graph_view_mode,
            flow_log_collapsed,
            workspace_profile_key,
            owner_session_id,
            title_mode,
            plan_history,
            todo_progress,
            background_work,
            parent_session_id,
            working_folder,
            pane_worktree,
            linked_task,
            ask_target,
            forked_from,
            bot,
            opinions,
            compare_runs,
            compare_member,
            shell_block_runs,
            mirror: _,
            body_on_disk: _,
            stored_digest: _,
            body_revision: _,
        } = self;
        id == &other.id
            && workspace_id == &other.workspace_id
            && title == &other.title
            && kind == &other.kind
            && status == &other.status
            && model == &other.model
            && provider == &other.provider
            && resume_id == &other.resume_id
            && created_at == &other.created_at
            && draft == &other.draft
            && agent_view_mode == &other.agent_view_mode
            && mighty_style == &other.mighty_style
            && mighty_style_hash == &other.mighty_style_hash
            && mighty_style_since == &other.mighty_style_since
            && graph_sizes_layout == &other.graph_sizes_layout
            && graph_view_mode == &other.graph_view_mode
            && flow_log_collapsed == &other.flow_log_collapsed
            && workspace_profile_key == &other.workspace_profile_key
            && owner_session_id == &other.owner_session_id
            && title_mode == &other.title_mode
            && parent_session_id == &other.parent_session_id
            && working_folder == &other.working_folder
            && ask_target == &other.ask_target
            && run_timing == &other.run_timing
            && same_opt(settings.as_ref(), other.settings.as_ref(), same_run_settings)
            && same_opt(session_usage.as_ref(), other.session_usage.as_ref(), same_session_usage)
            && same_opt(graph_result_size.as_ref(), other.graph_result_size.as_ref(), same_graph_block_size)
            && same_opt(graph_plan_size.as_ref(), other.graph_plan_size.as_ref(), same_graph_block_size)
            && same_ordered_map(graph_block_sizes.as_ref(), other.graph_block_sizes.as_ref(), |a, b| {
                same_opt(a.as_ref(), b.as_ref(), same_graph_block_size)
            })
            && plan_history == &other.plan_history
            && todo_progress == &other.todo_progress
            && background_work == &other.background_work
            && pane_worktree == &other.pane_worktree
            && linked_task == &other.linked_task
            && forked_from == &other.forked_from
            && bot == &other.bot
            && opinions == &other.opinions
            && compare_runs == &other.compare_runs
            && compare_member == &other.compare_member
            && shell_block_runs == &other.shell_block_runs
    }
}

fn same_opt_f64(a: Option<f64>, b: Option<f64>) -> bool {
    a.map(f64::to_bits) == b.map(f64::to_bits)
}

fn same_opt<T>(a: Option<&T>, b: Option<&T>, eq: impl Fn(&T, &T) -> bool) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => eq(x, y),
        _ => false,
    }
}

fn same_ordered_map<K: Eq, V>(a: Option<&IndexMap<K, V>>, b: Option<&IndexMap<K, V>>, eq: impl Fn(&V, &V) -> bool) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|((k1, v1), (k2, v2))| k1 == k2 && eq(v1, v2)),
        _ => false,
    }
}

fn same_layout_node(a: &PaneLayoutNode, b: &PaneLayoutNode) -> bool {
    let PaneLayoutNode { id, kind, session_ids, selected_session_id, axis, ratio, children } = a;
    id == &b.id
        && kind == &b.kind
        && session_ids == &b.session_ids
        && selected_session_id == &b.selected_session_id
        && axis == &b.axis
        && ratio.to_bits() == b.ratio.to_bits()
        && match (children, &b.children) {
            (None, None) => true,
            (Some(x), Some(y)) => {
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same_opt(p.as_ref(), q.as_ref(), same_layout_node))
            }
            _ => false,
        }
}

fn same_provider_mode_defaults(a: &ProviderModeDefaults, b: &ProviderModeDefaults) -> bool {
    let ProviderModeDefaults { mode_defaults, registered_models } = a;
    same_ordered_map(mode_defaults.as_ref(), b.mode_defaults.as_ref(), PartialEq::eq) && registered_models == &b.registered_models
}

fn same_model_defaults(a: &ModelDefaultsConfig, b: &ModelDefaultsConfig) -> bool {
    let ModelDefaultsConfig { claude, codex } = a;
    same_opt(claude.as_ref(), b.claude.as_ref(), same_provider_mode_defaults)
        && same_opt(codex.as_ref(), b.codex.as_ref(), same_provider_mode_defaults)
}

fn same_graph_block_size(a: &GraphBlockSize, b: &GraphBlockSize) -> bool {
    let GraphBlockSize { width, height } = a;
    width.to_bits() == b.width.to_bits() && height.to_bits() == b.height.to_bits()
}

fn same_run_settings(a: &RunSettings, b: &RunSettings) -> bool {
    let RunSettings { effort, permission_mode, max_turns, max_budget_usd, fast_mode, web_search, network_access, advisor } = a;
    effort == &b.effort
        && permission_mode == &b.permission_mode
        && max_turns == &b.max_turns
        && same_opt_f64(*max_budget_usd, b.max_budget_usd)
        && *fast_mode == b.fast_mode
        && web_search == &b.web_search
        && *network_access == b.network_access
        && advisor == &b.advisor
}

fn same_rate_limit(a: &SessionRateLimit, b: &SessionRateLimit) -> bool {
    let SessionRateLimit { kind, percent_used, resets_at } = a;
    kind == &b.kind && same_opt_f64(*percent_used, b.percent_used) && resets_at == &b.resets_at
}

fn same_session_usage(a: &SessionUsage, b: &SessionUsage) -> bool {
    let SessionUsage {
        provider,
        source,
        token_scope,
        model,
        selected_model,
        provider_session_id,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        reasoning_tokens,
        total_tokens,
        context_used_tokens,
        context_window_tokens,
        cost_usd,
        cost_scope,
        rate_limits,
        rate_limits_updated_at,
        updated_at,
    } = a;
    provider == &b.provider
        && source == &b.source
        && token_scope == &b.token_scope
        && model == &b.model
        && selected_model == &b.selected_model
        && provider_session_id == &b.provider_session_id
        && input_tokens == &b.input_tokens
        && output_tokens == &b.output_tokens
        && cache_read_tokens == &b.cache_read_tokens
        && cache_write_tokens == &b.cache_write_tokens
        && reasoning_tokens == &b.reasoning_tokens
        && total_tokens == &b.total_tokens
        && context_used_tokens == &b.context_used_tokens
        && context_window_tokens == &b.context_window_tokens
        && same_opt_f64(*cost_usd, b.cost_usd)
        && cost_scope == &b.cost_scope
        && match (rate_limits, &b.rate_limits) {
            (None, None) => true,
            (Some(x), Some(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same_rate_limit(p, q)),
            _ => false,
        }
        && rate_limits_updated_at == &b.rate_limits_updated_at
        && updated_at == &b.updated_at
}

/// A pane's run settings (C# `RunSettings` and its `RunSettingsJsonConverter`, which reads the exact names and always writes
/// `maxTurns`/`maxBudgetUsd`, null when unset, for legacy v1 hosts).
#[derive(Debug, Clone, PartialEq)]
pub struct RunSettings {
    pub effort: String,
    pub permission_mode: String,
    pub max_turns: Option<i32>,
    pub max_budget_usd: Option<f64>,
    pub fast_mode: bool,
    pub web_search: String,
    pub network_access: bool,
    pub advisor: Option<String>,
}

impl Default for RunSettings {
    fn default() -> Self {
        RunSettings {
            effort: "default".into(),
            permission_mode: "manual".into(),
            max_turns: None,
            max_budget_usd: None,
            fast_mode: false,
            web_search: "default".into(),
            network_access: false,
            advisor: None,
        }
    }
}

impl Wire for RunSettings {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "run settings")?;
        let text = |name: &str, default: &str| match o.exact(name) {
            None => Ok(default.to_string()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => fail(format!("run settings {name} is not a string")),
        };
        let flag = |name: &str| match o.exact(name) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => fail(format!("run settings {name} is not a bool")),
        };
        Ok(RunSettings {
            effort: text("effort", "default")?,
            permission_mode: text("permissionMode", "manual")?,
            max_turns: match o.exact("maxTurns") {
                None | Some(Value::Null) => None,
                Some(v) => Some(json::as_integer(v).and_then(|n| i32::try_from(n).ok()).ok_or_else(|| json::ReadError("maxTurns".into()))?),
            },
            max_budget_usd: match o.exact("maxBudgetUsd") {
                None | Some(Value::Null) => None,
                Some(v) => {
                    Some(v.as_f64().filter(|n| v.is_number() && n.is_finite()).ok_or_else(|| json::ReadError("maxBudgetUsd".into()))?)
                }
            },
            fast_mode: flag("fastMode")?,
            web_search: text("webSearch", "default")?,
            network_access: flag("networkAccess")?,
            advisor: o.text("advisor").filter(|a| !a.is_empty()).map(str::to_string),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("effort", Some(&self.effort));
        out.prop_str("permissionMode", Some(&self.permission_mode));
        out.name("maxTurns");
        match self.max_turns {
            Some(turns) => out.int(turns as i64),
            None => out.null(),
        }
        out.name("maxBudgetUsd");
        match self.max_budget_usd {
            Some(budget) => out.float(budget),
            None => out.null(),
        }
        if self.fast_mode {
            out.prop_bool("fastMode", true);
        }
        if self.web_search != "default" {
            out.prop_str("webSearch", Some(&self.web_search));
        }
        if self.network_access {
            out.prop_bool("networkAccess", true);
        }
        out.prop_str("advisor", self.advisor.as_deref());
        out.end_object();
    }
}

// ── LogEntry and what it holds ──────────────────────────────────────────────────────────────────────────────────────────

/// One transcript entry (C# `LogEntry(Id, Kind, Text, Timestamp, Provider?, Activity?, Images?)`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LogEntry {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub text: Option<String>,
    pub timestamp: Option<String>,
    pub provider: Option<String>,
    pub activity: Option<AgentActivity>,
    /// A null picture is kept until `AgentImageSupport.Normalized` counts and drops it.
    pub images: Option<Vec<Option<AgentImageRef>>>,
}

/// The entry kinds a transcript keeps (C# `LogEntry.Stored`).
pub const STORED_KINDS: [&str; 6] = ["user", "assistant", "system", "output", "error", "image"];

impl Wire for LogEntry {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a log entry")?;
        Ok(LogEntry {
            id: json::opt_string(o.get("id"))?,
            kind: json::opt_string(o.get("kind"))?,
            text: json::opt_string(o.get("text"))?,
            timestamp: json::opt_string(o.get("timestamp"))?,
            provider: json::opt_string(o.get("provider"))?,
            activity: match o.get("activity") {
                None | Some(Value::Null) => None,
                Some(v) => AgentActivity::from_json(v),
            },
            images: json::list(o.get("images"), AgentImageRef::read)?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", self.id.as_deref());
        out.prop_str("kind", self.kind.as_deref());
        out.prop_str("text", self.text.as_deref());
        out.prop_str("timestamp", self.timestamp.as_deref());
        out.prop_str("provider", self.provider.as_deref());
        out.prop("activity", self.activity.as_ref());
        if let Some(images) = &self.images {
            out.name("images");
            write_nullable_list(out, images);
        }
        out.end_object();
    }
}

impl LogEntry {
    pub fn new(id: String, kind: &str, text: String, timestamp: String, provider: Option<String>) -> Self {
        LogEntry {
            id: Some(id),
            kind: Some(kind.into()),
            text: Some(text),
            timestamp: Some(timestamp),
            provider,
            activity: None,
            images: None,
        }
    }
    pub fn id(&self) -> &str {
        self.id.as_deref().unwrap_or("")
    }
    pub fn kind(&self) -> &str {
        self.kind.as_deref().unwrap_or("")
    }
    pub fn text(&self) -> &str {
        self.text.as_deref().unwrap_or("")
    }
}

/// C# `AgentActivity(Id, Provider, Kind, State, Summary, ToolName?, Output?, DurationMs?)` with its `AgentActivityConverter`, which
/// normalizes what it reads (`ActivitySupport.Normalize`) and loads anything invalid as null.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentActivity {
    pub id: String,
    pub provider: String,
    pub kind: String,
    pub state: String,
    pub summary: String,
    pub tool_name: Option<String>,
    pub output: Option<String>,
    pub duration_ms: Option<f64>,
}

impl AgentActivity {
    pub fn from_json(v: &Value) -> Option<AgentActivity> {
        crate::normalize::activity(
            &AgentActivity {
                id: json::text(v, "id").unwrap_or("").into(),
                provider: json::text(v, "provider").unwrap_or("").into(),
                kind: json::text(v, "kind").unwrap_or("").into(),
                state: json::text(v, "state").unwrap_or("").into(),
                summary: json::text(v, "summary").unwrap_or("").into(),
                tool_name: json::text(v, "toolName").map(str::to_string),
                output: json::text(v, "output").map(str::to_string),
                duration_ms: json::number(v, "durationMs"),
            },
            false,
        )
    }
}

impl Wire for AgentActivity {
    fn read(value: &Value) -> R<Self> {
        AgentActivity::from_json(value).ok_or_else(|| json::ReadError("an invalid activity".into()))
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("provider", Some(&self.provider));
        out.prop_str("kind", Some(&self.kind));
        out.prop_str("state", Some(&self.state));
        out.prop_str("summary", Some(&self.summary));
        out.prop_str("toolName", self.tool_name.as_deref());
        out.prop_str("output", self.output.as_deref());
        out.prop_opt_float("durationMs", self.duration_ms);
        out.end_object();
    }
}

/// C# `AgentImageRef(Hash, MediaType, Width, Height, Bytes, Source, Path?)`.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentImageRef {
    pub hash: Option<String>,
    pub media_type: Option<String>,
    pub width: i32,
    pub height: i32,
    pub bytes: i32,
    pub source: Option<String>,
    pub path: Option<String>,
}

impl Wire for AgentImageRef {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a picture")?;
        Ok(AgentImageRef {
            hash: json::opt_string(o.get("hash"))?,
            media_type: json::opt_string(o.get("mediaType"))?,
            width: json::int32(o.get("width"), 0)?,
            height: json::int32(o.get("height"), 0)?,
            bytes: json::int32(o.get("bytes"), 0)?,
            source: json::opt_string(o.get("source"))?,
            path: json::opt_string(o.get("path"))?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("hash", self.hash.as_deref());
        out.prop_str("mediaType", self.media_type.as_deref());
        out.prop_int("width", self.width as i64);
        out.prop_int("height", self.height as i64);
        out.prop_int("bytes", self.bytes as i64);
        out.prop_str("source", self.source.as_deref());
        out.prop_str("path", self.path.as_deref());
        out.end_object();
    }
}

/// C# `AgentRunTiming(StartedAt, LastObservedAt, FinishedAt?, IsApproximate)` with its converter: times in `"O"`, an invalid
/// record read as null.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentRunTiming {
    pub started_at: DateTimeOffset,
    pub last_observed_at: DateTimeOffset,
    pub finished_at: Option<DateTimeOffset>,
    pub is_approximate: bool,
}

impl AgentRunTiming {
    pub fn is_valid(&self) -> bool {
        self.last_observed_at >= self.started_at && self.finished_at.is_none_or(|f| f >= self.last_observed_at)
    }

    /// `Interrupt()`: a run the quit cut off ends where it was last seen, approximately.
    pub fn interrupt(self) -> Self {
        if self.finished_at.is_some() {
            return self;
        }
        AgentRunTiming { finished_at: Some(self.last_observed_at), is_approximate: true, ..self }
    }

    /// `Observe(at)`.
    pub fn observe(self, at: Option<DateTimeOffset>) -> Self {
        if self.finished_at.is_some() {
            return self;
        }
        let now = at.unwrap_or_else(DateTimeOffset::now);
        // new[] { StartedAt, LastObservedAt, now }.Max(): the first of the greatest.
        let mut latest = self.started_at;
        for candidate in [self.last_observed_at, now] {
            if candidate > latest {
                latest = candidate;
            }
        }
        AgentRunTiming { last_observed_at: latest, ..self }
    }

    pub fn from_json(v: &Value) -> Option<AgentRunTiming> {
        let start = time::parse(json::text(v, "startedAt"))?;
        let end = time::parse(json::text(v, "finishedAt"));
        let observed = time::parse(json::text(v, "lastObservedAt")).or(end).unwrap_or(start);
        let present = |name: &str| v.as_object().and_then(|o| o.get(name)).is_some_and(|x| !x.is_null());
        if present("finishedAt") && end.is_none() || present("lastObservedAt") && time::parse(json::text(v, "lastObservedAt")).is_none() {
            return None;
        }
        let value = AgentRunTiming {
            started_at: start,
            last_observed_at: observed,
            finished_at: end,
            is_approximate: json::flag(v, "isApproximate"),
        };
        value.is_valid().then_some(value)
    }

    /// `Infer(logs)`: from the latest request to the latest answer after it, approximately.
    pub fn infer(logs: &[LogEntry]) -> Option<AgentRunTiming> {
        let index = logs.iter().rposition(|e| e.kind() == "user")?;
        let start = time::parse(logs[index].timestamp.as_deref())?;
        let mut latest: Option<DateTimeOffset> = None;
        for entry in &logs[index + 1..] {
            if !(matches!(entry.kind(), "assistant" | "output" | "error") || entry.activity.is_some()) {
                continue;
            }
            if let Some(date) = time::parse(entry.timestamp.as_deref()).filter(|d| *d > start)
                && latest.is_none_or(|l| date > l)
            {
                latest = Some(date);
            }
        }
        latest.map(|end| AgentRunTiming { started_at: start, last_observed_at: end, finished_at: Some(end), is_approximate: true })
    }
}

impl Wire for AgentRunTiming {
    fn read(value: &Value) -> R<Self> {
        AgentRunTiming::from_json(value).ok_or_else(|| json::ReadError("an invalid run timing".into()))
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("startedAt", Some(&self.started_at.to_o()));
        out.prop_str("lastObservedAt", Some(&self.last_observed_at.to_o()));
        out.prop_str("finishedAt", self.finished_at.map(|f| f.to_o()).as_deref());
        out.prop_bool("isApproximate", self.is_approximate);
        out.end_object();
    }
}

/// C# `SessionRateLimit(Kind, PercentUsed?, ResetsAt?)`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRateLimit {
    pub kind: String,
    pub percent_used: Option<f64>,
    pub resets_at: Option<String>,
}

impl Wire for SessionRateLimit {
    fn read(value: &Value) -> R<Self> {
        Ok(SessionRateLimit {
            kind: json::text(value, "kind").unwrap_or("").into(),
            percent_used: json::number(value, "percentUsed"),
            resets_at: json::text(value, "resetsAt").map(str::to_string),
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("kind", Some(&self.kind));
        out.prop_opt_float("percentUsed", self.percent_used);
        out.prop_str("resetsAt", self.resets_at.as_deref());
        out.end_object();
    }
}

/// C# `SessionUsage` with its `SessionUsageConverter`, which normalizes what it reads (`SessionUsageSupport.Normalize`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionUsage {
    pub provider: String,
    pub source: String,
    pub token_scope: String,
    pub model: Option<String>,
    pub selected_model: Option<String>,
    pub provider_session_id: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub context_used_tokens: Option<i64>,
    pub context_window_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub cost_scope: Option<String>,
    pub rate_limits: Option<Vec<SessionRateLimit>>,
    pub rate_limits_updated_at: Option<String>,
    pub updated_at: String,
}

impl SessionUsage {
    pub fn from_json(v: &Value) -> Option<SessionUsage> {
        let limits = match v.as_object().and_then(|o| o.get("rateLimits")) {
            Some(Value::Array(items)) => {
                Some(items.iter().take(16).map(|r| SessionRateLimit::read(r).unwrap_or_else(|_| unreachable!())).collect())
            }
            _ => None,
        };
        let text = |name: &str| json::text(v, name).map(str::to_string);
        crate::normalize::session_usage(&SessionUsage {
            provider: text("provider").unwrap_or_default(),
            source: text("source").unwrap_or_default(),
            token_scope: text("tokenScope").unwrap_or_default(),
            model: text("model"),
            selected_model: text("selectedModel"),
            provider_session_id: text("providerSessionId"),
            input_tokens: json::integer(v, "inputTokens"),
            output_tokens: json::integer(v, "outputTokens"),
            cache_read_tokens: json::integer(v, "cacheReadTokens"),
            cache_write_tokens: json::integer(v, "cacheWriteTokens"),
            reasoning_tokens: json::integer(v, "reasoningTokens"),
            total_tokens: json::integer(v, "totalTokens"),
            context_used_tokens: json::integer(v, "contextUsedTokens"),
            context_window_tokens: json::integer(v, "contextWindowTokens"),
            cost_usd: json::number(v, "costUSD"),
            cost_scope: text("costScope"),
            rate_limits: limits,
            rate_limits_updated_at: text("rateLimitsUpdatedAt"),
            updated_at: text("updatedAt").unwrap_or_default(),
        })
    }
}

impl Wire for SessionUsage {
    fn read(value: &Value) -> R<Self> {
        SessionUsage::from_json(value).ok_or_else(|| json::ReadError("an invalid usage".into()))
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("provider", Some(&self.provider));
        out.prop_str("source", Some(&self.source));
        out.prop_str("tokenScope", Some(&self.token_scope));
        out.prop_str("model", self.model.as_deref());
        out.prop_str("selectedModel", self.selected_model.as_deref());
        out.prop_str("providerSessionId", self.provider_session_id.as_deref());
        out.prop_opt_int("inputTokens", self.input_tokens);
        out.prop_opt_int("outputTokens", self.output_tokens);
        out.prop_opt_int("cacheReadTokens", self.cache_read_tokens);
        out.prop_opt_int("cacheWriteTokens", self.cache_write_tokens);
        out.prop_opt_int("reasoningTokens", self.reasoning_tokens);
        out.prop_opt_int("totalTokens", self.total_tokens);
        out.prop_opt_int("contextUsedTokens", self.context_used_tokens);
        out.prop_opt_int("contextWindowTokens", self.context_window_tokens);
        out.prop_opt_float("costUSD", self.cost_usd);
        out.prop_str("costScope", self.cost_scope.as_deref());
        out.prop_list("rateLimits", self.rate_limits.as_ref());
        out.prop_str("rateLimitsUpdatedAt", self.rate_limits_updated_at.as_deref());
        out.prop_str("updatedAt", Some(&self.updated_at));
        out.end_object();
    }
}

// ── Plan, checklist, background work, shell block runs ──────────────────────────────────────────────────────────────────

/// C# `PlanRecord(Id, RunId, Plan, ReceivedAt, DecidedAt, Outcome, Feedback?, PlanTruncated?, GraphRunId?, LaunchOverride?)`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRecord {
    pub id: Option<String>,
    pub run_id: Option<String>,
    pub plan: Option<String>,
    pub received_at: Option<String>,
    pub decided_at: Option<String>,
    pub outcome: Option<String>,
    pub feedback: Option<String>,
    pub plan_truncated: Option<bool>,
    pub graph_run_id: Option<String>,
    pub launch_override: Option<String>,
}

impl Wire for PlanRecord {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a plan record")?;
        Ok(PlanRecord {
            id: json::opt_string(o.get("id"))?,
            run_id: json::opt_string(o.get("runId"))?,
            plan: json::opt_string(o.get("plan"))?,
            received_at: json::opt_string(o.get("receivedAt"))?,
            decided_at: json::opt_string(o.get("decidedAt"))?,
            outcome: json::opt_string(o.get("outcome"))?,
            feedback: json::opt_string(o.get("feedback"))?,
            plan_truncated: json::opt_boolean(o.get("planTruncated"))?,
            graph_run_id: json::opt_string(o.get("graphRunId"))?,
            launch_override: json::opt_string(o.get("launchOverride"))?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", self.id.as_deref());
        out.prop_str("runId", self.run_id.as_deref());
        out.prop_str("plan", self.plan.as_deref());
        out.prop_str("receivedAt", self.received_at.as_deref());
        out.prop_str("decidedAt", self.decided_at.as_deref());
        out.prop_str("outcome", self.outcome.as_deref());
        out.prop_str("feedback", self.feedback.as_deref());
        out.prop_opt_bool("planTruncated", self.plan_truncated);
        out.prop_str("graphRunId", self.graph_run_id.as_deref());
        out.prop_str("launchOverride", self.launch_override.as_deref());
        out.end_object();
    }
}

/// C# `TodoItem(Content, Status, ActiveForm?, Id?)`.
#[derive(Debug, Clone, PartialEq)]
pub struct TodoItem {
    pub content: Option<String>,
    pub status: Option<String>,
    pub active_form: Option<String>,
    pub id: Option<String>,
}

impl Wire for TodoItem {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a checklist item")?;
        Ok(TodoItem {
            content: json::opt_string(o.get("content"))?,
            status: json::opt_string(o.get("status"))?,
            active_form: json::opt_string(o.get("activeForm"))?,
            id: json::opt_string(o.get("id"))?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("content", self.content.as_deref());
        out.prop_str("status", self.status.as_deref());
        out.prop_str("activeForm", self.active_form.as_deref());
        out.prop_str("id", self.id.as_deref());
        out.end_object();
    }
}

/// C# `TodoProgress(Items, RunId?)`.
#[derive(Debug, Clone, PartialEq)]
pub struct TodoProgress {
    pub items: Option<Vec<TodoItem>>,
    pub run_id: Option<String>,
}

impl Wire for TodoProgress {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a checklist")?;
        Ok(TodoProgress { items: json::skipping_list(o.get("items"))?, run_id: json::opt_string(o.get("runId"))? })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_list("items", self.items.as_ref());
        out.prop_str("runId", self.run_id.as_deref());
        out.end_object();
    }
}

/// C# `BackgroundTask(Id, Kind, Description, StartedAt, Status = "running", ToolUseId?, Summary?, EndedAt?)`.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundTask {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub description: Option<String>,
    pub started_at: Option<String>,
    pub status: Option<String>,
    pub tool_use_id: Option<String>,
    pub summary: Option<String>,
    pub ended_at: Option<String>,
}

impl Wire for BackgroundTask {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a background task")?;
        Ok(BackgroundTask {
            id: json::opt_string(o.get("id"))?,
            kind: json::opt_string(o.get("kind"))?,
            description: json::opt_string(o.get("description"))?,
            started_at: json::opt_string(o.get("startedAt"))?,
            status: json::string(o.get("status"), some("running"))?,
            tool_use_id: json::opt_string(o.get("toolUseId"))?,
            summary: json::opt_string(o.get("summary"))?,
            ended_at: json::opt_string(o.get("endedAt"))?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", self.id.as_deref());
        out.prop_str("kind", self.kind.as_deref());
        out.prop_str("description", self.description.as_deref());
        out.prop_str("startedAt", self.started_at.as_deref());
        out.prop_str("status", self.status.as_deref());
        out.prop_str("toolUseId", self.tool_use_id.as_deref());
        out.prop_str("summary", self.summary.as_deref());
        out.prop_str("endedAt", self.ended_at.as_deref());
        out.end_object();
    }
}

/// C# `BackgroundWork(Tasks, TurnEnded = false)`.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundWork {
    pub tasks: Option<Vec<BackgroundTask>>,
    pub turn_ended: bool,
}

impl Wire for BackgroundWork {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "background work")?;
        Ok(BackgroundWork { tasks: json::skipping_list(o.get("tasks"))?, turn_ended: json::boolean(o.get("turnEnded"), false)? })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_list("tasks", self.tasks.as_ref());
        out.prop_bool("turnEnded", self.turn_ended);
        out.end_object();
    }
}

/// C# `ShellBlockRun(Key, Command, State, ExitCode?, Output, StartedAt, EndedAt?)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellBlockRun {
    pub key: Option<String>,
    pub command: Option<String>,
    pub state: Option<String>,
    pub exit_code: Option<i32>,
    pub output: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

impl Wire for ShellBlockRun {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a shell block run")?;
        Ok(ShellBlockRun {
            key: json::opt_string(o.get("key"))?,
            command: json::opt_string(o.get("command"))?,
            state: json::opt_string(o.get("state"))?,
            exit_code: json::opt_int32(o.get("exitCode"))?,
            output: json::opt_string(o.get("output"))?,
            started_at: json::opt_string(o.get("startedAt"))?,
            ended_at: json::opt_string(o.get("endedAt"))?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("key", self.key.as_deref());
        out.prop_str("command", self.command.as_deref());
        out.prop_str("state", self.state.as_deref());
        out.prop_opt_int("exitCode", self.exit_code.map(i64::from));
        out.prop_str("output", self.output.as_deref());
        out.prop_str("startedAt", self.started_at.as_deref());
        out.prop_str("endedAt", self.ended_at.as_deref());
        out.end_object();
    }
}

// ── Layout ──────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// A workspace layout node (C# `PaneLayoutNode`): a tab group (`kind: "tabs"`) or a binary split.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneLayoutNode {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub session_ids: Option<Vec<Option<String>>>,
    pub selected_session_id: Option<String>,
    pub axis: Option<String>,
    pub ratio: f64,
    pub children: Option<Vec<Option<PaneLayoutNode>>>,
}

impl Default for PaneLayoutNode {
    /// `new PaneLayoutNode()`: a tab group with a new id.
    fn default() -> Self {
        PaneLayoutNode {
            id: Some(new_id()),
            kind: Some("tabs".into()),
            session_ids: Some(vec![]),
            selected_session_id: None,
            axis: None,
            ratio: 0.5,
            children: Some(vec![]),
        }
    }
}

impl Wire for PaneLayoutNode {
    fn read(value: &Value) -> R<Self> {
        let o = Obj::of(value, "a layout node")?;
        Ok(PaneLayoutNode {
            id: json::string(o.get("id"), || Some(new_id()))?,
            kind: json::string(o.get("kind"), some("tabs"))?,
            session_ids: match o.get("sessionIds") {
                None => Some(vec![]),
                value => json::string_list(value)?,
            },
            selected_session_id: json::opt_string(o.get("selectedSessionId"))?,
            axis: json::opt_string(o.get("axis"))?,
            ratio: json::float64(o.get("ratio"), 0.5)?,
            children: match o.get("children") {
                None => Some(vec![]),
                value => json::list(value, PaneLayoutNode::read)?,
            },
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", self.id.as_deref());
        out.prop_str("kind", self.kind.as_deref());
        if let Some(ids) = &self.session_ids {
            out.name("sessionIds");
            write_nullable_strings(out, ids);
        }
        out.prop_str("selectedSessionId", self.selected_session_id.as_deref());
        out.prop_str("axis", self.axis.as_deref());
        out.prop_float("ratio", self.ratio);
        if let Some(children) = &self.children {
            out.name("children");
            write_nullable_list(out, children);
        }
        out.end_object();
    }
}

impl PaneLayoutNode {
    pub fn id(&self) -> &str {
        self.id.as_deref().unwrap_or("")
    }
    pub fn kind(&self) -> &str {
        self.kind.as_deref().unwrap_or("")
    }
    /// The panes of a tab group (a null id left out).
    pub fn session_ids(&self) -> impl Iterator<Item = &str> {
        self.session_ids.iter().flatten().flatten().map(String::as_str)
    }
    pub fn children(&self) -> impl Iterator<Item = &PaneLayoutNode> {
        self.children.iter().flatten().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_checklist_is_on_unless_switched_off() {
        let mut snapshot = AppSnapshot::default();
        assert!(snapshot.agent_checklist_on(), "a state file that never named it asks");
        snapshot.agent_checklist = Some(false);
        assert!(!snapshot.agent_checklist_on());
        snapshot.agent_checklist = Some(true);
        assert!(snapshot.agent_checklist_on());
    }

    #[test]
    fn the_agent_checklist_reads_the_older_key_and_saves_the_new_one() {
        let read = |text: &str| AppSnapshot::read(&serde_json::from_str::<Value>(text).unwrap()).unwrap();
        let old = read(r#"{"version":1,"antigravityChecklist":false}"#);
        assert_eq!(old.agent_checklist, Some(false), "0.5.4's key keeps its choice");
        assert_eq!(read(r#"{"version":1,"agentChecklist":true,"antigravityChecklist":false}"#).agent_checklist, Some(true));
        let mut out = JsonOut::new();
        old.write_with(&mut out, false, false);
        let saved: Value = serde_json::from_str(&out.finish()).unwrap();
        assert_eq!(saved["agentChecklist"], false);
        assert!(saved.get("antigravityChecklist").is_none());
    }

    #[test]
    fn a_session_round_trips_in_csharp_order() {
        let text = r#"{"id":"s1","workspaceId":"w1","title":"T","kind":"claude","status":"idle","model":"default","provider":"claude","settings":{"effort":"default","permissionMode":"manual","maxTurns":null,"maxBudgetUsd":null},"logs":[],"createdAt":"2026-01-01T00:00:00Z","draft":"","graphSizesLayout":1}"#;
        let value: Value = serde_json::from_str(text).unwrap();
        let session = RunSession::read(&value).unwrap();
        assert_eq!(session.to_json(), text);
    }

    #[test]
    fn absent_fields_take_the_csharp_defaults_and_nulls_stay_null() {
        let value: Value = serde_json::from_str(r#"{"version":1,"workspaces":[],"sessions":[],"theme":null,"Layout":"tabs"}"#).unwrap();
        let state = AppSnapshot::read(&value).unwrap();
        assert_eq!(state.theme, None);
        assert_eq!(state.layout.as_deref(), Some("tabs"));
        assert_eq!(state.sidebar_width, 252.0);
        assert!(state.status_line_enabled);
        assert!(AppSnapshot::read(&serde_json::from_str::<Value>(r#"{"sidebarCollapsed":"yes"}"#).unwrap()).is_err());
        let lenient =
            AppSnapshot::read(&serde_json::from_str::<Value>(r#"{"autoLoginCLIs":"yes","referenceBubbleWidth":"x"}"#).unwrap()).unwrap();
        assert_eq!(lenient.auto_login_clis, None);
        assert_eq!(lenient.reference_bubble_width, None);
    }

    /// 휴대폰에서 볼 워크스페이스: every local workspace by default (nothing new written, so C#'s and Swift's text stays as it was), only
    /// the shared ones once chosen, which survives a save, a read and the normalizing pass; anything but true reads as the default.
    #[test]
    fn the_phone_workspace_choice_is_saved_only_when_it_narrows() {
        let default = MobileRemoteSettings::default();
        assert!(!default.phone_shared_only && !default.to_json().contains("phoneSharedOnly"), "every workspace, nothing written");
        let shared = MobileRemoteSettings { phone_shared_only: true, ..MobileRemoteSettings::default() };
        let text = shared.to_json();
        assert!(text.ends_with(r#","phoneSharedOnly":true}"#), "{text}");
        let read = MobileRemoteSettings::read(&serde_json::from_str::<Value>(&text).unwrap()).unwrap();
        assert_eq!(read, shared, "it round-trips");
        assert!(crate::normalize::mobile_remote(Some(&read)).phone_shared_only, "the normalizing pass keeps it");
        for odd in [r#"{"phoneSharedOnly":"yes"}"#, r#"{"phoneSharedOnly":null}"#, r#"{"phoneSharedOnly":1}"#, "{}"] {
            assert!(!MobileRemoteSettings::read(&serde_json::from_str::<Value>(odd).unwrap()).unwrap().phone_shared_only, "{odd}");
        }
        let state = AppSnapshot { mobile_remote: Some(shared.clone()), ..AppSnapshot::default() };
        let again = AppSnapshot::read(&serde_json::from_str::<Value>(&state.to_json()).unwrap()).unwrap();
        assert_eq!(crate::normalize::mobile_remote(again.mobile_remote.as_ref()), shared, "through the saved state");
    }

    /// The legacy apps (key-only auth) are off by default, and the `allowLegacyPhones: true` every older version wrote for everyone
    /// does not turn them on; turned on, the choice is kept in `legacyPhones` (and `allowLegacyPhones` for an older version).
    #[test]
    fn legacy_phones_are_off_unless_turned_on_here() {
        assert!(!MobileRemoteSettings::default().allow_legacy_phones);
        let older = MobileRemoteSettings::read(&serde_json::from_str::<Value>(r#"{"enabled":true,"allowLegacyPhones":true}"#).unwrap());
        let older = older.unwrap();
        assert!(!older.allow_legacy_phones, "an older version's default is no choice");
        assert!(older.to_json().contains(r#""allowLegacyPhones":true,"directEnabled""#), "and it is written back as it was read");
        let on = MobileRemoteSettings { allow_legacy_phones: true, legacy_phones_as_saved: true, ..MobileRemoteSettings::default() };
        let text = on.to_json();
        assert!(text.contains(r#""allowLegacyPhones":true,"legacyPhones":true"#), "{text}");
        assert_eq!(MobileRemoteSettings::read(&serde_json::from_str::<Value>(&text).unwrap()).unwrap(), on, "it round-trips");
        assert!(MobileRemoteSettings::default().to_json().contains(r#""allowLegacyPhones":true,"directEnabled""#), "C#'s default text");
    }
}
