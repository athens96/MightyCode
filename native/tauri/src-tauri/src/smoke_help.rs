//! The smoke's help capture step (Rust-only; plan W9-H): after the companion step, a two-shot capture through the help capture's own
//! picture path (help_capture.rs: the window sized to the shot, the screen opened, the page's picture saved, the manifest written), so a
//! plain smoke proves the capture still works without the run a full `--help-capture` takes. Owner: W9-H.
//!
//! The two shots are shots.json's `sidebar-collapsed` and `dashboard` (what `MIGHTY_HELP_SHOTS=sidebar-collapsed,dashboard` would take):
//! whole-page shots at 1440 × 900 whose screens need no demo seeding, so the smoke's own state stays as the later steps expect it. Each is
//! taken once, in the page's theme (dark when none is saved, as the store reads it), into `help-shots\<language>\<id>-<theme>.png` beside
//! a `manifest.json` in the capture's shape: under the profile where the app may write it, else beside the smoke's result, so a
//! read-only smoke of a real profile (`smoke.ps1 -Profile`) leaves it as it was. The window's size, position and maximized state, the
//! sidebar and the dashboard are put back afterwards. The pictures are WebView2's on Windows and WKWebView's snapshot on the Mac (M-G).
//! Checks: `helpShotsTaken` (both pictures written),
//! `helpShotSize` (each picture is the window's content, and that content is the shot's size wherever the screen holds it) and
//! `helpManifest` (the manifest lists both, ok).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::help_capture;
use crate::host::{Host, WINDOW};

/// The step's shots (shots.json ids).
pub const SHOTS: [&str; 2] = ["sidebar-collapsed", "dashboard"];

/// Starts the step after the companion step (smoke.rs `Stage::Help`): it reports once through `Smoke::probed`. Not inside a help capture
/// or perf scenario run, which take the page over themselves.
pub fn start(host: &Arc<Host>) -> bool {
    let Some(smoke) = &host.smoke else { return false };
    if smoke.is_special() {
        return false;
    }
    host.log.line("smoke: help capture step (two shots)");
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let report = match run(&host).await {
            Ok(report) => report,
            Err(error) => {
                host.log.line(&format!("smoke: help capture step failed: {error}"));
                json!({"error": error})
            }
        };
        if let Some(smoke) = &host.smoke {
            smoke.probed(&host, report);
        }
    });
    true
}

/// The window's content size in points and its scale, to put it back.
fn window_now(host: &Host) -> Option<(f64, f64, f64)> {
    let window = tauri::Manager::get_window(&host.app, WINDOW)?;
    let scale = window.scale_factor().ok()?;
    let size = window.inner_size().ok()?.to_logical::<f64>(scale);
    Some((size.width, size.height, scale))
}

/// What the step changes of the window and puts back: its maximized state, position and content size, the sidebar and the dashboard.
struct Window {
    maximized: bool,
    position: Option<tauri::PhysicalPosition<i32>>,
    size: Option<(f64, f64, f64)>,
    sidebar_collapsed: bool,
    shows_dashboard: bool,
}

impl Window {
    fn now(host: &Host) -> Window {
        let window = tauri::Manager::get_window(&host.app, WINDOW);
        let (sidebar_collapsed, shows_dashboard) = {
            let view = host.view();
            (view.snapshot.sidebar_collapsed, view.shows_dashboard)
        };
        Window {
            maximized: window.as_ref().is_some_and(|w| w.is_maximized().unwrap_or(false)),
            position: window.as_ref().and_then(|w| w.outer_position().ok()),
            size: window_now(host),
            sidebar_collapsed,
            shows_dashboard,
        }
    }

    fn put_back(&self, host: &Host) {
        if let Some((width, height, _)) = self.size {
            help_capture::resize(host, width, height);
        }
        if let Some(window) = tauri::Manager::get_window(&host.app, WINDOW) {
            if let Some(position) = self.position {
                let _ = window.set_position(position);
            }
            if self.maximized {
                let _ = window.maximize();
            }
        }
        let collapsed = self.sidebar_collapsed;
        if host.current().sidebar_collapsed != collapsed {
            let _ = set_sidebar(host, collapsed);
        }
        host.view().shows_dashboard = self.shows_dashboard;
        host.flush();
    }
}

/// Where the step's pictures go: the profile's `help-shots` where the app may write it, else beside the smoke's result (review M1).
fn output_folder(host: &Host, language: &str) -> std::path::PathBuf {
    let base = match (&host.smoke, host.writes()) {
        (Some(smoke), false) => smoke.out().parent().map_or_else(std::env::temp_dir, Path::to_path_buf),
        _ => host.data_dir.clone(),
    };
    base.join("help-shots").join(language)
}

/// The screen's size in device pixels where the window is.
fn screen_now(host: &Host) -> Option<(u32, u32)> {
    let window = tauri::Manager::get_window(&host.app, WINDOW)?;
    window.current_monitor().ok().flatten().map(|m| (m.size().width, m.size().height))
}

