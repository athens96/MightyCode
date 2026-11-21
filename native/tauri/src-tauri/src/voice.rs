//! Desktop voice dictation & Push-to-Talk (`MC-4`): bridges `mighty_platform::SpeechRecognizer` to the web composer (`voiceStatus`,
//! `voiceHints`, `voiceStart`, `voiceStop`, `voiceOpenSettings`), extracts workspace file/symbol names as contextual STT hints, and
//! restores exact code-identifier casing in recognized transcripts.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use mighty_platform::{SpeechEvent, SpeechRequest, SpeechSession};
use serde_json::{Value, json};

use crate::host::{Host, Registry, lock};
use crate::methods::{flag, text};

struct ActiveVoice {
    pane_id: String,
    session: Box<dyn SpeechSession>,
}

static ACTIVE: LazyLock<Mutex<(u64, Option<ActiveVoice>)>> = LazyLock::new(|| Mutex::new((0, None)));

pub fn shutdown() {
    let mut guard = lock(&ACTIVE);
    if let Some(active) = guard.1.take() {
        active.session.cancel();
    }
}

fn bcp47_for_language(code: &str) -> String {
    let trimmed = code.trim();
    if trimmed.contains('-') {
        return trimmed.to_string();
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "ko" => "ko-KR".into(),
        "en" => "en-US".into(),
        "ja" => "ja-JP".into(),
        "zh" => "zh-CN".into(),
        _ => "ko-KR".into(),
    }
}

fn pane_workspace_root(host: &Host, pane_id: &str) -> Option<String> {
    let view = host.view();
    let pane = view.snapshot.session(pane_id)?;
    let workspace = view.snapshot.workspace(pane.workspace_id()).filter(|w| w.remote.is_none())?;
    Some(pane.working_folder.clone().unwrap_or_else(|| workspace.path().to_string()))
}

const NOISE_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", "build", ".next", ".expo", ".turbo", "vendor", "out", "coverage"];

fn is_code_extension(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "swift"
            | "kt"
            | "go"
            | "java"
            | "cs"
            | "c"
            | "cpp"
            | "h"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "md"
            | "css"
            | "html"
            | "sh"
    )
}

fn push_hint(hints: &mut Vec<String>, seen: &mut HashSet<String>, candidate: &str) {
    let trimmed = candidate.trim();
    if trimmed.len() >= 3 && trimmed.len() <= 48 && !trimmed.chars().any(char::is_control) && seen.insert(trimmed.to_string()) {
        hints.push(trimmed.to_string());
    }
}

