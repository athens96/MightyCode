//! The help site's pictures (C# `WinUI\MainWindow.HelpCapture.cs`, `MainWindow.HelpCaptureDelegation.cs`; plan §1.8 "Help capture",
//! D16, W9-H). Owner: W9-H.
//!
//! `--help-capture --profile <empty> [-language ko|en|zh|ja] [--smoke-exit]` runs in the smoke's place (args.rs makes it a smoke, in an
//! empty profile of its own; main.rs reads the page in the capture's language). It reads `docs/help/shots.json` and the demo profile
//! (`native/contracts/fixtures/demo-profile.json`, both embedded), takes the welcome shot first on the empty profile, then seeds the
//! demo's workspaces (folders and files under `<profile>\Projects`), panes, runs, drafts, layouts and the shell pane's terminal, and for
//! each shot sets the window's content size, drives the page to the shot's screen and saves the page's picture as
//! `<profile>\help-shots\<lang>\<id>-<theme>.png` for each theme (light and dark when the shot lists none), beside a `manifest.json` with
//! every shot's status and `<profile>\smoke-result.json` (C#'s shapes and key order). A popover shot is cut out of the page by its element's
//! bounds (the DevTools protocol's `Page.captureScreenshot` at the page's scale); a whole-page shot is WebView2's `CapturePreview`. The
//! delegation demo is seeded on the first delegation shot. `MIGHTY_HELP_SHOTS=<id>,<id>…` takes only those shots (the rest are listed as
//! skipped); `MIGHTY_HELP_DOM=1` keeps each shot's markup beside its picture.
//!
//! The runs are the profile's canned CLI lines played by this exe in the CLI's place (`--help-demo-cli`, mighty-delegation
//! `help_demo_cli`) through the real run path, so the panes, cards and diagrams show what a real run would. No real CLI starts (runs.rs
//! finds none in this run; the demo's `claude` and `codex` are overrides naming this exe) and nothing goes on the network; the workspaces
//! are folders under the profile and the "continue a session" records live under the profile's own history home. The pet's picture is
//! one pet window's own buffer (as the companion smoke's test overlay), since a capture, as any smoke, never puts the app's pet on the
//! desktop. The pictures need WebView2 (Windows); elsewhere every shot fails with that reason. The help site keeps the Mac's pictures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_core::model::{AgentRunTiming, RunSession, RunSettings};
use mighty_core::time::DateTimeOffset;
use mighty_core::wire;
use mighty_run::model::{CliCommand, StartRunRequest};
use serde_json::{Map, Value, json};
use tauri::Manager;

use crate::host::{Host, View, WINDOW};

/// The help site's shot list (C# embeds it as `MightyCode.WinUI.HelpShots.json`).
pub(crate) const SHOTS: &str = include_str!("../../../../docs/help/shots.json");
/// The demo profile (C#'s `MightyCode.WinUI.DemoProfile.json`).
const DEMO_PROFILE: &str = include_str!("../../../contracts/fixtures/demo-profile.json");

/// The screens this capture draws (C# `HelpKnownScreens`); any other is listed as skipped.
pub const KNOWN_SCREENS: [&str; 34] = [
    "welcome",
    "overview",
    "sidebar-collapsed",
    "dashboard",
    "agent-basic",
    "agent-mighty",
    "agent-mighty-overview",
    "agent-timeline",
    "agent-structure",
    "result-card",
    "composer",
    "composer-settings",
    "session-info",
    "plan-card",
    "plan-document",
    "question-card",
    "permission-card",
    "background-work",
    "terminal-pane",
    "files-pane",
    "companion-pet",
    "resume-choice",
    "resume-list",
    "settings-general",
    "settings-models",
    "settings-styles",
    "settings-tools",
    "settings-cli",
    "settings-mobile",
    "settings-companion",
    "settings-about",
    "delegation-tree",
    "delegation-held",
    "delegation-parent-closed",
];

/// The version the canned Claude says it is (C# `CliCommand(…, "2.1.271")`).
const CLAUDE_VERSION: &str = "2.1.271";

/// Called by the smoke once the page is up (smoke.rs) in a `--help-capture` run: the capture takes over from the smoke's probe.
pub fn begin(host: &Host, language: &str) -> bool {
    let Some(shared) = host.app.try_state::<Arc<Host>>().map(|h| h.inner().clone()) else { return false };
    host.log.line(&format!("help capture: {language}"));
    let language = language.to_string();
    let task = tauri::async_runtime::spawn(async move { run(shared, language).await });
    let watcher = host.app.try_state::<Arc<Host>>().map(|h| h.inner().clone());
    tauri::async_runtime::spawn(async move {
        // A panic ends the run as failed, with what it said in the log (the task alone would end silently and the app wait forever).
        if let (Err(error), Some(host)) = (task.await, watcher) {
            host.log.line(&format!("help capture: the run panicked: {error}"));
            if let Some(smoke) = &host.smoke {
                smoke.end(&host, false);
            }
        }
    });
    true
}

// ── the page ──

/// C# `ExecuteScriptAsync`: the script's value as JSON text (`"true"`, `null`, a quoted string…), at most 30 s.
pub(crate) async fn js(host: &Host, script: &str) -> Result<String, String> {
    let page = host.app.get_webview(WINDOW).ok_or("no main window")?;
    let (sender, answer) = tokio::sync::oneshot::channel();
    let sender = Mutex::new(Some(sender));
    page.eval_with_callback(script, move |value| {
        let taken = sender.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(sender) = taken {
            let _ = sender.send(value);
        }
    })
    .map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(30), answer)
        .await
        .map_err(|_| "the page did not answer a script".to_string())?
        .map_err(|e| e.to_string())
}

/// C# `HelpTrue`.
pub(crate) async fn js_true(host: &Host, script: &str) -> bool {
    js(host, script).await.is_ok_and(|value| value == "true")
}

