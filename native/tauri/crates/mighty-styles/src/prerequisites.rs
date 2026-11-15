//! What a style still needs (C# `StylePrerequisites.cs`): an executable on PATH, a skill's `SKILL.md` in the user's or the
//! workspace's `.claude\skills`, a plugin in `~\.claude\plugins\installed_plugins.json` installed for the user. Owner: W9-D.

use std::path::Path;

use serde_json::Value;

use crate::manifest::{StyleManifest, items, strings, text};

/// C# `StylePrerequisiteResult(Ready, Missing, Hint, InstallCommand)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StylePrerequisiteResult {
    pub ready: bool,
    pub missing: Vec<String>,
    pub hint: Option<String>,
    pub install_command: Option<String>,
}

impl StylePrerequisiteResult {
    pub fn ready() -> Self {
        StylePrerequisiteResult { ready: true, ..Self::default() }
    }
}

/// `StylePrerequisites.Read(manifest, workspace, home)`.
pub fn read(manifest: &StyleManifest, workspace: &Path, home: &Path) -> StylePrerequisiteResult {
    let p = manifest.get("prerequisites");
    let probes = items(p, "probes");
    let mut missing: Vec<&Value> = Vec::new();
    for probe in probes {
        let kind = text(probe, "kind");
        let name = text(probe, "name").or_else(|| text(probe, "prefix")).unwrap_or_default();
        let ready = match kind {
            Some("executable") => std::env::var_os("PATH").is_some_and(|path| {
                std::env::split_paths(&path)
                    .any(|dir| ["", ".exe", ".cmd", ".bat"].iter().any(|suffix| dir.join(format!("{name}{suffix}")).is_file()))
            }),
            Some("skill") => strings(probe, "scopes").iter().any(|scope| {
                let base = if scope == "user" { home } else { workspace };
                base.join(".claude").join("skills").join(name).join("SKILL.md").is_file()
            }),
            Some("plugin") => crate::files::read(home, ".claude/plugins/installed_plugins.json", 4 * 1024 * 1024)
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|doc| doc.get("plugins").and_then(Value::as_object).cloned())
                .is_some_and(|plugins| {
                    plugins.iter().any(|(key, value)| {
                        key.starts_with(name) && value.as_array().is_some_and(|a| a.iter().any(|item| text(item, "scope") == Some("user")))
                    })
                }),
            _ => false,
        };
        if !ready {
            missing.push(probe);
        }
    }
    let mode = text(p, "mode");
    if probes.is_empty() || (mode == Some("all") && missing.is_empty()) || (mode == Some("any") && missing.len() < probes.len()) {
        return StylePrerequisiteResult::ready();
    }
    let shown: Vec<&Value> = if text(p, "report") == Some("all") { missing.clone() } else { missing.iter().take(1).copied().collect() };
    let installs = missing.iter().any(|p| p.get("install").is_none_or(|v| v == &Value::Bool(true)));
    StylePrerequisiteResult {
        ready: false,
        missing: shown.iter().map(|p| text(p, "missing").unwrap_or_default().to_string()).collect(),
        hint: missing.first().and_then(|p| text(p, "hint")).map(str::to_string),
        install_command: if installs { manifest.root.get("install").and_then(|i| text(i, "command")).map(str::to_string) } else { None },
    }
}
