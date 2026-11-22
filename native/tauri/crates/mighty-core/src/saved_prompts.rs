//! Reusable saved prompts (`<data>/saved-prompts.json`, Swift `SavedPrompts.swift`): scoped per workspace or shared across all
//! workspaces, invoked via `/prompt:<name>` with `{branch}`, `{workspace}`, `{date}`, and `{clipboard}` placeholder substitution.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "saved-prompts.json";
pub const INVOCATION_PREFIX: &str = "prompt:";
pub const MAXIMUM_PROMPTS: usize = 200;
pub const MAXIMUM_TEXT_BYTES: usize = 64 * 1024;
pub const PLACEHOLDERS: [&str; 4] = ["branch", "workspace", "date", "clipboard"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPrompt {
    pub id: String,
    pub name: String,
    pub text: String,
    #[serde(rename = "workspaceId", skip_serializing_if = "Option::is_none", default)]
    pub workspace_id: Option<String>,
}

impl SavedPrompt {
    pub fn new(name: impl Into<String>, text: impl Into<String>, workspace_id: Option<String>) -> Self {
        SavedPrompt {
            id: crate::wire::new_id(),
            name: name.into(),
            text: text.into(),
            workspace_id: workspace_id.filter(|id| !id.trim().is_empty()),
        }
    }
}

/// Normalizes a raw prompt name into a palette-safe slug: lowercased, spaces converted to `-`, keeping Unicode letters, digits, `-`,
/// and `_`, capped at 40 characters; `None` if nothing remains.
pub fn normalized_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let kept: String = trimmed
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == ' ' { '-' } else { c })
        .filter(|c| c.is_alphabetic() || c.is_numeric() || *c == '-' || *c == '_')
        .take(40)
        .collect();
    if kept.is_empty() { None } else { Some(kept) }
}

/// Prompts offered in `workspace_id`: workspace-specific entries sorted by `name` first, then shared (`workspace_id == None`) entries
/// whose `name` is not shadowed by a workspace entry, also sorted by `name`.
pub fn offered(prompts: &[SavedPrompt], workspace_id: Option<&str>) -> Vec<SavedPrompt> {
    let ws = workspace_id.filter(|id| !id.is_empty());
    let mut own: Vec<SavedPrompt> = match ws {
        Some(target) => prompts.iter().filter(|p| p.workspace_id.as_deref() == Some(target)).cloned().collect(),
        None => Vec::new(),
    };
    own.sort_by(|a, b| a.name.cmp(&b.name));
    let names: HashSet<&str> = own.iter().map(|p| p.name.as_str()).collect();
    let mut shared: Vec<SavedPrompt> =
        prompts.iter().filter(|p| p.workspace_id.is_none() && !names.contains(p.name.as_str())).cloned().collect();
    shared.sort_by(|a, b| a.name.cmp(&b.name));
    own.extend(shared);
    own
}

/// Replaces `{branch}`, `{workspace}`, `{date}`, and `{clipboard}` with their values in `values` (defaulting to `""` if absent);
/// leaves any other `{token}` untouched.
pub fn expand(text: &str, values: &BTreeMap<&str, String>) -> String {
    let mut result = text.to_string();
    for key in PLACEHOLDERS {
        let needle = format!("{{{key}}}");
        let replacement = values.get(key).map(String::as_str).unwrap_or("");
        if result.contains(&needle) {
            result = result.replace(&needle, replacement);
        }
    }
    result
}

/// Loads valid saved prompts from `<directory>/saved-prompts.json`, returning an empty list if missing or unreadable.
pub fn load(directory: &Path) -> Vec<SavedPrompt> {
    let path = directory.join(FILE_NAME);
    let Ok(bytes) = std::fs::read(&path) else { return Vec::new() };
    let Ok(parsed) = serde_json::from_slice::<Vec<SavedPrompt>>(&bytes) else { return Vec::new() };
    parsed
        .into_iter()
        .filter(|p| {
            !p.id.trim().is_empty() && normalized_name(&p.name).as_deref() == Some(p.name.as_str()) && p.text.len() <= MAXIMUM_TEXT_BYTES
        })
        .take(MAXIMUM_PROMPTS)
        .collect()
}