/// C# `HelpUntil`: the script true within `seconds`, asked every 100 ms.
pub(crate) async fn until(host: &Host, script: &str, seconds: f64) -> bool {
    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < deadline {
        if js_true(host, script).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// C# `HelpWait`: `done` true within `seconds`, asked every 100 ms.
pub(crate) async fn wait(seconds: f64, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn delay(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// A JSON string literal of `text` for a script (C# `JsonSerializer.Serialize(text)`).
pub(crate) fn literal(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

/// A PNG's width and height: the IHDR's first two big-endian words (C# `HelpPngSize`).
pub(crate) fn png_size(bytes: &[u8]) -> (u32, u32) {
    if bytes.len() > 24 {
        let word = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        (word(16), word(20))
    } else {
        (0, 0)
    }
}

/// The page's picture as PNG bytes: the whole page (WebView2's `CapturePreview`), or with `clip` only that element's box, cut out by the
/// DevTools protocol at the page's scale (C# `HelpSave`).
#[cfg(windows)]
async fn picture(host: &Host, clip: Option<&str>) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    let page = host.app.get_webview(WINDOW).ok_or("no main window")?;
    let Some(clip) = clip else { return mighty_platform_windows::page::capture_preview(&page).await.map_err(|e| e.to_string()) };
    let script = format!(
        "(() => {{ const e = document.querySelector({}); if (!e) return null; const r = e.getBoundingClientRect(); return {{ x: r.left, y: r.top, width: r.width, height: r.height, scale: window.devicePixelRatio }}; }})()",
        literal(clip)
    );
    let r: Value = serde_json::from_str(&js(host, &script).await?).map_err(|e| e.to_string())?;
    if !r.is_object() {
        return Err("the element to capture is gone".into());
    }
    let number = |key: &str| r.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let arguments = json!({"format": "png", "clip": {"x": number("x"), "y": number("y"), "width": number("width"), "height": number("height"), "scale": number("scale")}});
    let answer = mighty_platform_windows::page::devtools(&page, "Page.captureScreenshot", &arguments.to_string())
        .await
        .map_err(|e| e.to_string())?;
    let answer: Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
    base64::engine::general_purpose::STANDARD.decode(answer.get("data").and_then(Value::as_str).unwrap_or("")).map_err(|e| e.to_string())
}

#[cfg(not(windows))]
async fn picture(_host: &Host, _clip: Option<&str>) -> Result<Vec<u8>, String> {
    Err("the help capture pictures the page through WebView2 (Windows only); the help site keeps the Mac's pictures".into())
}

/// Still pictures, as the Mac's CI capture draws them: the page's reduced-motion rules (C#'s `Emulation.setEmulatedMedia`).
#[cfg(windows)]
async fn reduce_motion(host: &Host) -> Result<(), String> {
    let page = host.app.get_webview(WINDOW).ok_or("no main window")?;
    let features = r#"{"features":[{"name":"prefers-reduced-motion","value":"reduce"}]}"#;
    mighty_platform_windows::page::devtools(&page, "Emulation.setEmulatedMedia", features).await.map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(not(windows))]
async fn reduce_motion(_host: &Host) -> Result<(), String> {
    Ok(())
}

/// The page's picture saved to `path` (C# `HelpSave`): its pixel size.
pub(crate) async fn save(host: &Host, clip: Option<&str>, path: &Path) -> Result<(u32, u32), String> {
    let bytes = picture(host, clip).await?;
    std::fs::write(path, &bytes).map_err(|e| e.to_string())?;
    Ok(png_size(&bytes))
}

/// C# `HelpResize`: the window restored and its content at `width` × `height` points, at the screen's top left.
pub(crate) fn resize(host: &Host, width: f64, height: f64) {
    let Some(window) = host.app.get_window(WINDOW) else { return };
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    }
    let _ = window.set_size(tauri::LogicalSize::new(width, height));
    let _ = window.set_position(tauri::PhysicalPosition::new(0, 0));
}

/// The window's content size in device pixels now (what a whole-page picture measures).
pub(crate) fn content_size(host: &Host) -> Option<(u32, u32)> {
    host.app.get_window(WINDOW).and_then(|w| w.inner_size().ok()).map(|s| (s.width, s.height))
}

/// A change of the window's state (C#'s `UpdateAsync` or a layout action, then `Render`).
fn change(host: &Host, act: impl FnOnce(&mut View) -> Result<(), String>) -> Result<(), String> {
    let result = {
        let mut view = host.view();
        act(&mut view)
    };
    host.flush();
    result
}

fn update(host: &Host, edit: impl FnOnce(&mut mighty_core::AppSnapshot)) -> Result<(), String> {
    change(host, |view| view.update(edit))
}

fn select_workspace(host: &Host, id: &str) -> Result<(), String> {
    change(host, |view| crate::frame::select_workspace(view, id))
}

fn select_session(host: &Host, id: &str) -> Result<(), String> {
    change(host, |view| crate::frame::select_session(view, id))
}

fn apply_preset(host: &Host, preset: &str) -> Result<(), String> {
    change(host, |view| crate::frame::apply_layout_preset(view, preset))
}

fn hide_dashboard(host: &Host) {
    host.view().shows_dashboard = false;
    host.flush();
}

/// A pane as it is now (the run service's copy).
fn pane(host: &Host, id: &str) -> Option<Arc<RunSession>> {
    host.current().session(id).cloned()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn errored<T>(why: impl Into<String>) -> Result<T, String> {
    Err(why.into())
}

// ── the run ──

/// The seeded demo (C# `HelpDemo`): the profile in the capture's language, and the app's ids of its workspaces and panes by the
/// profile's ids and roles.
struct Demo {
    profile: Value,
    workspaces: HashMap<String, String>,
    panes: HashMap<String, String>,
    terminal_problem: Option<String>,
    delegation_seeded: bool,
    delegation_problem: Option<String>,
    /// The delegation panes' runs: how long each ran, and whether it finished.
    delegation_timings: HashMap<String, (f64, bool)>,
}

impl Demo {
    fn new(profile: Value) -> Demo {
        Demo {
            profile,
            workspaces: HashMap::new(),
            panes: HashMap::new(),
            terminal_problem: None,
            delegation_seeded: false,
            delegation_problem: None,
            delegation_timings: HashMap::new(),
        }
    }

    fn pane(&self, role: &str) -> Result<String, String> {
        self.panes.get(role).cloned().ok_or_else(|| format!("the demo has no {role} pane"))
    }

    fn delegation_workspace(&self) -> Option<&str> {
        self.profile.get("delegation").and_then(|d| d.get("workspace")).and_then(Value::as_str)
    }

    /// The layouts but the delegation workspace's.
    fn layouts(&self) -> Vec<Value> {
        let later = self.delegation_workspace();
        list(&self.profile, "layouts").iter().filter(|l| l.get("workspace").and_then(Value::as_str) != later).cloned().collect()
    }

    fn workspace(&self, key: &str) -> Result<String, String> {
        self.workspaces.get(key).cloned().ok_or_else(|| format!("the demo has no {key} workspace"))
    }
}

/// What one screen shows (C# `HelpTarget`): the element to cut out (none: the whole page), how to put things back, and its own way of
/// saving its picture (the pet's).
#[derive(Default)]
struct Target {
    clip: Option<&'static str>,
    cleanup: Option<Cleanup>,
    pet: Option<PetPicture>,
}

enum Cleanup {
    SidebarExpanded,
    Dashboard,
    Escape(u64),
    PlanDocument,
    Settings,
    Dialog,
    FilesPane(Option<String>),
    Delegation { screen: String, expanded: Option<Vec<String>> },
}

/// C# `RunHelpCapture`.
async fn run(host: Arc<Host>, language: String) {
    let directory = host.data_dir.clone();
    let output = directory.join("help-shots").join(&language);
    let mut manifest = Map::new();
    manifest.insert("language".into(), json!(language));
    manifest.insert("appVersion".into(), json!(host.version));
    manifest.insert("frontEnd".into(), json!("web"));
    let mut shots: Vec<Map<String, Value>> = vec![];
    if let Err(error) = capture(&host, &language, &directory, &output, &mut manifest, &mut shots).await {
        manifest.insert("error".into(), json!(error));
    }
    let count = |status: &str| shots.iter().filter(|s| s.get("status").and_then(Value::as_str) == Some(status)).count();
    let failed = count("failed");
    let skipped = count("skipped");
    let passed = !manifest.contains_key("error") && failed == 0 && count("ok") > 0;
    manifest.insert("shots".into(), Value::Array(shots.into_iter().map(Value::Object).collect()));
    manifest.insert("failed".into(), json!(failed));
    manifest.insert("skipped".into(), json!(skipped));
    manifest.insert("passed".into(), json!(passed));
    let result = json!({"passed": passed, "helpCapture": true, "aiRequestSent": false, "clipboardUsed": false, "failed": failed,
        "error": manifest.get("error").cloned().unwrap_or(Value::Null)});
    let out = host.smoke.as_ref().map_or_else(|| directory.join("smoke-result.json"), |s| s.out().to_path_buf());
    let written = std::fs::create_dir_all(&output)
        .and_then(|()| {
            std::fs::write(output.join("manifest.json"), serde_json::to_vec_pretty(&Value::Object(manifest)).unwrap_or_default())
        })
        .and_then(|()| std::fs::write(&out, serde_json::to_vec_pretty(&result).unwrap_or_default()));
    if let Err(error) = written {
        host.log.line(&format!("help capture manifest: {error}"));
    }
    host.log.line(&format!("help capture: {} ({failed} failed, {skipped} skipped)", if passed { "passed" } else { "FAILED" }));
    // C# `CloseTerminalsAsync`, then `FinishSmoke(passed)`.
    if let Ok(terminals) = crate::terminal::terminals(&host) {
        let _ = tokio::time::timeout(Duration::from_secs(20), terminals.close_all()).await;
    }
    if let Some(smoke) = &host.smoke {
        smoke.end(&host, passed);
    }
}

/// The shots, in C#'s order: the welcome shot first, then the rest as listed.
async fn capture(
    host: &Arc<Host>,
    language: &str,
    directory: &Path,
    output: &Path,
    manifest: &mut Map<String, Value>,
    shots: &mut Vec<Map<String, Value>>,
) -> Result<(), String> {
    std::fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let list: Vec<Value> = serde_json::from_str::<Value>(SHOTS)
        .ok()
        .and_then(|v| v.get("shots").and_then(Value::as_array).cloned())
        .ok_or("shots.json lists no shots")?;
    let profile = mighty_core::demo::localized(&serde_json::from_str::<Value>(DEMO_PROFILE).map_err(|e| e.to_string())?, language);
    if !profile.is_object() {
        return errored("the demo profile is not an object");
    }
    manifest.insert("shotCount".into(), json!(list.len()));
    if !until(host, "!!document.querySelector('[data-testid=shell]')", 30.0).await {
        return errored("the page did not mount");
    }
    reduce_motion(host).await?;
    let ordered = list.iter().filter(|s| text(s, "screen") == "welcome").chain(list.iter().filter(|s| text(s, "screen") != "welcome"));
    let only = std::env::var("MIGHTY_HELP_SHOTS").ok().filter(|v| !v.is_empty());
    let mut demo: Option<Demo> = None;
    let mut seed_error: Option<String> = None;
    for shot in ordered {
        let screen = text(shot, "screen").to_string();
        let themes: Vec<String> = match list_of_strings(shot.get("themes")) {
            Some(listed) if !listed.is_empty() => listed,
            _ => vec!["light".into(), "dark".into()],
        };
        let entry = |theme: &str, status: &str| entry(shot, &screen, theme, status);
        if let Some(only) = &only
            && !only.split(',').any(|id| id == text(shot, "id"))
        {
            for theme in &themes {
                let mut e = entry(theme, "skipped");
                e.insert("note".into(), json!("not in MIGHTY_HELP_SHOTS"));
                shots.push(e);
            }
            continue;
        }
        if !KNOWN_SCREENS.contains(&screen.as_str()) {
            for theme in &themes {
                let mut e = entry(theme, "skipped");
                e.insert("note".into(), json!("the Windows capture does not draw this screen yet"));
                shots.push(e);
            }
            continue;
        }
        if screen != "welcome" && demo.is_none() && seed_error.is_none() {
            match seed(host, Demo::new(profile.clone()), directory).await {
                Ok(seeded) => demo = Some(seeded),
                Err(error) => {
                    manifest.insert("error".into(), json!(format!("seeding the demo profile failed: {error}")));
                    seed_error = Some(error);
                }
            }
        }
        if screen != "welcome"
            && let Some(error) = &seed_error
        {
            for theme in &themes {
                let mut e = entry(theme, "failed");
                e.insert("error".into(), json!(format!("seeding the demo profile failed: {error}")));
                shots.push(e);
            }
            continue;
        }
        host.log.line(&format!("help capture: {}", text(shot, "id")));
        let taken = capture_shot(host, shot, &screen, &themes, demo.as_mut(), output).await;
        shots.extend(taken);
    }
    Ok(())
}

fn list_of_strings(value: Option<&Value>) -> Option<Vec<String>> {
    value.and_then(Value::as_array).map(|items| items.iter().map(|t| t.as_str().unwrap_or("").to_string()).collect())
}

/// C#'s `Entry(theme, status)`: one shot's row in the manifest, in C#'s key order.
pub(crate) fn entry(shot: &Value, screen: &str, theme: &str, status: &str) -> Map<String, Value> {
    let id = shot.get("id").cloned().unwrap_or(Value::Null);
    let mut e = Map::new();
    e.insert("id".into(), id.clone());
    e.insert("section".into(), shot.get("section").cloned().unwrap_or(Value::Null));
    e.insert("screen".into(), json!(screen));
    e.insert("theme".into(), json!(theme));
    e.insert("file".into(), json!(format!("{}-{theme}.png", id.as_str().unwrap_or(""))));
    e.insert("status".into(), json!(status));
    e
}

/// C# `CaptureHelpShot`: the window sized, the screen opened, then a picture for each theme; the screen put back.
async fn capture_shot(
    host: &Arc<Host>,
    shot: &Value,
    screen: &str,
    themes: &[String],
    mut demo: Option<&mut Demo>,
    output: &Path,
) -> Vec<Map<String, Value>> {
    if let Some(window) = shot.get("window") {
        let size = |key: &str| window.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        resize(host, size("width"), size("height"));
        delay(400).await;
    }
    let target = match open_screen(host, screen, demo.as_deref_mut()).await {
        Ok(target) => target,
        Err(error) => {
            if let Some(demo) = demo.as_deref() {
                let _ = help_select(host, demo).await;
            }
            return themes
                .iter()
                .map(|theme| {
                    let mut e = entry(shot, screen, theme, "failed");
                    e.insert("error".into(), json!(error));
                    e
                })
                .collect();
        }
    };
    let id = text(shot, "id");
    let mut results = vec![];
    for theme in themes {
        let mut value = entry(shot, screen, theme, "ok");
        let file = output.join(format!("{id}-{theme}.png"));
        match picture_in_theme(host, &target, theme, &file, &mut value).await {
            Ok((width, height)) => {
                if std::env::var("MIGHTY_HELP_DOM").ok().as_deref() == Some("1") {
                    let script = format!("(document.querySelector({}) ?? document.body).outerHTML", literal(target.clip.unwrap_or("body")));
                    if let Ok(markup) = js(host, &script).await {
                        let markup: String = serde_json::from_str(&markup).unwrap_or_default();
                        let _ = std::fs::write(output.join(format!("{id}-{theme}.html")), markup);
                    }
                }
                value.insert("pixelWidth".into(), json!(width));
                value.insert("pixelHeight".into(), json!(height));
            }
            Err(error) => {
                value.insert("status".into(), json!("failed"));
                value.insert("error".into(), json!(error));
            }
        }
        results.push(value);
    }
    if let Some(cleanup) = target.cleanup
        && let Err(error) = clean_up(host, cleanup, demo.as_deref()).await
    {
        host.log.line(&format!("help capture cleanup: {error}"));
    }
    if let Some(demo) = demo.as_deref() {
        let _ = help_select(host, demo).await;
    }
    results
}

/// One theme's picture: the theme taken by the page, a banner cleared, the picture saved; its pixel size.
async fn picture_in_theme(
    host: &Host,
    target: &Target,
    theme: &str,
    file: &Path,
    value: &mut Map<String, Value>,
) -> Result<(u32, u32), String> {
    if host.current().theme() != theme {
        update(host, |state| state.theme = Some(theme.to_string()))?;
    }
    if !until(host, &format!("document.documentElement.dataset.theme === {}", literal(theme)), 10.0).await {
        return errored(format!("the page did not take the {theme} theme"));
    }
    delay(700).await;
    let banner = host.view().error.take();
    if let Some(message) = banner.filter(|m| !m.is_empty()) {
        value.insert("clearedError".into(), json!(message));
        host.flush();
        delay(300).await;
    }
    match &target.pet {
        Some(pet) => pet.save(host, file).await,
        None => save(host, target.clip, file).await,
    }
}

/// C# `HelpDraft`: a pane's draft as typed in its composer, so the page's editor and the composer's own rules both see it.
async fn help_draft(host: &Host, id: &str, text: &str) {
    if !pane(host, id).is_some_and(|p| matches!(p.kind(), "claude" | "shell")) {
        return;
    }
    if let Some(runs) = host.runs() {
        let _ = runs.service.update_draft(id, text);
        host.view().sync();
        host.flush();
    }
    let script = format!(
        "(() => {{ const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set; for (const e of document.querySelectorAll('[data-session-id=\"{id}\"] [data-testid=composer-input]')) {{ set.call(e, {}); e.dispatchEvent(new Event('input', {{ bubbles: true }})); }} }})()",
        literal(text)
    );
    let _ = js(host, &script).await;
}

// ── seeding ──

/// A folder listed as a workspace (approved for saving), as C#'s `AddWorkspaceAsync` lists it: its id.
fn add_workspace(host: &Host, folder: &Path) -> Result<String, String> {
    let workspace = host.approve_local(folder).map_err(|e| e.to_string())?;
    let id = workspace.id().to_string();
    update(host, |state| {
        if !state.workspaces().iter().any(|w| w.id() == id) {
            state.workspaces_mut().push(workspace.clone());
        }
    })?;
    Ok(id)
}

/// A demo workspace's folder with its files under `root`.
fn write_files(root: &Path, spec: &Value) -> Result<PathBuf, String> {
    let directory = spec.get("directory").and_then(Value::as_str).ok_or("a demo workspace has no folder")?;
    let folder = root.join(directory);
    for file in list(spec, "files") {
        let path = folder.join(text(file, "path").replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, text(file, "content")).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    Ok(folder)
}

/// A pane of the demo as C# makes it (`new RunSession { … }` / `Filled`).
fn filled(session: RunSession, spec: &Value) -> RunSession {
    let or = |key: &str, default: &str| spec.get(key).and_then(Value::as_str).unwrap_or(default).to_string();
    RunSession {
        kind: Some(or("kind", "claude")),
        provider: Some(or("provider", "claude")),
        model: Some(or("model", "default")),
        title: Some(spec.get("title").and_then(Value::as_str).unwrap_or(text(spec, "role")).to_string()),
        title_mode: Some("fixed".into()),
        settings: Some(RunSettings {
            effort: or("effort", "default"),
            permission_mode: or("permissionMode", "manual"),
            ..RunSettings::default()
        }),
        agent_view_mode: Some("mighty".into()),
        ..session
    }
}

/// C# `SeedHelpDemo`: the demo as the Mac's capture seeds it, the delegation workspace left out.
async fn seed(host: &Arc<Host>, mut demo: Demo, directory: &Path) -> Result<Demo, String> {
    let later = demo.delegation_workspace().map(str::to_string);
    let root = directory.join("Projects");
    let profile = demo.profile.clone();
    for spec in list(&profile, "workspaces").iter().filter(|w| w.get("id").and_then(Value::as_str) != later.as_deref()) {
        let folder = write_files(&root, spec)?;
        let id = add_workspace(host, &folder)?;
        demo.workspaces.insert(text(spec, "id").to_string(), id);
    }
    let panes: Vec<Value> =
        list(&profile, "panes").iter().filter(|p| p.get("workspace").and_then(Value::as_str) != later.as_deref()).cloned().collect();
    let mut sessions = vec![];
    for spec in &panes {
        let role = text(spec, "role").to_string();
        let session = filled(
            RunSession {
                id: Some(wire::new_id()),
                workspace_id: Some(demo.workspace(text(spec, "workspace"))?),
                draft: Some(text(spec, "draft").to_string()),
                ..RunSession::default()
            },
            spec,
        );
        demo.panes.insert(role, session.id().to_string());
        sessions.push(session);
    }
    let first_workspace = demo.workspace(text(&profile, "activeWorkspace"))?;
    write_resume_records(host, &demo, directory)?;
    // The shell pane is a real terminal, as the Mac's is; no status line runs the user's own command into a picture.
    if let (Ok(terminals), Some(shell)) =
        (crate::terminal::terminals(host), profile.get("terminal").map(|t| t.get("role").and_then(Value::as_str).unwrap_or("shell")))
        && let Some(id) = demo.panes.get(shell)
    {
        terminals.smoke_allow(id);
    }
    let first_session = sessions.first().map(|s| s.id().to_string());
    let expanded: Vec<String> = demo.workspaces.values().cloned().collect();
    update(host, |state| {
        state.status_line_enabled = false;
        state.sessions_mut().extend(sessions.into_iter().map(Arc::new));
        state.active_workspace_id = Some(first_workspace.clone());
        state.active_session_id = first_session.clone();
        state.expanded_workspace_ids = Some(expanded.clone());
    })?;

    // The runs: the canned CLI in the real CLIs' place, each run waited for until its last line is in.
    let scripts = directory.join("help-demo");
    std::fs::create_dir_all(&scripts).map_err(|e| e.to_string())?;
    let (runs, plays) = canned_runs(&panes, &demo.panes, &scripts);
    let script = scripts.join("script.json");
    std::fs::write(&script, json!({"runs": runs}).to_string()).map_err(|e| e.to_string())?;
    let engine = host.engine()?;
    let executable = std::env::current_exe().map_err(|e| format!("the app's own binary is not known: {e}"))?;
    for provider in list(&profile, "providers") {
        let id = text(provider, "id");
        if matches!(id, "claude" | "codex") {
            let version = if id == "claude" {
                Some(CLAUDE_VERSION.to_string())
            } else {
                provider.get("version").and_then(Value::as_str).map(str::to_string)
            };
            engine.catalog.set_override(id, Some(demo_cli(&executable, &script, version.as_deref())));
        }
    }
    let mut timings: HashMap<String, AgentRunTiming> = HashMap::new();
    for play in &plays {
        play_run(host, play).await?;
        if let Some(seconds) = play.run.get("elapsedSeconds").and_then(Value::as_f64) {
            timings.insert(play.id.clone(), timing(seconds, play.finish.is_some()));
        }
    }
    update(host, |state| {
        for session in state.sessions_mut() {
            if let Some(timing) = timings.get(session.id()) {
                *session = Arc::new(RunSession { run_timing: Some(*timing), ..(**session).clone() });
            }
        }
    })?;
    // The drafts as typed, so each composer's own rules (send or queue) see them too.
    for spec in &panes {
        let draft = text(spec, "draft");
        if !draft.is_empty()
            && let Some(id) = demo.panes.get(text(spec, "role")).cloned()
        {
            help_draft(host, &id, draft).await;
        }
    }
    for layout in demo.layouts() {
        select_workspace(host, &demo.workspace(text(&layout, "workspace"))?)?;
        select_session(host, &demo.pane(text(&layout, "active"))?)?;
        apply_preset(host, layout.get("preset").and_then(Value::as_str).unwrap_or("grid"))?;
    }
    help_select(host, &demo).await?;
    if let Err(error) = prepare_terminal(host, &demo).await {
        demo.terminal_problem = Some(error);
    }
    Ok(demo)
}

/// `CliCommand(executable, [HelpDemoCli.Argument, script], version)`.
fn demo_cli(executable: &Path, script: &Path, version: Option<&str>) -> CliCommand {
    CliCommand::new(
        executable.to_string_lossy().into_owned(),
        vec![mighty_delegation::help_demo_cli::ARGUMENT.into(), script.to_string_lossy().into_owned()],
        version,
    )
}

/// A run to play: its pane's role and id, the run's spec, its marker file and how it is meant to end.
struct Play {
    role: String,
    id: String,
    run: Value,
    marker: PathBuf,
    finish: Option<String>,
}

/// The canned CLI's script entries for `panes`' runs (those whose pane the demo has), and the plays in order.
fn canned_runs(panes: &[Value], ids: &HashMap<String, String>, scripts: &Path) -> (Vec<Value>, Vec<Play>) {
    let (mut runs, mut plays) = (vec![], vec![]);
    for spec in panes {
        let role = text(spec, "role");
        let Some(id) = ids.get(role) else { continue };
        for (index, run) in list(spec, "runs").iter().enumerate() {
            let marker = scripts.join(format!("{role}-{index}.done"));
            runs.push(json!({"provider": spec.get("provider").cloned().unwrap_or(Value::Null), "input": run.get("input").cloned().unwrap_or(Value::Null),
                "frames": run.get("frames").cloned().unwrap_or(Value::Null), "marker": marker.to_string_lossy()}));
            plays.push(Play {
                role: role.to_string(),
                id: id.clone(),
                run: run.clone(),
                marker,
                finish: run.get("finish").and_then(Value::as_str).map(str::to_string),
            });
        }
    }
    (runs, plays)
}

/// One run sent through the real run path and waited for: its marker (every line out), its end when it is meant to end (a `stopped`
/// one stopped first, as the delegation demo does), then a moment for the page.
async fn play_run(host: &Arc<Host>, play: &Play) -> Result<(), String> {
    let engine = host.engine()?;
    let current = pane(host, &play.id).ok_or_else(|| format!("the {} pane is missing", play.role))?;
    let mut request = StartRunRequest::new(current.id(), current.workspace_id(), current.kind(), text(&play.run, "input"));
    request.model = current.model().to_string();
    request.provider = current.provider().to_string();
    request.settings = current.settings.clone();
    request.resume_id = current.resume_id.clone();
    crate::delegation::send(&engine, request).await.map_err(|e| e.to_string())?;
    let marker = play.marker.clone();
    if !wait(30.0, || marker.exists()).await {
        return errored(format!("the {} pane's run did not play", play.role));
    }
    if play.finish.as_deref() == Some("stopped") {
        delay(300).await;
        let _ = engine.service.stop(&play.id).await;
    }
    if play.finish.is_some() && !wait(30.0, || pane(host, &play.id).is_some_and(|p| !matches!(p.status(), "running" | "idle"))).await {
        return errored(format!("the {} pane's run did not end", play.role));
    }
    delay(400).await;
    Ok(())
}

/// A run's clock as the profile gives it, read from now: started `seconds` ago, last seen now, finished now when it finished.
fn timing(seconds: f64, finished: bool) -> AgentRunTiming {
    let now = DateTimeOffset::now();
    let started = DateTimeOffset::from_utc_ticks(now.utc_ticks() - (seconds * 10_000_000.0) as i64);
    AgentRunTiming { started_at: started, last_observed_at: now, finished_at: finished.then_some(now), is_approximate: false }
}

/// C# `WriteHelpResumeRecords`: the Claude session records "continue a session" lists, under a history home in the profile that this
/// process alone reads.
fn write_resume_records(host: &Host, demo: &Demo, directory: &Path) -> Result<(), String> {
    let home = directory.join("help-demo").join("home");
    let now = chrono::Utc::now();
    for record in list(&demo.profile, "resumeSessions").iter().filter(|r| text(r, "provider") == "claude") {
        let workspace_path = {
            let id = demo.workspace(text(record, "workspace"))?;
            host.current().workspace(&id).map(|w| w.path().to_string()).ok_or("a resume record's workspace is missing")?
        };
        let folder = home.join(".claude").join("projects").join(mighty_history::history::claude_project_folder(&workspace_path));
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let (id, title) = (text(record, "sessionId"), text(record, "title"));
        let minutes = record.get("minutesAgo").and_then(Value::as_f64).unwrap_or(0.0);
        let modified = now - chrono::Duration::milliseconds((minutes * 60_000.0) as i64);
        let count = record.get("requests").and_then(Value::as_i64).unwrap_or(1).max(1);
        let mut lines = vec![];
        for index in 0..count {
            let stamp = (modified + chrono::Duration::seconds((index - count) * 90)).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
            lines.push(
                json!({"cwd": workspace_path, "message": {"content": title, "role": "user"}, "sessionId": id, "timestamp": stamp, "type": "user", "uuid": format!("u{index}-{id}")})
                    .to_string(),
            );
            lines.push(
                json!({"cwd": workspace_path, "message": {"content": [{"text": title, "type": "text"}], "model": "claude-sonnet-5-5", "role": "assistant"},
                    "sessionId": id, "timestamp": stamp, "type": "assistant", "uuid": format!("a{index}-{id}")})
                .to_string(),
            );
        }
        let file = folder.join(format!("{id}.jsonl"));
        std::fs::write(&file, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
        if let Ok(opened) = std::fs::File::options().write(true).open(&file) {
            let _ = opened.set_modified(std::time::SystemTime::from(modified));
        }
    }
    crate::history::of(host).set_home_override(Some(home));
    Ok(())
}

/// C# `PrepareHelpTerminal`: the demo command typed into the shell pane's terminal — a prompt named after the workspace and a command of
/// that name that prints the profile's canned output from a file, so nothing runs.
async fn prepare_terminal(host: &Arc<Host>, demo: &Demo) -> Result<(), String> {
    let spec = demo.profile.get("terminal").ok_or("the demo has no terminal")?;
    let id = demo.pane(spec.get("role").and_then(Value::as_str).unwrap_or("shell"))?;
    let name = {
        let current = host.current();
        let session = current.session(&id).ok_or("the pane is missing")?;
        current.workspace(session.workspace_id()).map(|w| w.name().to_string()).unwrap_or_default()
    };
    select_session(host, &id)?;
    let opened = format!("!!document.querySelector('[data-session-id=\"{id}\"] .xterm, .pane-slot.active .xterm')");
    if !until(host, &opened, 20.0).await {
        return errored("the terminal did not open");
    }
    let file = host.data_dir.join("help-demo").join("terminal-output.txt");
    let output: Vec<&str> = list(spec, "output").iter().map(|l| l.as_str().unwrap_or("")).collect();
    std::fs::write(&file, output.join("\r\n") + "\r\n").map_err(|e| e.to_string())?;
    let command = text(spec, "command");
    let program = command.split(' ').next().unwrap_or("");
    let plain = program.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && program.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !plain {
        return errored("the terminal command needs a plain program name");
    }
    let terminals = crate::terminal::terminals(host)?;
    let terminal = terminals.pane(host, Some(&id))?;
    if !wait(20.0, || terminal.is_running()).await {
        return errored("the terminal's shell did not start");
    }
    let shell = terminals.default_shell();
    let powershell = shell.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("powershell"));
    let file_text = file.to_string_lossy();
    // A prompt like the workspace's, a function (or macro) of the command's name that types the canned output, and a cleared screen.
    let setup = if powershell {
        format!(
            "Remove-Module PSReadLine -ErrorAction SilentlyContinue; function global:prompt {{ '{name} > ' }}; function global:{program} {{ [Console]::Write([IO.File]::ReadAllText('{}')) }}; Clear-Host\r",
            file_text.replace('\'', "''")
        )
    } else {
        format!("prompt {name} $G $S& doskey {program}=type \"{file_text}\"& cls\r")
    };
    terminal.write(&setup);
    delay(1200).await;
    terminal.write(&format!("{command}\r"));
    let marker = output.iter().map(|l| l.trim()).rfind(|l| !l.is_empty()).unwrap_or("");
    let shown = format!("(document.querySelector('.pane-slot.active .xterm-rows')?.textContent ?? '').includes({})", literal(marker));
    if !until(host, &shown, 10.0).await {
        return errored("the terminal output did not show");
    }
    Ok(())
}

/// C# `HelpSelect`: back to the overview — the active workspace with its layout's pane selected, every pane in the Mighty view with its
/// draft.
async fn help_select(host: &Arc<Host>, demo: &Demo) -> Result<(), String> {
    hide_dashboard(host);
    for spec in list(&demo.profile, "panes") {
        let Some(id) = demo.panes.get(text(spec, "role")) else { continue };
        let Some(session) = pane(host, id) else { continue };
        let draft = text(spec, "draft");
        if session.agent_view_mode.as_deref() != Some("mighty") || session.graph_view_mode.is_some() {
            update(host, |state| {
                for each in state.sessions_mut().iter_mut().filter(|p| p.id() == id) {
                    *each = Arc::new(RunSession { agent_view_mode: Some("mighty".into()), graph_view_mode: None, ..(**each).clone() });
                }
            })?;
        }
        if session.draft.as_deref().unwrap_or("") != draft {
            help_draft(host, id, draft).await;
        }
    }
    let active = text(&demo.profile, "activeWorkspace");
    if let Some(layout) = demo.layouts().into_iter().find(|l| text(l, "workspace") == active) {
        let workspace = demo.workspace(active)?;
        select_workspace(host, &workspace)?;
        select_session(host, &demo.pane(text(&layout, "active"))?)?;
        let preset = layout.get("preset").and_then(Value::as_str).unwrap_or("grid");
        if mighty_core::shell::layout_mode(&host.current(), &workspace) != preset {
            apply_preset(host, preset)?;
        }
    }
    host.flush();
    delay(300).await;
    Ok(())
}

// ── screens ──

/// C# `OpenHelpScreen`.
async fn open_screen(host: &Arc<Host>, screen: &str, demo: Option<&mut Demo>) -> Result<Target, String> {
    if screen == "welcome" {
        if demo.is_some() || !host.current().workspaces().is_empty() {
            return errored("the welcome screen needs the empty profile");
        }
        delay(600).await;
        return Ok(Target::default());
    }
    let Some(demo) = demo else { return errored("the demo profile is not seeded") };
    match screen {
        "overview" => {
            help_select(host, demo).await?;
            Ok(Target::default())
        }
        "sidebar-collapsed" => {
            update(host, |state| state.sidebar_collapsed = true)?;
            delay(500).await;
            Ok(Target { cleanup: Some(Cleanup::SidebarExpanded), ..Target::default() })
        }
        "dashboard" => {
            host.view().shows_dashboard = true;
            host.flush();
            if !until(host, "!!document.querySelector('[data-testid=dashboard]')", 10.0).await {
                return errored("the dashboard did not open");
            }
            delay(500).await;
            Ok(Target { cleanup: Some(Cleanup::Dashboard), ..Target::default() })
        }
        "agent-basic"
        | "agent-mighty"
        | "agent-mighty-overview"
        | "agent-timeline"
        | "agent-structure"
        | "result-card"
        | "composer"
        | "plan-card"
        | "question-card"
        | "permission-card"
        | "background-work"
        | "terminal-pane" => {
            let role = match screen {
                "agent-basic" => "codex",
                "plan-card" => "plan",
                "question-card" => "question",
                "permission-card" => "permission",
                "terminal-pane" => "shell",
                "result-card" => "result",
                _ => "mighty",
            };
            let id = demo.pane(role)?;
            check_screen(host, screen, &id, demo)?;
            // As the Mac's capture: the timeline or the structure view.
            let view = match screen {
                "agent-timeline" => Some("timeline"),
                "agent-structure" => Some("structure"),
                _ => None,
            };
            if let Some(view) = view {
                update(host, |state| {
                    for each in state.sessions_mut().iter_mut().filter(|p| p.id() == id) {
                        *each = Arc::new(RunSession { graph_view_mode: Some(view.into()), ..(**each).clone() });
                    }
                })?;
            }
            select_session(host, &id)?;
            apply_preset(host, "focus")?;
            // The diagram aims at the next-request block while a draft is typed; the Mac's capture empties it for the screens that show
            // the requests.
            let shows_requests =
                matches!(screen, "agent-mighty" | "agent-mighty-overview" | "agent-timeline" | "agent-structure" | "background-work");
            if shows_requests {
                delay(400).await;
                help_draft(host, &id, "").await;
            }
            host.flush();
            delay(if shows_requests || screen == "result-card" { 1500 } else { 900 }).await;
            Ok(Target::default())
        }
        "composer-settings" => {
            focus_pane(host, &demo.pane("mighty")?, 600).await?;
            if !js_true(host, "(() => { const b = document.querySelector('.pane-slot.active:not(.hidden) [data-pill=more]'); if (!b) return false; b.click(); return true; })()").await {
                return errored("the pane has no run settings button");
            }
            if !until(host, "!!document.querySelector('[data-testid=run-settings]')", 6.0).await {
                return errored("the run settings did not open");
            }
            delay(500).await;
            Ok(Target { clip: Some("[data-testid=run-settings]"), cleanup: Some(Cleanup::Escape(200)), ..Target::default() })
        }
        "plan-document" => {
            focus_pane(host, &demo.pane("plan")?, 800).await?;
            let expand = "(() => { const b = document.querySelector('.pane-slot.active:not(.hidden) [data-answer=plan-expand], .pane-slot.active:not(.hidden) [data-testid=mighty-plan-expand]'); if (!b) return false; b.click(); return true; })()";
            if !until(host, expand, 6.0).await {
                return errored("the plan card has no expand button");
            }
            if !until(host, "!!document.querySelector('[data-testid=plan-document]')", 6.0).await {
                return errored("the plan document did not open");
            }
            delay(500).await;
            Ok(Target { clip: Some("[data-testid=plan-document]"), cleanup: Some(Cleanup::PlanDocument), ..Target::default() })
        }
        "session-info" => {
            focus_pane(host, &demo.pane("mighty")?, 600).await?;
            if !js_true(host, "(() => { const r = document.querySelector('.pane-slot.active:not(.hidden) [data-testid=context-ring]'); if (!r) return false; r.click(); return true; })()").await {
                return errored("the pane has no context ring");
            }
            if !until(host, "!!document.querySelector('[data-testid=session-info]')", 6.0).await {
                return errored("the session details did not open");
            }
            delay(500).await;
            Ok(Target { clip: Some("[data-testid=session-info]"), cleanup: Some(Cleanup::Escape(200)), ..Target::default() })
        }
        "settings-general" | "settings-models" | "settings-styles" | "settings-tools" | "settings-cli" | "settings-mobile"
        | "settings-companion" | "settings-about" => {
            // As the Mac's: the saved category, then Settings opened; the sheet alone is the picture.
            let category = &screen["settings-".len()..];
            open_settings(host, category).await?;
            Ok(Target { clip: Some("[data-testid=native-settings]"), cleanup: Some(Cleanup::Settings), ..Target::default() })
        }
        "resume-choice" | "resume-list" => {
            // Add Pane → Claude in the active workspace, which has earlier sessions: the choice, and for the list its Continue.
            help_select(host, demo).await?;
            let workspace = demo.workspace(text(&demo.profile, "activeWorkspace"))?;
            let add = format!(
                "(() => {{ const b = document.querySelector('[data-testid=sidebar-workspace][data-workspace-id=\"{workspace}\"] [data-testid=sidebar-add]'); if (!b) return false; b.click(); return true; }})()"
            );
            if !until(host, &add, 10.0).await
                || !until(host, "(() => { const e = document.querySelector('[data-testid=\"add-entry-agent:claude\"]'); if (!e) return false; e.click(); return true; })()", 10.0).await
            {
                return errored("the add menu has no Claude entry");
            }
            if !until(host, "!!document.querySelector('[data-testid=resume-choice-resume]:not([disabled])')", 10.0).await {
                return errored("the resume choice did not open");
            }
            if screen == "resume-list" {
                let _ = js(host, "document.querySelector('[data-testid=resume-choice-resume]')?.click()").await;
                if !until(host, "document.querySelectorAll('[data-testid=resume-rows] .resume-row').length > 0", 10.0).await {
                    return errored("the session list shows no record");
                }
            }
            delay(600).await;
            Ok(Target { clip: Some("[data-testid=dialog] .dialog"), cleanup: Some(Cleanup::Dialog), ..Target::default() })
        }
        "companion-pet" => {
            // The pet is its own window, not the page: its picture is the one it last drew, with the permission pane's approval in its
            // bubble as the Mac's capture focuses it.
            let id = demo.pane("permission")?;
            let pet = PetPicture::open(host, &id).await?;
            delay(1200).await;
            Ok(Target { pet: Some(pet), ..Target::default() })
        }
        "files-pane" => {
            let workspace = demo.workspace(text(&demo.profile, "activeWorkspace"))?;
            change(host, |view| crate::files::open_files_pane(view, &host.locale, Some(&workspace)))?;
            // The files pane on its own first: a change of layout draws the pane again, which would drop a selection made before it.
            let pane_id = host.current().active_session_id.clone();
            apply_preset(host, "focus")?;
            delay(500).await;
            if !until(host, "!!document.querySelector('.pane-slot.active:not(.hidden) [data-testid=files-tree]')", 10.0).await {
                return errored("the files pane did not open");
            }
            let click = |name: &str| {
                format!(
                    "(() => {{ const row = [...document.querySelectorAll('.pane-slot.active:not(.hidden) [data-testid=files-tree] [data-path]')].find(e => e.dataset.path === {}); if (!row) return false; row.click(); return true; }})()",
                    literal(name)
                )
            };
            if !until(host, &click("src"), 10.0).await {
                return errored("the files tree has no src folder");
            }
            delay(600).await;
            if !until(host, &click("README.md"), 10.0).await {
                return errored("the files tree has no README.md");
            }
            if !until(host, "!!document.querySelector('.pane-slot.active:not(.hidden) [data-testid=files-preview] .markdown')", 10.0).await
            {
                return errored("the README preview did not show");
            }
            delay(500).await;
            Ok(Target { cleanup: Some(Cleanup::FilesPane(pane_id)), ..Target::default() })
        }
        "delegation-tree" | "delegation-held" | "delegation-parent-closed" => open_delegation(host, screen, demo).await,
        other => errored(format!("unknown screen {other}")),
    }
}

/// The pane selected on its own, then a moment for the page.
async fn focus_pane(host: &Host, id: &str, ms: u64) -> Result<(), String> {
    select_session(host, id)?;
    apply_preset(host, "focus")?;
    delay(ms).await;
    Ok(())
}

/// Settings opened on `category` (the saved category first), waited for until its form shows (C#'s settings screens).
pub(crate) async fn open_settings(host: &Arc<Host>, category: &str) -> Result<(), String> {
    update(host, |state| state.settings_pane = Some(category.to_string()))?;
    if !crate::settings::select(host, category) {
        crate::settings::open(host, Some(category));
    }
    let shown = format!("!!document.querySelector('[data-testid=settings-form][data-category={category}]')");
    if !until(host, &shown, 10.0).await {
        return errored(format!("Settings did not open on {category}"));
    }
    delay(900).await;
    Ok(())
}

async fn clean_up(host: &Arc<Host>, cleanup: Cleanup, demo: Option<&Demo>) -> Result<(), String> {
    match cleanup {
        Cleanup::SidebarExpanded => update(host, |state| state.sidebar_collapsed = false),
        Cleanup::Dashboard => {
            hide_dashboard(host);
            Ok(())
        }
        Cleanup::Escape(ms) => {
            js(host, "document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))").await?;
            delay(ms).await;
            Ok(())
        }
        Cleanup::PlanDocument => {
            js(host, "document.querySelector('[data-testid=plan-document-close]')?.click()").await?;
            delay(200).await;
            Ok(())
        }
        Cleanup::Settings => {
            crate::settings::close(host);
            delay(300).await;
            Ok(())
        }
        Cleanup::Dialog => {
            js(host, "document.querySelector('[data-testid=dialog] .dialog')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))").await?;
            delay(400).await;
            Ok(())
        }
        Cleanup::FilesPane(pane_id) => {
            if let Some(id) = pane_id.filter(|id| pane(host, id).is_some_and(|p| !matches!(p.kind(), "claude" | "shell"))) {
                crate::frame::close_session(host, &id).await?;
            }
            let Some(demo) = demo else { return Ok(()) };
            let active = text(&demo.profile, "activeWorkspace");
            for layout in demo.layouts().into_iter().filter(|l| text(l, "workspace") == active) {
                select_session(host, &demo.pane(text(&layout, "active"))?)?;
                apply_preset(host, layout.get("preset").and_then(Value::as_str).unwrap_or("grid"))?;
            }
            Ok(())
        }
        Cleanup::Delegation { screen, expanded } => {
            if screen == "delegation-held"
                && let Some(demo) = demo
                && let Some(layout) = list(&demo.profile, "layouts")
                    .iter()
                    .find(|l| l.get("workspace").and_then(Value::as_str) == demo.delegation_workspace())
            {
                apply_preset(host, layout.get("preset").and_then(Value::as_str).unwrap_or("tabs"))?;
            }
            update(host, |state| state.expanded_workspace_ids = expanded)
        }
    }
}

/// C# `CheckHelpScreen`: the seeded pane is in the state the screen is meant to show.
fn check_screen(host: &Host, screen: &str, id: &str, demo: &Demo) -> Result<(), String> {
    let session = pane(host, id).ok_or("the pane is missing")?;
    match screen {
        "agent-mighty" | "agent-mighty-overview" | "agent-timeline" | "agent-structure" | "result-card"
            if session.agent_view_mode.as_deref() != Some("mighty")
                || !session.graph_runs().iter().any(|r| r.status.as_deref() == Some("completed")) =>
        {
            errored("the Mighty pane has no finished request")
        }
        "background-work"
            if !session.background_work.as_ref().is_some_and(|w| {
                w.turn_ended && w.tasks.as_deref().unwrap_or_default().iter().any(|t| t.status.as_deref() == Some("running"))
            }) =>
        {
            errored("the pane has no background work")
        }
        "terminal-pane" if demo.terminal_problem.is_some() => {
            errored(format!("the terminal is not ready: {}", demo.terminal_problem.as_deref().unwrap_or_default()))
        }
        _ => Ok(()),
    }
}

// ── the pet ──

/// The pet's picture (C#'s `companionOverlay.SavePicture`): a pet window of its own (as the companion smoke's test overlay; a capture,
/// as any smoke, never shows the app's pet on the desktop) with the profile's preferences and chosen pet, the focused pane's card in its
/// bubble, its frame drawn into the window's own buffer.
struct PetPicture {
    window: Arc<dyn mighty_platform::PetWindow>,
    overlay: Mutex<mighty_companion::Companion>,
}

impl Drop for PetPicture {
    fn drop(&mut self) {
        let _ = self.window.close();
    }
}

impl PetPicture {
    async fn open(host: &Arc<Host>, pane_id: &str) -> Result<PetPicture, String> {
        let companion = crate::companion::app(host).ok_or("the pet is not shown")?;
        let surfaces = companion.surfaces().ok_or("the pet is not shown")?;
        let (preferences, chosen) = {
            let state = companion.state();
            let chosen = state.pets.iter().find(|p| p.id == state.prefs.selected_pet).or_else(|| state.pets.first()).cloned();
            (state.prefs.clone(), chosen)
        };
        if !preferences.enabled {
            return errored("the pet is not shown");
        }
        let atlas = match chosen {
            Some(pet) => tauri::async_runtime::spawn_blocking(move || {
                mighty_companion::pet::CompanionPet::load(&pet.source, &pet.id).and_then(|loaded| loaded.decode())
            })
            .await
            .map_err(|e| e.to_string())?
            .ok(),
            None => None,
        };
        let window: Arc<dyn mighty_platform::PetWindow> = Arc::from(surfaces.open_pet().await.map_err(|e| e.to_string())?);
        let mut overlay = mighty_companion::Companion::new(window.clone(), &preferences);
        overlay.set_atlas(atlas);
        let session = pane(host, pane_id).ok_or("the pane is missing")?;
        let permission = host.runs().and_then(|runs| runs.service.pending_permissions(pane_id).into_iter().find(|r| r.state == "pending"));
        let status = if permission.is_some() { "waiting" } else { session.status() };
        let card = crate::companion::card_for(host, pane_id, &session, status, permission.as_ref(), 0, 0, false);
        let light = host.current().theme() == "light";
        overlay.set_card(Some(mighty_companion::card::CompanionCard { light, ..card }), true);
        let picture = PetPicture { window, overlay: Mutex::new(overlay) };
        let scene = picture.overlay.lock().unwrap_or_else(|p| p.into_inner()).scene();
        picture.window.show(&scene).map_err(|e| e.to_string())?;
        Ok(picture)
    }

    /// C#'s `Save`: the frame drawn again (in the theme now), waited for until the window made it, and written as a PNG.
    async fn save(&self, host: &Host, path: &Path) -> Result<(u32, u32), String> {
        let scene = {
            let mut overlay = self.overlay.lock().unwrap_or_else(|p| p.into_inner());
            overlay.set_frame(0, 0, true);
            overlay.scene()
        };
        let before = self.window.capture().map(|c| c.frames).unwrap_or(0);
        self.window.draw(&scene).map_err(|e| e.to_string())?;
        let mut frame = None;
        let drawn = wait(10.0, || {
            frame = self.window.capture().ok().filter(|c| c.frames > before && !c.bgra.is_empty());
            frame.is_some()
        })
        .await;
        let frame = frame.filter(|_| drawn).ok_or("the pet has no picture")?;
        let png = mighty_companion::pet::png(frame.width, frame.height, frame.width * 4, &frame.bgra)?;
        std::fs::write(path, &png).map_err(|e| e.to_string())?;
        let _ = host;
        Ok(png_size(&png))
    }
}

// ── delegation ──

/// C# `OpenHelpDelegation`: the delegation demo seeded on the first delegation shot, then the screen's pane shown in the delegation
/// workspace with the other workspaces' lists closed.
async fn open_delegation(host: &Arc<Host>, screen: &str, demo: &mut Demo) -> Result<Target, String> {
    if let Some(problem) = &demo.delegation_problem {
        return errored(problem.clone());
    }
    if !demo.delegation_seeded {
        match seed_delegation(host, demo).await {
            Ok(()) => demo.delegation_seeded = true,
            Err(error) => {
                let problem = format!("seeding the delegation demo failed: {error}");
                demo.delegation_problem = Some(problem.clone());
                return errored(problem);
            }
        }
    }
    let space = demo.workspace(demo.delegation_workspace().unwrap_or_default())?;
    let id = demo.pane(if screen == "delegation-parent-closed" { "child-of-closed" } else { "parent" })?;
    check_delegation(host, screen, &id, demo)?;
    delegation_clocks(host, demo)?;
    let expanded = host.current().expanded_workspace_ids.clone();
    select_workspace(host, &space)?;
    // The other workspaces' lists are closed, so the tree has the sidebar to itself.
    update(host, |state| state.expanded_workspace_ids = Some(vec![space.clone()]))?;
    select_session(host, &id)?;
    // The held notices with the parent pane on its own; the tree with the pane's tab selected.
    if screen == "delegation-held" {
        apply_preset(host, "focus")?;
    }
    host.flush();
    if !until(host, &format!("!!document.querySelector('[data-session-id=\"{id}\"]')"), 10.0).await {
        return errored("the delegation pane did not show");
    }
    delay(1000).await;
    Ok(Target { cleanup: Some(Cleanup::Delegation { screen: screen.to_string(), expanded }), ..Target::default() })
}

/// C# `SeedHelpDelegation`: the delegation workspace, its parent pane, the open children opened beside it the way delegate opens them,
/// their canned runs played through the real run path, then the profile's delegation records saved as the profile's delegation file and
/// loaded into the app's delegation, so the sidebar's tree, the children's cards and the queued lists show them.
async fn seed_delegation(host: &Arc<Host>, demo: &mut Demo) -> Result<(), String> {
    use mighty_delegation::copy::{DelegationCopy, DelegationCopyKind};
    use mighty_delegation::host::{DelegationChildPane, DelegationHost as _};
    use mighty_delegation::model::{
        ChildRecord, ChildRecordOptions, ChildState, ChildStates as _, DeliveryLane, DeliveryReceipt, DeliveryRoute, MergeKind,
        MergeRecord, Notice, NoticeKind, request_key,
    };
    use mighty_delegation::request::DelegationName as _;
    use mighty_delegation::store::{DelegationFile, DelegationFileStore};

    let app = crate::delegation::app().ok_or("no delegation")?;
    let directory = host.data_dir.clone();
    let profile = demo.profile.clone();
    let spec = profile.get("delegation").ok_or("the demo has no delegation")?;
    let space = demo.delegation_workspace().ok_or("the delegation demo has no workspace")?.to_string();
    let children: Vec<Value> = list(spec, "children").to_vec();
    let child_roles: Vec<&str> = children.iter().map(|c| text(c, "role")).collect();
    let workspace_spec =
        list(&profile, "workspaces").iter().find(|w| text(w, "id") == space).ok_or("the delegation demo has no workspace")?;

    // The workspace, as the other demo workspaces are made.
    let checkout = write_files(&directory.join("Projects"), workspace_spec)?;
    let workspace = add_workspace(host, &checkout)?;
    demo.workspaces.insert(space.clone(), workspace.clone());
    let panes: Vec<Value> = list(&profile, "panes").iter().filter(|p| text(p, "workspace") == space).cloned().collect();
    let pane_spec =
        |role: &str| panes.iter().find(|p| text(p, "role") == role).ok_or_else(|| format!("the delegation demo has no {role} pane"));

    // The parents' panes.
    let mut parents: HashMap<String, String> = HashMap::new();
    for spec in panes.iter().filter(|p| !child_roles.contains(&text(p, "role"))) {
        let session = filled(RunSession { id: Some(wire::new_id()), workspace_id: Some(workspace.clone()), ..RunSession::default() }, spec);
        let (role, id) = (text(spec, "role").to_string(), session.id().to_string());
        demo.panes.insert(role.clone(), id.clone());
        parents.insert(role, id.clone());
        update(host, |state| {
            state.sessions_mut().push(Arc::new(session.clone()));
            state.active_workspace_id = Some(workspace.clone());
            state.active_session_id = Some(id.clone());
        })?;
    }
    for role in list(spec, "closedParents") {
        parents.insert(role.as_str().unwrap_or("").to_string(), uuid_text());
    }
    let opened =
        children.iter().find_map(|c| demo.panes.get(text(c, "parent")).cloned()).ok_or("the delegation demo has no open parent")?;
    host.flush();

    // The open children, opened beside their parent as delegate opens them, each working in its own folder; a closed child has no pane.
    // Where delegation itself puts a smoke's worktrees (delegation.rs `start`: the smoke's own folder), so the records name them there.
    let smoke_root = crate::delegation::smoke_root(host);
    let worktrees = mighty_delegation::git::worktree::Maker::new(
        &smoke_root.join("worktrees").to_string_lossy(),
        mighty_delegation::git::Git::new(host.processes.clone(), smoke_root.join("git-no-hooks")),
        Arc::new(mighty_platform::fake::FixedDiskSpace(u64::MAX)),
    );
    let state_of = |child: &Value| ChildState::parse(text(child, "state")).ok_or("a delegation child has an unknown state");
    for child in &children {
        if !state_of(child)?.is_open() {
            continue;
        }
        let (role, id) = (text(child, "role"), text(child, "id"));
        let spec = pane_spec(role)?.clone();
        let parent = parents.get(text(child, "parent")).cloned().ok_or_else(|| format!("the delegation demo has no parent for {role}"))?;
        let folder = worktrees.worktree_path(id);
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let beside = demo.panes.get(text(child, "parent")).cloned().unwrap_or_else(|| opened.clone());
        let made = app
            .create_pane(DelegationChildPane {
                session_id: id.to_string(),
                parent_session_id: beside,
                mode: child.get("mode").and_then(Value::as_str).unwrap_or("acceptEdits").to_string(),
                folder: folder.clone(),
                started_by_human: false,
                provider: "claude".into(),
            })
            .await;
        if !made {
            return errored(format!("could not open the {role} pane"));
        }
        demo.panes.insert(role.to_string(), id.to_string());
        update(host, |state| {
            for each in state.sessions_mut().iter_mut().filter(|p| p.id() == id) {
                *each = Arc::new(RunSession { parent_session_id: Some(parent.clone()), ..filled((**each).clone(), &spec) });
            }
        })?;
    }
    host.flush();

    // Their runs, played by the canned CLI through the real run path, each waited for until its last line is in.
    let scripts = directory.join("help-demo");
    std::fs::create_dir_all(&scripts).map_err(|e| e.to_string())?;
    let (runs, plays) = canned_runs(&panes, &demo.panes, &scripts);
    let script = scripts.join("delegation-script.json");
    std::fs::write(&script, json!({"runs": runs}).to_string()).map_err(|e| e.to_string())?;
    let executable = std::env::current_exe().map_err(|e| format!("the app's own binary is not known: {e}"))?;
    host.engine()?.catalog.set_override("claude", Some(demo_cli(&executable, &script, Some(CLAUDE_VERSION))));
    for play in &plays {
        play_run(host, play).await?;
        if let Some(seconds) = play.run.get("elapsedSeconds").and_then(Value::as_f64) {
            demo.delegation_timings.insert(play.id.clone(), (seconds, play.finish.is_some()));
        }
    }
    delegation_clocks(host, demo)?;

    // The records, as the Mac's HelpDemoDelegation.file writes them, loaded the way a launch loads them.
    let parent_branch = spec.get("parentBranch").and_then(Value::as_str).unwrap_or("main");
    let base_commit = text(spec, "baseCommit");
    const DELIVERED_RUN_ID: &str = "help-demo-run";
    let checkout_text = checkout.to_string_lossy().into_owned();
    let (mut records, mut merges, mut copies) = (vec![], vec![], vec![]);
    for child in &children {
        let (id, task) = (text(child, "id"), text(child, "task"));
        let mode = child.get("mode").and_then(Value::as_str).unwrap_or("acceptEdits");
        let parent = parents
            .get(text(child, "parent"))
            .cloned()
            .ok_or_else(|| format!("the delegation demo has no session for {}", text(child, "parent")))?;
        let head = child.get("reportHead").and_then(Value::as_str).map(str::to_string);
        let mut record = ChildRecord::make_with(
            id,
            &parent,
            &worktrees.worktree_path(id),
            parent_branch,
            base_commit,
            mode,
            &request_key(&parent, DELIVERED_RUN_ID, task, mode, None),
            ChildRecordOptions {
                report_revision: child.get("reportRevision").and_then(Value::as_i64).unwrap_or(0) as i32,
                report_head: head.clone(),
                state: Some(state_of(child)?),
                parent_checkout: Some(checkout_text.clone()),
                ..Default::default()
            },
        );
        copies.push(DelegationCopy::of(id, DelegationCopyKind::Task, 0, task.as_bytes(), None));
        if let (Some(kind), Some(head)) = (MergeKind::parse(text(child, "merge")), head.as_deref()) {
            merges.push(MergeRecord::new(id, kind, parent_branch, base_commit, head, head));
            if record.state == ChildState::Closed {
                record.closed_head = Some(head.to_string());
            }
        }
        records.push(record);
    }
    let time = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut notices = vec![];
    for notice in list(spec, "notices") {
        let child = children
            .iter()
            .find(|c| text(c, "role") == text(notice, "child"))
            .ok_or_else(|| format!("the delegation demo has no child {}", text(notice, "child")))?;
        let lane = DeliveryLane::parse(text(notice, "lane")).ok_or("a delegation notice has an unknown lane")?;
        let kind = NoticeKind::parse(text(notice, "kind")).ok_or("a delegation notice has an unknown kind")?;
        let mut made = Notice::new(
            text(notice, "id"),
            text(child, "id"),
            notice.get("revision").and_then(Value::as_i64).unwrap_or(0) as i32,
            kind,
            lane,
        );
        if lane == DeliveryLane::Delivered {
            made.receipt = Some(DeliveryReceipt::new(&time, DeliveryRoute::Queue, DELIVERED_RUN_ID));
        }
        notices.push(made);
    }
    let held = notices.iter().filter(|n| n.lane == DeliveryLane::Held).count();
    DelegationFileStore::new(smoke_root)
        .save(&DelegationFile::of(records.clone(), notices, vec![], merges, copies))
        .map_err(|e| format!("{e:?}"))?;
    if !app.restart().await {
        return errored("delegation did not load the demo's records");
    }
    let expected: Vec<String> = records.iter().map(|r| r.id.clone()).collect();
    let loaded = wait(10.0, || {
        app.rows().iter().map(|r| r.id.clone()).collect::<Vec<_>>() == expected
            && parents.values().map(|p| app.held_items(p).len()).sum::<usize>() == held
    })
    .await;
    if !loaded {
        return errored("the delegation records did not load");
    }
    if let Some(layout) = list(&profile, "layouts").iter().find(|l| text(l, "workspace") == space) {
        select_workspace(host, &workspace)?;
        apply_preset(host, layout.get("preset").and_then(Value::as_str).unwrap_or("tabs"))?;
        select_session(host, &demo.pane(text(layout, "active"))?)?;
    }
    host.flush();
    Ok(())
}

/// A new lowercase GUID's text (C# `Guid.NewGuid().ToString().ToLowerInvariant()`).
fn uuid_text() -> String {
    wire::new_id().to_lowercase()
}

/// C# `HelpDelegationClocks`: the delegation demo's run clocks as the profile gives them, read from now, and each pane's transcript moved
/// with them, its last line now.
fn delegation_clocks(host: &Host, demo: &Demo) -> Result<(), String> {
    let Some(workspace) = demo.delegation_workspace().and_then(|w| demo.workspaces.get(w)).cloned() else { return Ok(()) };
    let now = DateTimeOffset::now();
    update(host, |state| {
        for session in state.sessions_mut().iter_mut().filter(|p| p.workspace_id() == workspace) {
            let mut next = (**session).clone();
            let last = next.logs().last().and_then(|l| mighty_core::time::parse(l.timestamp.as_deref()));
            if let (Some(last), Some(logs)) = (last, next.logs.as_mut()) {
                let shift = now.utc_ticks() - last.utc_ticks();
                for entry in logs.iter_mut() {
                    if let Some(at) = mighty_core::time::parse(entry.timestamp.as_deref()) {
                        entry.timestamp =
                            Some(DateTimeOffset { local_ticks: at.local_ticks + shift, offset_minutes: at.offset_minutes }.to_o());
                    }
                }
            }
            if let Some((seconds, finished)) = demo.delegation_timings.get(next.id()) {
                next.run_timing = Some(timing(*seconds, *finished));
            }
            *session = Arc::new(next);
        }
    })
}

/// C# `CheckHelpDelegation`: the delegation demo shows what the screen is meant to.
fn check_delegation(host: &Host, screen: &str, id: &str, demo: &Demo) -> Result<(), String> {
    let app = crate::delegation::app().ok_or("no delegation")?;
    let children = demo.profile.get("delegation").map(|d| list(d, "children").to_vec()).unwrap_or_default();
    let snapshot = host.current();
    let session = snapshot.session(id).cloned().ok_or("the pane is missing")?;
    let workspace = demo.workspace(demo.delegation_workspace().unwrap_or_default())?;
    let rows = app.rows();
    let tree = mighty_core::shell::delegation_tree(&workspace, &snapshot, &rows);
    match screen {
        "delegation-tree" => {
            let expected: Vec<&str> = children
                .iter()
                .filter(|c| demo.panes.get(text(c, "parent")).map(String::as_str) == Some(id))
                .map(|c| text(c, "id"))
                .collect();
            let listed: Option<Vec<&str>> = tree.children.get(id).map(|under| under.iter().map(|c| c.id.as_str()).collect());
            if expected.is_empty() || listed.as_deref() != Some(expected.as_slice()) {
                return errored("the parent's children are not listed under it");
            }
            if !session
                .logs()
                .iter()
                .any(|l| l.kind.as_deref() == Some("user") && l.text.as_deref().unwrap_or("").starts_with("[Mighty Code notice "))
            {
                return errored("the parent's transcript has no notice");
            }
        }
        "delegation-held" => {
            if session.status() != "stopped" || app.held_items(id).is_empty() {
                return errored("the stopped parent does not hold its notices");
            }
        }
        "delegation-parent-closed" => {
            let row = tree.parent_closed.iter().flat_map(|(_, _, children)| children.iter()).find(|c| c.id == id);
            if !row.is_some_and(|row| row.held_notices > 0 && snapshot.session(&row.parent_session_id).is_none()) {
                return errored("the closed parent's child is not listed with its held notices");
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded shot list is the help site's and every screen it names is one this capture draws (C#'s list, the same 34).
    #[test]
    fn every_listed_shot_is_a_screen_the_capture_knows() {
        let shots: Value = serde_json::from_str(SHOTS).unwrap();
        let list = shots["shots"].as_array().unwrap();
        assert_eq!(list.len(), 34);
        for shot in list {
            assert!(KNOWN_SCREENS.contains(&shot["screen"].as_str().unwrap()), "{shot}");
            assert!(shot["id"].as_str().is_some_and(|id| !id.is_empty()));
        }
        let mut known = KNOWN_SCREENS.to_vec();
        known.sort_unstable();
        known.dedup();
        assert_eq!(known.len(), KNOWN_SCREENS.len(), "each screen once");
    }

    /// The demo profile reads, and in every language its user-visible strings are plain strings.
    #[test]
    fn the_demo_profile_reads_in_every_language() {
        let profile: Value = serde_json::from_str(DEMO_PROFILE).unwrap();
        for language in ["ko", "en", "zh", "ja"] {
            let localized = mighty_core::demo::localized(&profile, language);
            assert!(localized["workspaces"].as_array().is_some_and(|w| !w.is_empty()));
            assert!(localized["panes"].as_array().unwrap().iter().all(|p| p["title"].is_string() || p["title"].is_null()));
            assert!(localized["delegation"]["workspace"].is_string());
        }
    }

    /// C#'s `Entry`: the manifest row's keys in C#'s order, the file named `<id>-<theme>.png`.
    #[test]
    fn a_manifest_entry_has_csharps_keys_in_order() {
        let shot = json!({"id": "overview", "section": "layout", "screen": "overview"});
        let e = entry(&shot, "overview", "dark", "ok");
        assert_eq!(e.keys().collect::<Vec<_>>(), ["id", "section", "screen", "theme", "file", "status"]);
        assert_eq!(e["file"], "overview-dark.png");
    }

    /// The canned runs: each run of a pane the demo has, its marker named by role and index, in the panes' order.
    #[test]
    fn the_canned_script_lists_each_runs_frames_and_marker() {
        let panes = vec![
            json!({"role": "mighty", "provider": "claude", "runs": [{"input": "a", "frames": [1], "finish": "completed"}, {"input": "b", "frames": []}]}),
            json!({"role": "absent", "provider": "codex", "runs": [{"input": "c"}]}),
        ];
        let ids = HashMap::from([("mighty".to_string(), "pane-1".to_string())]);
        let scripts = Path::new("scripts");
        let (runs, plays) = canned_runs(&panes, &ids, scripts);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0]["provider"], "claude");
        assert_eq!(runs[0]["marker"], scripts.join("mighty-0.done").to_string_lossy().as_ref());
        assert_eq!(plays[0].finish.as_deref(), Some("completed"));
        assert_eq!(plays[1].finish, None);
        assert_eq!(plays[1].marker, scripts.join("mighty-1.done"));
    }

    #[test]
    fn a_png_size_is_read_from_its_header() {
        let mut png = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        png.extend_from_slice(&1440u32.to_be_bytes());
        png.extend_from_slice(&900u32.to_be_bytes());
        png.push(8);
        assert_eq!(png_size(&png), (1440, 900));
        assert_eq!(png_size(&[0; 10]), (0, 0));
    }

    /// A run's clock: started the profile's seconds ago, finished now only when it finished.
    #[test]
    fn a_runs_clock_is_read_from_now() {
        let finished = timing(90.0, true);
        assert_eq!(finished.last_observed_at.utc_ticks() - finished.started_at.utc_ticks(), 900_000_000);
        assert_eq!(finished.finished_at, Some(finished.last_observed_at));
        assert_eq!(timing(5.0, false).finished_at, None);
    }
}