/// The browser and history steps' workspaces live in memory only, never approved by a folder selection, so every save since they were
/// added failed on them and the shots showed "Save failed: Only paths approved via folder selection may be saved." (both OSes; the
/// Mac's help shot showed it first). Both steps are done before this one: their workspaces and panes leave, and the banner with them.
fn leave_finished_steps(host: &Arc<Host>) -> Result<(), String> {
    let finished = [crate::smoke::WORKSPACE, crate::smoke_history::WORKSPACE];
    host.view().update(|state| {
        let left: Vec<String> =
            state.sessions().iter().filter(|p| finished.contains(&p.workspace_id())).map(|p| p.id().to_string()).collect();
        state.workspaces_mut().retain(|w| !finished.contains(&w.id()));
        state.sessions_mut().retain(|p| !finished.contains(&p.workspace_id()));
        if state.active_session_id.as_ref().is_some_and(|id| left.contains(id)) {
            state.active_session_id = None;
        }
        if let Some(expanded) = state.expanded_workspace_ids.as_mut() {
            expanded.retain(|id| !finished.contains(&id.as_str()));
        }
        if state.active_workspace_id.as_deref().is_some_and(|id| finished.contains(&id)) {
            state.active_workspace_id = None;
        }
    })?;
    host.view().error = None;
    Ok(())
}