/// Atomically writes up to `MAXIMUM_PROMPTS` entries to `<directory>/saved-prompts.json`.
pub fn save(prompts: &[SavedPrompt], directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let capped: Vec<&SavedPrompt> = prompts.iter().take(MAXIMUM_PROMPTS).collect();
    let mut json = serde_json::to_vec_pretty(&capped).map_err(std::io::Error::other)?;
    json.push(b'\n');
    let target = directory.join(FILE_NAME);
    let temp = directory.join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
    std::fs::write(&temp, &json)?;
    if let Err(err) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }
    Ok(())
}

/// Inserts or replaces a prompt in `prompts` (matching `id` if provided, or `(name, workspace_id)` collision) and returns the saved item.
pub fn upsert(
    prompts: &mut Vec<SavedPrompt>,
    id: Option<&str>,
    raw_name: &str,
    text: &str,
    workspace_id: Option<String>,
) -> Result<SavedPrompt, &'static str> {
    let Some(name) = normalized_name(raw_name) else {
        return Err("invalid prompt name");
    };
    if text.trim().is_empty() {
        return Err("prompt text is empty");
    }
    if text.len() > MAXIMUM_TEXT_BYTES {
        return Err("prompt text exceeds 64 KiB");
    }
    let scope = workspace_id.filter(|w| !w.trim().is_empty());
    let target_id = id.filter(|s| !s.trim().is_empty());

    // Remove any conflicting `(name, scope)` entry with a different id so saving under the same name replaces cleanly.
    prompts.retain(|existing| {
        let same_slot = existing.name == name && existing.workspace_id == scope;
        !(same_slot && target_id != Some(existing.id.as_str()))
    });

    if let Some(existing_id) = target_id
        && let Some(slot) = prompts.iter_mut().find(|p| p.id == existing_id)
    {
        slot.name = name;
        slot.text = text.to_string();
        slot.workspace_id = scope;
        return Ok(slot.clone());
    }

    if let Some(slot) = prompts.iter_mut().find(|p| p.name == name && p.workspace_id == scope) {
        slot.text = text.to_string();
        return Ok(slot.clone());
    }

    let item = SavedPrompt {
        id: target_id.map(str::to_string).unwrap_or_else(crate::wire::new_id),
        name,
        text: text.to_string(),
        workspace_id: scope,
    };
    if prompts.len() >= MAXIMUM_PROMPTS {
        prompts.remove(0);
    }
    prompts.push(item.clone());
    Ok(item)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_names_preserving_unicode_letters_and_capping_length() {
        assert_eq!(normalized_name("  Review PR! "), Some("review-pr".into()));
        assert_eq!(normalized_name("릴리스 노트!"), Some("릴리스-노트".into()));
        assert_eq!(normalized_name("!!!"), None);
        let long = "a".repeat(60);
        assert_eq!(normalized_name(&long).unwrap().len(), 40);
    }

    #[test]
    fn workspace_prompts_shadow_shared_prompts_and_placeholders_expand() {
        let list = vec![
            SavedPrompt { id: "1".into(), name: "deploy".into(), text: "shared".into(), workspace_id: None },
            SavedPrompt { id: "2".into(), name: "deploy".into(), text: "local".into(), workspace_id: Some("ws-1".into()) },
            SavedPrompt { id: "3".into(), name: "audit".into(), text: "global".into(), workspace_id: None },
        ];
        let rows = offered(&list, Some("ws-1"));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "deploy");
        assert_eq!(rows[0].text, "local");
        assert_eq!(rows[1].name, "audit");

        let values = BTreeMap::from([
            ("branch", "feat/mc-11".to_string()),
            ("workspace", "MightyCode".to_string()),
            ("date", "2026-11-22".to_string()),
            ("clipboard", "clip".to_string()),
        ]);
        assert_eq!(
            expand("On {branch} in {workspace} ({date}): {clipboard} {other}", &values),
            "On feat/mc-11 in MightyCode (2026-11-22): clip {other}"
        );
    }

    #[test]
    fn round_trips_to_disk_and_upserts_cleanly() {
        let dir = std::env::temp_dir().join(format!("mighty-prompts-test-{}", crate::wire::new_id()));
        let mut prompts = Vec::new();
        let p1 = upsert(&mut prompts, None, "Check CI", "Check {branch}", None).unwrap();
        assert_eq!(p1.name, "check-ci");
        save(&prompts, &dir).unwrap();
        let loaded = load(&dir);
        assert_eq!(loaded, prompts);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