/// Extracts up to `max_hints` file names, module stems, and exported symbol names from `root` to boost code-vocabulary recognition.
pub fn workspace_code_hints(root: &Path, max_hints: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut hints = Vec::new();

    let mut queue = std::collections::VecDeque::new();
    queue.push_back((root.to_path_buf(), 0usize));
    let mut visited_dirs = 0usize;
    let mut symbol_files = Vec::new();

    while let Some((dir, depth)) = queue.pop_front() {
        if hints.len() >= max_hints || visited_dirs >= 24 {
            break;
        }
        visited_dirs += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten().take(80) {
            if hints.len() >= max_hints {
                break;
            }
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if name.starts_with('.') && name != ".env" {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if depth < 3 && !NOISE_DIRS.contains(&name.as_ref()) {
                    push_hint(&mut hints, &mut seen, &name);
                    queue.push_back((entry.path(), depth + 1));
                }
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let ext = Path::new(name.as_ref()).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if is_code_extension(&ext) {
                push_hint(&mut hints, &mut seen, &name);
                if let Some(stem) = Path::new(name.as_ref()).file_stem().map(|s| s.to_string_lossy())
                    && stem != "index"
                    && stem != "mod"
                    && stem != "lib"
                    && stem != "main"
                {
                    push_hint(&mut hints, &mut seen, &stem);
                }
                if symbol_files.len() < 16 && matches!(ext.as_str(), "rs" | "ts" | "tsx" | "swift" | "kt" | "py") {
                    symbol_files.push(entry.path());
                }
            }
        }
    }

    for path in symbol_files {
        if hints.len() >= max_hints {
            break;
        }
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.len() > 64 * 1024 {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        for line in content.lines().take(200) {
            for prefix in [
                "export function ",
                "export async function ",
                "export interface ",
                "export type ",
                "export class ",
                "pub fn ",
                "pub async fn ",
                "pub struct ",
                "pub enum ",
                "pub trait ",
                "class ",
                "struct ",
                "def ",
            ] {
                if let Some(rest) = line.trim_start().strip_prefix(prefix) {
                    let ident: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                    if ident.len() >= 4 {
                        push_hint(&mut hints, &mut seen, &ident);
                    }
                }
            }
            if hints.len() >= max_hints {
                break;
            }
        }
    }

    hints
}

fn is_distinctive_code_term(term: &str) -> bool {
    if term.len() < 4 {
        return false;
    }
    let has_mixed = term.chars().any(|c| c.is_ascii_uppercase()) && term.chars().any(|c| c.is_ascii_lowercase());
    let has_punct = term.contains('.') || term.contains('_') || term.contains('-');
    has_mixed || has_punct
}

/// Normalizes spoken file-extension phrases (`"dot tsx"`, localized spoken dot) and restores exact identifier/filename casing from `hints`.
pub fn apply_code_term_hints(transcript: &str, hints: &[String], spoken_dot: &str) -> String {
    if transcript.trim().is_empty() {
        return String::new();
    }
    let mut normalized = transcript.to_string();
    let dot_word = spoken_dot.trim();
    for ext in ["tsx", "ts", "rs", "json", "md", "py", "css"] {
        let mut patterns = vec![format!(" dot {ext}")];
        if !dot_word.is_empty() && !dot_word.eq_ignore_ascii_case("dot") {
            patterns.push(format!(" {dot_word} {ext}"));
        }
        for pat in patterns {
            let lower_pat = pat.to_lowercase();
            loop {
                let lower_norm = normalized.to_lowercase();
                let Some(idx) = lower_norm.find(&lower_pat) else { break };
                normalized.replace_range(idx..idx + pat.len(), &format!(".{ext}"));
            }
        }
    }

    let mut lookup: HashMap<String, &str> = HashMap::new();
    for hint in hints {
        if is_distinctive_code_term(hint) {
            lookup.entry(hint.to_ascii_lowercase()).or_insert(hint.as_str());
        }
    }
    if lookup.is_empty() {
        return normalized;
    }

    // Token-preserving scan: only replace standalone ASCII identifier/filename runs that match a distinctive workspace term.
    let chars: Vec<char> = normalized.chars().collect();
    let mut out = String::with_capacity(normalized.len());
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_ascii_alphanumeric() || ch == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '_' | '.' | '-')) {
                i += 1;
            }
            // Trim trailing sentence period if not part of a known filename.
            let mut end = i;
            while end > start && matches!(chars[end - 1], '.' | '-') {
                let candidate: String = chars[start..end].iter().collect();
                if lookup.contains_key(&candidate.to_ascii_lowercase()) {
                    break;
                }
                end -= 1;
            }
            let token: String = chars[start..end].iter().collect();
            if let Some(canonical) = lookup.get(&token.to_ascii_lowercase()) {
                out.push_str(canonical);
            } else {
                out.push_str(&token);
            }
            for &tail in &chars[end..i] {
                out.push(tail);
            }
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

pub fn register(methods: &mut Registry) {
    methods.add("voiceStatus", |_host: Arc<Host>, _| async move {
        let port = crate::platform::speech_recognizer();
        Ok(json!({
            "nativeAvailable": port.available(),
            "engine": port.engine_name(),
            "permission": port.permission().word(),
        }))
    });

    methods.add("voiceHints", |host: Arc<Host>, args| async move {
        let Some(pane_id) = text(&args, "paneId").map(str::to_string) else {
            return Ok(json!({"hints": []}));
        };
        let Some(root) = pane_workspace_root(&host, &pane_id) else {
            return Ok(json!({"hints": []}));
        };
        let hints = tauri::async_runtime::spawn_blocking(move || workspace_code_hints(Path::new(&root), 80)).await.unwrap_or_default();
        Ok(json!({"hints": hints}))
    });

    methods.add("voiceStart", |host: Arc<Host>, args| async move {
        let Some(pane_id) = text(&args, "paneId").map(str::to_string) else {
            return Err("Missing paneId.".into());
        };
        let port = crate::platform::speech_recognizer();
        if !port.available() {
            return Ok(json!({"started": false, "reason": "unavailable"}));
        }

        let language = bcp47_for_language(text(&args, "language").unwrap_or_else(|| host.locale.language()));
        let mut caller_hints: Vec<String> = args
            .get("hints")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(Value::as_str).take(60).map(str::to_string).collect())
            .unwrap_or_default();

        if let Some(root) = pane_workspace_root(&host, &pane_id) {
            let ws_hints =
                tauri::async_runtime::spawn_blocking(move || workspace_code_hints(Path::new(&root), 60)).await.unwrap_or_default();
            for h in ws_hints {
                if caller_hints.len() >= 100 {
                    break;
                }
                if !caller_hints.contains(&h) {
                    caller_hints.push(h);
                }
            }
        }

        // Cancel any previous session before starting the new one.
        let session_id = {
            let mut guard = lock(&ACTIVE);
            guard.0 += 1;
            if let Some(prev) = guard.1.take() {
                prev.session.cancel();
            }
            guard.0
        };

        let correction_hints = Arc::new(caller_hints.clone());
        let spoken_dot = host.locale.get("composer.voice.spokenDot");
        let event_host = Arc::downgrade(&host);
        let event_pane = pane_id.clone();
        let hints_for_cb = correction_hints.clone();

        let callback = Arc::new(move |event: SpeechEvent| {
            let Some(host) = event_host.upgrade() else { return };
            // Ignore late events from superseded sessions unless it's a terminal cleanup.
            let is_current = lock(&ACTIVE).0 == session_id;
            if !is_current {
                return;
            }
            let message = match event {
                SpeechEvent::Level(level) => json!({
                    "kind": "voice",
                    "paneId": event_pane,
                    "state": "level",
                    "level": level,
                }),
                SpeechEvent::Partial(raw) => json!({
                    "kind": "voice",
                    "paneId": event_pane,
                    "state": "partial",
                    "text": apply_code_term_hints(&raw, &hints_for_cb, &spoken_dot),
                }),
                SpeechEvent::Transcribing => json!({
                    "kind": "voice",
                    "paneId": event_pane,
                    "state": "transcribing",
                }),
                SpeechEvent::Final { text: raw, engine } => {
                    lock(&ACTIVE).1 = None;
                    json!({
                        "kind": "voice",
                        "paneId": event_pane,
                        "state": "result",
                        "text": apply_code_term_hints(&raw, &hints_for_cb, &spoken_dot),
                        "engine": engine,
                    })
                }
                SpeechEvent::Cancelled => {
                    lock(&ACTIVE).1 = None;
                    json!({
                        "kind": "voice",
                        "paneId": event_pane,
                        "state": "cancelled",
                    })
                }
                SpeechEvent::PermissionDenied { kind } => {
                    lock(&ACTIVE).1 = None;
                    json!({
                        "kind": "voice",
                        "paneId": event_pane,
                        "state": "permissionDenied",
                        "permissionKind": kind,
                    })
                }
                SpeechEvent::Error(error) => {
                    lock(&ACTIVE).1 = None;
                    json!({
                        "kind": "voice",
                        "paneId": event_pane,
                        "state": "error",
                        "error": error,
                    })
                }
            };
            let view = host.view();
            host.push(&view, "voice", message.to_string());
        });

        let started = port.start(SpeechRequest { language, hints: caller_hints }, callback);
        match started {
            Ok(session) => {
                let mut guard = lock(&ACTIVE);
                if guard.0 == session_id {
                    guard.1 = Some(ActiveVoice { pane_id: pane_id.clone(), session });
                } else {
                    session.cancel();
                }
                drop(guard);
                let recording_msg = json!({"kind": "voice", "paneId": pane_id, "state": "recording"});
                host.push(&host.view(), "voice", recording_msg.to_string());
                Ok(json!({"started": true, "engine": port.engine_name()}))
            }
            Err(error) => Ok(json!({"started": false, "reason": error.to_string()})),
        }
    });

    methods.add("voiceStop", |_host: Arc<Host>, args| async move {
        let cancel = flag(&args, "cancel");
        let pane_filter = text(&args, "paneId");
        let target = {
            let mut guard = lock(&ACTIVE);
            let matches_pane = guard.1.as_ref().is_some_and(|active| pane_filter.is_none_or(|wanted| active.pane_id == wanted));
            if !matches_pane {
                None
            } else if cancel {
                guard.1.take()
            } else {
                // Keep in `ACTIVE` during `Transcribing` so a follow-up `voiceStop({ cancel: true })` can still abort STT!
                if let Some(active) = &guard.1 {
                    active.session.stop();
                }
                None
            }
        };
        if let Some(cancelled) = target {
            cancelled.session.cancel();
        }
        Ok(Value::Null)
    });

    methods.add("voiceOpenSettings", |_host: Arc<Host>, _| async move {
        let port = crate::platform::speech_recognizer();
        let opened = tauri::async_runtime::spawn_blocking(move || port.open_permission_settings().is_ok()).await.unwrap_or(false);
        Ok(json!(opened))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_term_hints_restore_casing_and_spoken_extensions() {
        let hints = vec![
            "FilesView.tsx".to_string(),
            "FilesView".to_string(),
            "CodeEditor".to_string(),
            "RunSession".to_string(),
            "mighty-files".to_string(),
        ];
        let ko_dot = mighty_core::locale::Locale::new("ko").get("composer.voice.spokenDot");
        assert_eq!(
            apply_code_term_hints("filesview dot tsx 에서 runsession 타입을 확인해줘", &hints, &ko_dot),
            "FilesView.tsx 에서 RunSession 타입을 확인해줘"
        );
        assert_eq!(apply_code_term_hints("check codeeditor in mighty-files.", &hints, "dot"), "check CodeEditor in mighty-files.");
        assert_eq!(apply_code_term_hints(&format!("FilesView {ko_dot} tsx 수정해줘"), &hints, &ko_dot), "FilesView.tsx 수정해줘");
    }

    #[test]
    fn workspace_hints_extract_filenames_and_exported_symbols() {
        let temp = std::env::temp_dir().join(format!("mighty-voice-hints-{}", mighty_core::wire::new_id()));
        std::fs::create_dir_all(temp.join("src")).unwrap();
        std::fs::create_dir_all(temp.join("node_modules/pkg")).unwrap();
        std::fs::write(temp.join("src/VoiceWidget.tsx"), "export function renderWaveformBars() {}\nexport interface DictationState {}\n")
            .unwrap();
        std::fs::write(temp.join("node_modules/pkg/IgnoredSecret.ts"), "export class IgnoredSecret {}\n").unwrap();

        let hints = workspace_code_hints(&temp, 50);
        assert!(hints.contains(&"VoiceWidget.tsx".to_string()));
        assert!(hints.contains(&"VoiceWidget".to_string()));
        assert!(hints.contains(&"renderWaveformBars".to_string()));
        assert!(hints.contains(&"DictationState".to_string()));
        assert!(!hints.iter().any(|h| h.contains("IgnoredSecret")), "node_modules must be skipped");

        let _ = std::fs::remove_dir_all(&temp);
    }
}