async fn run(host: &Arc<Host>) -> Result<Value, String> {
    let list: Vec<Value> = serde_json::from_str::<Value>(help_capture::SHOTS)
        .ok()
        .and_then(|v| v.get("shots").and_then(Value::as_array).cloned())
        .ok_or("shots.json lists no shots")?;
    let language = host.locale.language().to_string();
    let output = output_folder(host, &language);
    std::fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    leave_finished_steps(host)?;
    let before = Window::now(host);
    // No saved theme reads as dark (store's saved theme, main.rs's window colour).
    let theme = if host.current().theme() == "light" { "light" } else { "dark" }.to_string();
    let mut shots = vec![];
    let mut sizes = vec![];
    for id in SHOTS {
        let shot =
            list.iter().find(|s| s.get("id").and_then(Value::as_str) == Some(id)).ok_or_else(|| format!("shots.json has no {id}"))?;
        let screen = shot.get("screen").and_then(Value::as_str).unwrap_or(id).to_string();
        let mut entry = help_capture::entry(shot, &screen, &theme, "ok");
        let outcome = take(host, shot, &screen, &output.join(format!("{id}-{theme}.png"))).await;
        match outcome {
            Ok(taken) => {
                entry.insert("pixelWidth".into(), json!(taken.picture.0));
                entry.insert("pixelHeight".into(), json!(taken.picture.1));
                sizes.push(taken.json());
            }
            Err(error) => {
                entry.insert("status".into(), json!("failed"));
                entry.insert("error".into(), json!(error));
            }
        }
        shots.push(entry);
    }
    before.put_back(host);
    let failed = shots.iter().filter(|s| s.get("status").and_then(Value::as_str) == Some("failed")).count();
    let mut manifest = Map::new();
    manifest.insert("language".into(), json!(language));
    manifest.insert("appVersion".into(), json!(host.version));
    manifest.insert("frontEnd".into(), json!("web"));
    manifest.insert("shotCount".into(), json!(SHOTS.len()));
    manifest.insert("shots".into(), Value::Array(shots.iter().cloned().map(Value::Object).collect()));
    manifest.insert("failed".into(), json!(failed));
    manifest.insert("skipped".into(), json!(0));
    manifest.insert("passed".into(), json!(failed == 0));
    let manifest_path = output.join("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&Value::Object(manifest)).unwrap_or_default()).map_err(|e| e.to_string())?;
    let files: Vec<Value> = shots
        .iter()
        .map(|s| {
            let file = s.get("file").and_then(Value::as_str).unwrap_or("");
            json!({"file": file, "exists": output.join(file).is_file()})
        })
        .collect();
    Ok(json!({"shots": shots, "sizes": sizes, "files": files, "manifest": manifest_path, "language": language, "theme": theme}))
}

/// One shot as taken: the window asked for (points), its content and the screen (device pixels) and the picture's size.
struct Taken {
    asked: (f64, f64),
    scale: f64,
    content: (u32, u32),
    screen: Option<(u32, u32)>,
    picture: (u32, u32),
}

impl Taken {
    fn json(&self) -> Value {
        json!({"asked": [self.asked.0, self.asked.1], "scale": self.scale, "content": [self.content.0, self.content.1],
            "screen": self.screen.map(|s| [s.0, s.1]), "picture": [self.picture.0, self.picture.1]})
    }
}

/// The shot's window, its screen opened, the picture saved, the screen put back.
async fn take(host: &Arc<Host>, shot: &Value, screen: &str, file: &Path) -> Result<Taken, String> {
    let window = shot.get("window").ok_or("the shot has no window size")?;
    let asked = (window.get("width").and_then(Value::as_f64).unwrap_or(0.0), window.get("height").and_then(Value::as_f64).unwrap_or(0.0));
    help_capture::resize(host, asked.0, asked.1);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let opened = match screen {
        "sidebar-collapsed" => {
            set_sidebar(host, true)?;
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
        "dashboard" => {
            host.view().shows_dashboard = true;
            host.flush();
            if help_capture::until(host, "!!document.querySelector('[data-testid=dashboard]')", 10.0).await {
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(())
            } else {
                Err("the dashboard did not open".to_string())
            }
        }
        other => Err(format!("the smoke's step does not open {other}")),
    };
    let saved = match opened {
        Ok(()) => {
            tokio::time::sleep(Duration::from_millis(700)).await;
            let content = help_capture::content_size(host).unwrap_or((0, 0));
            let scale = window_now(host).map_or(1.0, |w| w.2);
            help_capture::save(host, None, file).await.map(|picture| Taken { asked, scale, content, screen: screen_now(host), picture })
        }
        Err(error) => Err(error),
    };
    match screen {
        "sidebar-collapsed" => set_sidebar(host, false)?,
        _ => {
            host.view().shows_dashboard = false;
            host.flush();
        }
    }
    saved
}

fn set_sidebar(host: &Host, collapsed: bool) -> Result<(), String> {
    let result = host.view().update(|state| state.sidebar_collapsed = collapsed);
    host.flush();
    result
}

/// Whether a shot's picture is its window's content, and that content the shot's size wherever the screen holds it (a window may not
/// grow past its screen).
fn sized(size: &Value) -> bool {
    let pair = |key: &str| size.get(key).and_then(Value::as_array).map(|a| (a[0].as_f64().unwrap_or(-1.0), a[1].as_f64().unwrap_or(-1.0)));
    let (Some(asked), Some(content), Some(picture)) = (pair("asked"), pair("content"), pair("picture")) else { return false };
    let scale = size.get("scale").and_then(Value::as_f64).unwrap_or(1.0);
    let wanted = ((asked.0 * scale).round(), (asked.1 * scale).round());
    let room = pair("screen").is_none_or(|screen| screen.0 >= wanted.0 && screen.1 >= wanted.1);
    picture == content
        && picture.0 > 0.0
        && picture.1 > 0.0
        && (!room || (content.0 - wanted.0).abs() <= 1.0 && (content.1 - wanted.1).abs() <= 1.0)
}

/// The step's checks from its report.
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let shots = report.get("shots").and_then(Value::as_array).cloned().unwrap_or_default();
    let ok = |shot: &Value| shot.get("status").and_then(Value::as_str) == Some("ok");
    let files = report.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
    let sizes = report.get("sizes").and_then(Value::as_array).cloned().unwrap_or_default();
    let listed: Vec<&str> = shots.iter().filter_map(|s| s.get("id").and_then(Value::as_str)).collect();
    vec![
        (
            "helpShotsTaken",
            report.get("error").is_none()
                && files.len() == SHOTS.len()
                && files.iter().all(|f| f.get("exists") == Some(&Value::Bool(true))),
        ),
        ("helpShotSize", sizes.len() == SHOTS.len() && sizes.iter().all(sized)),
        (
            "helpManifest",
            listed == SHOTS
                && shots.iter().all(ok)
                && report.get("manifest").and_then(Value::as_str).is_some_and(|p| Path::new(p).is_file()),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(asked: (f64, f64), scale: f64, content: (u32, u32), screen: Option<(u32, u32)>, picture: (u32, u32)) -> Value {
        Taken { asked, scale, content, screen, picture }.json()
    }

    /// A picture is the window's content; that content is the shot's size at the screen's scale unless the screen is smaller.
    #[test]
    fn a_shot_is_sized_as_asked_where_the_screen_holds_it() {
        assert!(sized(&size((1440.0, 900.0), 1.5, (2160, 1350), Some((3840, 2160)), (2160, 1350))));
        assert!(!sized(&size((1440.0, 900.0), 1.5, (2160, 1350), Some((3840, 2160)), (2160, 1349))), "the picture is not the content");
        assert!(!sized(&size((1440.0, 900.0), 1.0, (1200, 800), Some((1920, 1080)), (1200, 800))), "the screen held the asked size");
        assert!(
            sized(&size((1440.0, 900.0), 1.0, (1024, 740), Some((1024, 768)), (1024, 740))),
            "a smaller screen keeps the window smaller"
        );
        assert!(!sized(&size((1440.0, 900.0), 1.0, (0, 0), None, (0, 0))));
    }

    #[test]
    fn a_step_that_failed_fails_every_check() {
        assert!(checks(&json!({"error": "no main window"})).iter().all(|(_, passed)| !passed));
        assert_eq!(
            checks(&json!({})).iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            ["helpShotsTaken", "helpShotSize", "helpManifest"]
        );
    }

    /// Both shots are in shots.json, whole-page (a window, no popover) and drawn by the capture.
    #[test]
    fn the_steps_shots_are_whole_page_shots_of_the_list() {
        let list: Value = serde_json::from_str(help_capture::SHOTS).unwrap();
        for id in SHOTS {
            let shot = list["shots"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap();
            assert!(shot["window"]["width"].is_number() && shot["window"]["height"].is_number());
            assert!(help_capture::KNOWN_SCREENS.contains(&shot["screen"].as_str().unwrap()));
        }
    }
}
