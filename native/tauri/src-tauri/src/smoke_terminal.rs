//! The smoke's terminal step (C# MainWindow.WebShell.Panes.cs `RunWebPanesSmoke`, its terminal part), where the page draws the
//! terminal (Windows: xterm.js over ConPTY): the host adds a shell pane in a workspace of its own (a temporary folder, in memory)
//! and lets it start a shell; the probe waits for the shell (idle at its prompt), types a command through the page's xterm that
//! writes a marker in two halves and waits for the joined marker in the terminal's buffer, reads the footer, types a command that
//! runs a while (ping) and waits for the pane to read running and then idle again, and closes the pane (its shell ends).
//!
//! Where the platform draws the terminal (the Mac's Ghostty, M-D), the native variant ([`start_native`], [`NATIVE_PROBE`]) adds the
//! same pane and drives its view instead: typing through Ghostty's input path, reading both the pane's buffer and Ghostty's grid, and
//! adding Swift's terminal smoke's stages (the TTY and its grid, the working folder and title, Ctrl+C, a resize, the snapshot, a
//! restart).

use std::path::PathBuf;
use std::sync::Arc;

use mighty_core::{AppSnapshot, Wire, wire};
use serde_json::{Value, json};

use crate::host::Host;
use crate::terminal::terminals;

/// The pane and workspace the step adds (UUID-shaped, as the state's ids are).
pub const PANE: &str = "5a0e0b0b-0000-4000-8000-0000000007e1";
const WORKSPACE: &str = "5a0e0b0b-0000-4000-8000-0000000007e2";

/// The probe; `__PANE__` and `__MARKER__` are plain ASCII, `__TYPED__` and `__BUSY__` JSON string literals.
pub const PROBE: &str = r#"(async () => {
  const until = async (test, ms) => { const end = performance.now() + ms; while (performance.now() < end) { const v = await test(); if (v) return v; await new Promise((r) => setTimeout(r, 100)); } return test(); };
  const invoke = (command, args) => window.__TAURI_INTERNALS__.invoke(command, args);
  const paneId = '__PANE__', marker = '__MARKER__', typed = __TYPED__, busy = __BUSY__;
  const state = () => invoke('smoke_terminal', { paneId, action: 'state' });
  const slot = () => document.querySelector('.pane-slot:not(.hidden)[data-pane-id="' + paneId + '"]');
  const buffer = () => window.mightyPanes?.terminalText?.(paneId) ?? '';
  const report = { paneId };
  try {
    report.pane = !!(await until(() => slot()?.querySelector('[data-testid=terminal-pane]'), 15000));
    report.started = !!(await until(async () => (await state()).running, 30000));
    report.idle = !!(await until(async () => (await state()).status === 'idle', 15000));
    report.typed = !!(await until(() => window.mightyPanes?.terminalInput?.(paneId, typed) === true, 15000));
    report.echoed = !!(await until(() => buffer().includes(marker), 20000));
    report.footer = slot()?.querySelector('[data-testid=terminal-footer]')?.textContent ?? null;
    report.notice = slot()?.querySelector('[data-testid=terminal-notice]')?.textContent ?? null;
    const sent = window.mightyPanes?.terminalInput?.(paneId, busy) === true;
    report.running = sent && !!(await until(async () => (await state()).status === 'running', 15000));
    report.idleAgain = report.running && !!(await until(async () => (await state()).status === 'idle', 30000));
    if (!report.echoed || !report.running) {
      report.text = buffer().slice(-600);
      report.outputs = window.mightyPanes?.terminalOutputs?.() ?? null;
      report.host = await state();
    }
    report.closed = await invoke('smoke_terminal', { paneId, action: 'close' });
  } catch (error) {
    report.error = String(error);
  }
  await invoke('smoke_probe', { probe: report });
})();"#;

/// The step's workspace folder: a new temporary folder.
fn workspace_folder() -> std::io::Result<PathBuf> {
    let folder = std::env::temp_dir().join(format!("mighty-tauri-smoke-terminal-{}", std::process::id()));
    std::fs::create_dir_all(&folder)?;
    Ok(folder)
}

/// Adds the shell pane (made the active one, in memory as the other steps' panes are), lets it start a shell, and gives the probe
/// with its marker and commands filled in.
pub fn start(host: &Host) -> Result<String, String> {
    let terminals = add_pane(host)?;
    // A marker the terminal must render back through the pseudo console, typed in two halves so the echo of the typing alone never
    // holds it whole.
    let marker = format!("MIGHTY_WEB_{}", wire::new_id().replace('-', ""));
    let (head, tail) = marker.split_at(12);
    let powershell = terminals.default_shell().file_stem().is_some_and(|stem| stem.eq_ignore_ascii_case("powershell"));
    let (typed, busy) = if powershell {
        (format!("[Console]::WriteLine(('{head}'+'{tail}'))\r"), "ping -n 6 127.0.0.1 > $null\r".to_string())
    } else {
        (
            format!("set MIGHTY_SMOKE={head}\rset MIGHTY_SMOKE=%MIGHTY_SMOKE%{tail}\recho %MIGHTY_SMOKE%\r"),
            "ping -n 6 127.0.0.1 >nul\r".to_string(),
        )
    };
    Ok(PROBE
        .replace("__PANE__", PANE)
        .replace("__MARKER__", &marker)
        .replace("__TYPED__", &serde_json::to_string(&typed).unwrap_or_default())
        .replace("__BUSY__", &serde_json::to_string(&busy).unwrap_or_default()))
}

/// Adds the step's shell pane in a workspace of its own (made the active one, in memory as the other steps' panes are) and lets it
/// start a shell (both variants).
fn add_pane(host: &Host) -> Result<Arc<crate::terminal::Terminals>, String> {
    let terminals = terminals(host)?;
    let folder = workspace_folder().map_err(|e| e.to_string())?;
    terminals.smoke_allow(PANE);
    {
        let mut view = host.view();
        let mut state: Value = serde_json::from_str(&view.snapshot.to_json()).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let workspace = json!({"id": WORKSPACE, "name": "Smoke terminal", "path": folder, "createdAt": now});
        let pane = json!({
            "id": PANE, "workspaceId": WORKSPACE, "title": "Web smoke terminal", "kind": "shell", "status": "idle", "model": "default",
            "provider": "claude", "settings": {"effort": "default", "permissionMode": "manual", "maxTurns": null, "maxBudgetUsd": null},
            "logs": [], "draft": "", "createdAt": now,
        });
        for (name, item) in [("workspaces", workspace), ("sessions", pane)] {
            state.get_mut(name).and_then(Value::as_array_mut).ok_or_else(|| format!("the state has no {name}"))?.push(item);
        }
        let mut expanded: Vec<Value> = state.get("expandedWorkspaceIds").and_then(Value::as_array).cloned().unwrap_or_default();
        expanded.push(json!(WORKSPACE));
        state["expandedWorkspaceIds"] = json!(expanded);
        state["activeWorkspaceId"] = json!(WORKSPACE);
        state["activeSessionId"] = json!(PANE);
        view.replace(AppSnapshot::read(&state).map_err(|e| e.to_string())?);
    }
    Ok(terminals)
}

/// The step's checks from its probe's report.
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let yes = |key: &str| report.get(key) == Some(&Value::Bool(true));
    let footer = report.get("footer").and_then(Value::as_str).unwrap_or("");
    vec![
        ("terminalNoError", report.get("error").is_none()),
        ("terminalStarted", yes("pane") && yes("started") && yes("idle")),
        ("terminalEcho", yes("typed") && yes("echoed")),
        // The engine and the grid ("ConPTY · powershell … 120×30").
        ("terminalFooter", footer.contains("ConPTY") && footer.contains('×')),
        ("terminalStatus", yes("running") && yes("idleAgain")),
        ("terminalClosed", yes("closed")),
    ]
}

/// M-D: the native variant's checks, where the platform draws the terminal (the Mac's Ghostty fed by the pane's PTY): Windows' five
/// (`terminalStarted`, `terminalEcho` read from the pane's buffer, `terminalFooter` with engine `Ghostty`, `terminalStatus`,
/// `terminalClosed`) and Swift's terminal smoke's keys (`--terminal-smoke-test`, native-macos.yml:221-250): the TTY and its grid
/// (`/usr/bin/tty`, `stty size`), the working folder and title (OSC 7, the title callback), Ctrl+C, a resize, a restart, and a
/// snapshot that is not blank.
pub const NATIVE_CHECKS: [&str; 11] = [
    "terminalStarted",
    "terminalEcho",
    "terminalFooter",
    "terminalStatus",
    "terminalClosed",
    "terminalTty",
    "terminalCwdTitle",
    "terminalCtrlC",
    "terminalResize",
    "terminalRestart",
    "terminalSnapshot",
];

/// The native variant's probe (M-D; Swift's `runTerminalSmokeTest` stages on the Tauri page). Everything is typed through the view
/// (`type`: Ghostty's own input path, each CR as the Return key, `\u0003` as Ctrl+C), so what the shell gets came back through
/// libghostty's `receive_buffer`; what the shell printed is read twice: from the pane's buffer (`state`'s tail, the output the pane
/// sent on) and from the grid Ghostty drew (`native`'s text). Markers are put together by `printf`, so the typed command's own echo
/// never holds one. `__PANE__`, `__HEAD__` and `__TAIL__` are plain ASCII, `__FOLDER__` a JSON string literal.
pub const NATIVE_PROBE: &str = r#"(async () => {
  const until = async (test, ms) => { const end = performance.now() + ms; while (performance.now() < end) { const v = await test(); if (v) return v; await new Promise((r) => setTimeout(r, 100)); } return test(); };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const invoke = (command, args) => window.__TAURI_INTERNALS__.invoke(command, args);
  const paneId = '__PANE__', head = '__HEAD__', tail = '__TAIL__', folder = __FOLDER__, marker = head + tail;
  const act = (action, text) => invoke('smoke_terminal', { paneId, action, text: text ?? null });
  const state = () => act('state');
  const native = () => act('native');
  const screen = async () => (await native()).text ?? '';
  const type = (line) => act('type', line + '\r');
  const seen = (text, ms) => until(async () => (await screen()).includes(text), ms ?? 8000);
  const status = async (want, ms) => !!(await until(async () => (await state()).status === want, ms));
  const quote = (text) => "'" + text.split("'").join("'\\''") + "'";
  const slot = () => document.querySelector('.pane-slot:not(.hidden)[data-pane-id="' + paneId + '"]');
  const cell = () => slot()?.querySelector('[data-testid=terminal-pane] [data-native-slot=terminal]');
  const footerOf = async () => (await state()).footer ?? {};
  // The pane's grid as `stty size` says it: [rows, columns].
  const gridOf = async () => { const m = /^(\d+)×(\d+)$/.exec((await footerOf()).grid ?? ''); return m ? [Number(m[2]), Number(m[1])] : null; };
  const last = (text, pattern) => { let found = null; for (const m of text.matchAll(pattern)) found = m; return found; };
  const same = (a, b) => !!a && !!b && a[0] === b[0] && a[1] === b[1];
  const picture = (url) => new Promise((resolve) => {
    const image = new Image();
    image.onload = () => {
      const canvas = document.createElement('canvas');
      canvas.width = image.width; canvas.height = image.height;
      const context = canvas.getContext('2d');
      context.drawImage(image, 0, 0);
      const data = context.getImageData(0, 0, canvas.width, canvas.height).data;
      const colours = new Set();
      for (let i = 0; i < data.length && colours.size < 8; i += 4 * 7) colours.add((data[i] << 16) | (data[i + 1] << 8) | data[i + 2]);
      resolve({ width: image.width, height: image.height, colours: colours.size });
    };
    image.onerror = () => resolve(null);
    image.src = url;
  });
  const report = { paneId };
  const began = performance.now();
  try {
    report.pane = !!(await until(() => cell(), 15000));
    report.view = !!(await until(async () => (await native()).view === true, 15000));
    report.started = !!(await until(async () => (await state()).running, 20000));
    report.idle = report.started && await status('idle', 15000);
    // The page lays the pane out again once the shell runs (the notice goes, the footer comes): the view follows the slot, so the
    // two are compared once that settles (at most 5 s), as a user would see them.
    const measure = async () => {
      const opened = await native();
      const box = cell()?.getBoundingClientRect();
      const scale = window.devicePixelRatio || 1;
      report.bounds = opened.bounds ?? null;
      report.slot = box ? [box.left * scale, box.top * scale, box.width * scale, box.height * scale].map(Math.round) : null;
      report.boundsMatch = !!opened.visible && !!report.slot && !!report.bounds
        && Math.abs(report.bounds.x - report.slot[0]) <= 2 && Math.abs(report.bounds.y - report.slot[1]) <= 2
        && Math.abs(report.bounds.width - report.slot[2]) <= 2 && Math.abs(report.bounds.height - report.slot[3]) <= 2;
      return report.boundsMatch;
    };
    await until(measure, 5000);
    report.terminalStarted = report.pane && report.view && report.idle && report.boundsMatch;

    await type("PS1='mighty-smoke% '; PROMPT='mighty-smoke% '; RPROMPT=''; /usr/bin/tty; [[ -t 0 && -t 1 && -t 2 ]] && printf 'MIGHTY_%s\\n' 'PTY_OK'; printf 'MIGHTY_GRID %s\\n' \"$(/bin/stty size)\"; printf 'MIGHTY_PID %s\\n' \"$$\"");
    report.ptyOk = !!(await seen('MIGHTY_PTY_OK'));
    await seen('MIGHTY_PID ', 3000);
    const first = await screen();
    report.tty = /\/dev\/ttys\d+/.exec(first)?.[0] ?? null;
    const sttyFirst = last(first, /MIGHTY_GRID (\d+) (\d+)/g);
    report.sttyGrid = sttyFirst ? [Number(sttyFirst[1]), Number(sttyFirst[2])] : null;
    report.grid = await gridOf();
    // A grid that changed after `stty` ran (the view's last resize of the layout above) is asked again: once the layout has settled
    // one more answer must agree (the terminal follows the view's grid). The count is kept, and more than one re-ask fails the check,
    // so a terminal that lags the view does not pass by being asked until it catches up.
    report.sttyRetries = 0;
    for (let again = 0; again < 5 && !same(report.sttyGrid, report.grid); again += 1) {
      report.sttyRetries = again + 1;
      await new Promise((r) => setTimeout(r, 500));
      await type("printf 'MIGHTY_GRID %s\\n' \"$(/bin/stty size)\"");
      await seen('MIGHTY_GRID ', 3000);
      await new Promise((r) => setTimeout(r, 200));
      const now = last(await screen(), /MIGHTY_GRID (\d+) (\d+)/g);
      report.sttyGrid = now ? [Number(now[1]), Number(now[2])] : null;
      report.grid = await gridOf();
    }
    report.shellPid = last(first, /MIGHTY_PID (\d+)/g)?.[1] ?? null;
    report.terminalTty = report.ptyOk && !!report.tty && same(report.sttyGrid, report.grid) && report.sttyRetries <= 1;

    await type("printf '%s%s\\n' '" + head + "' '" + tail + "'");
    report.echoed = !!(await until(async () => ((await state()).diagnostics?.tail ?? '').includes(marker), 8000));
    report.drawn = (await screen()).includes(marker);
    report.terminalEcho = report.echoed && report.drawn;
    report.footer = await footerOf();
    report.footerText = slot()?.querySelector('[data-testid=terminal-footer]')?.textContent ?? '';
    report.terminalFooter = report.footer.engine === 'Ghostty' && report.footerText.includes('Ghostty') && report.footerText.includes('×');

    await type('/bin/sleep 3');
    report.running = await status('running', 8000);
    report.idleAgain = report.running && await status('idle', 10000);
    report.terminalStatus = report.running && report.idleAgain;

    const nested = folder + '/Persistent';
    await type('mkdir -p ' + quote(nested) + ' && cd ' + quote(nested) + "; printf '\\033]7;file://%s%s\\007' \"$(/bin/hostname)\" \"$PWD\"; printf '\\033]2;%s\\007' 'Mighty Smoke Terminal'; printf 'MIGHTY_%s\\n' 'CWD_OK'");
    report.cwdOk = !!(await seen('MIGHTY_CWD_OK'));
    report.titled = !!(await until(async () => (await footerOf()).title === 'Mighty Smoke Terminal', 5000));
    report.moved = !!(await until(async () => ((await footerOf()).directory ?? '').endsWith('/Persistent'), 5000));
    report.directory = (await footerOf()).directory ?? null;
    report.terminalCwdTitle = report.cwdOk && report.titled && report.moved;

    await type('/bin/sleep 30');
    report.sleeping = await status('running', 8000);
    await act('type', '\u0003');
    report.interrupted = report.sleeping && await status('idle', 8000);
    await type("printf 'MIGHTY_%s\\n' 'INTERRUPT_OK'");
    report.terminalCtrlC = report.interrupted && !!(await seen('MIGHTY_INTERRUPT_OK'));

    const before = await gridOf();
    report.resizedTo = await act('resize', 'grow');
    report.gridChanged = !!(await until(async () => { const now = await gridOf(); return !!now && !same(now, before); }, 8000));
    await sleep(600);
    // As the tty check above: asked once more when the answer is missing or disagrees (the packaged app's first CI round, 44081587468,
    // never showed the line typed right after the resize), the count kept and more than one re-ask failing the check.
    report.resizedRetries = 0;
    for (let ask = 0; ask < 3; ask += 1) {
      report.resizedRetries = ask;
      await type("printf 'MIGHTY_RESIZED%s %s\\n' '" + ask + "' \"$(/bin/stty size)\"");
      await seen('MIGHTY_RESIZED' + ask + ' ', 5000);
      const resized = [...(await screen()).matchAll(/MIGHTY_RESIZED(\d) (\d+) (\d+)/g)].filter((m) => m[1] === String(ask)).pop();
      report.resizedStty = resized ? [Number(resized[2]), Number(resized[3])] : null;
      report.resizedGrid = await gridOf();
      if (same(report.resizedStty, report.resizedGrid)) break;
      await sleep(500);
    }
    report.terminalResize = report.gridChanged && same(report.resizedStty, report.resizedGrid) && !same(report.resizedStty, report.sttyGrid)
      && report.resizedRetries <= 1;
    await act('resize', 'restore');

    const url = await act('snapshot');
    report.snapshot = typeof url === 'string' ? await picture(url) : null;
    report.terminalSnapshot = typeof url === 'string' && url.startsWith('data:image/png') && !!report.snapshot
      && report.snapshot.width > 0 && report.snapshot.height > 0 && report.snapshot.colours >= 2;

    await type('exit');
    report.exited = await status('completed', 8000);
    await act('restart');
    report.restarted = report.exited && !!(await until(async () => { const s = await state(); return s.running && s.status === 'idle'; }, 15000));
    await type("printf 'MIGHTY_%s %s\\n' 'RESTART_OK' \"$$\"");
    await seen('MIGHTY_RESTART_OK', 8000);
    report.restartedPid = last(await screen(), /MIGHTY_RESTART_OK (\d+)/g)?.[1] ?? null;
    report.terminalRestart = report.restarted && !!report.restartedPid && report.restartedPid !== report.shellPid;

    if (!report.terminalEcho || !report.terminalTty || !report.terminalRestart) report.screen = (await screen()).slice(-1500);
    report.closed = await act('close');
    report.terminalClosed = report.closed === true;
  } catch (error) {
    report.error = String(error);
  }
  report.seconds = Math.round((performance.now() - began) / 100) / 10;
  await invoke('smoke_probe', { probe: report });
})();"#;

/// Starts the native variant (M-D): the shell pane added as for Windows' step, and the probe run on the page. False when the pane
/// could not be added (its checks then fail as not reported, smoke.rs).
pub fn start_native(host: &Host) -> bool {
    let Some(smoke) = host.smoke.as_ref() else { return false };
    if let Err(error) = add_pane(host) {
        host.log.line(&format!("smoke: the native terminal step's pane was not added: {error}"));
        return false;
    }
    let folder = match workspace_folder() {
        Ok(folder) => folder,
        Err(error) => {
            host.log.line(&format!("smoke: the native terminal step has no folder: {error}"));
            return false;
        }
    };
    let marker = format!("MIGHTY_NATIVE_{}", wire::new_id().replace('-', ""));
    let (head, tail) = marker.split_at(14);
    let script = NATIVE_PROBE
        .replace("__PANE__", PANE)
        .replace("__HEAD__", head)
        .replace("__TAIL__", tail)
        .replace("__FOLDER__", &serde_json::to_string(&folder.to_string_lossy()).unwrap_or_default());
    host.flush();
    host.log.line("smoke: native terminal step");
    smoke.eval(host, &script);
    true
}

/// The native variant's checks from its report: each `true` under its own name, and no error.
pub fn native_checks(report: &Value) -> Vec<(&'static str, bool)> {
    let clean = report.get("error").is_none();
    NATIVE_CHECKS.iter().map(|name| (*name, clean && report.get(*name) == Some(&Value::Bool(true)))).collect()
}

/// The main window's size before the native step grew it (`resize`), to put back.
static WINDOW_SIZE: std::sync::Mutex<Option<tauri::LogicalSize<f64>>> = std::sync::Mutex::new(None);

/// The native step's window resize: `grow` makes the main window 160 points wider and 90 taller (remembering its size), `restore`
/// puts it back. Its new logical size.
fn resize_window(host: &Host, how: &str) -> Result<Value, String> {
    use tauri::Manager as _;
    let window = host.app.get_window(crate::host::WINDOW).ok_or("no main window")?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let now = window.inner_size().map_err(|e| e.to_string())?.to_logical::<f64>(scale);
    let mut kept = WINDOW_SIZE.lock().unwrap_or_else(|p| p.into_inner());
    let next = match how {
        "grow" => {
            kept.get_or_insert(now);
            tauri::LogicalSize::new(now.width + 160.0, now.height + 90.0)
        }
        _ => kept.take().unwrap_or(now),
    };
    window.set_size(next).map_err(|e| e.to_string())?;
    Ok(json!({"width": next.width, "height": next.height}))
}

/// What the probe asks the host (smoke only): `state` of the step's pane, or `close` it (its shell ends; the pane leaves the state);
/// the native variant's `native` (the view as the OS has it, the grid Ghostty drew), `type` (`text` typed through the view),
/// `snapshot` (the view's picture), `resize` (`text`: `grow` or `restore`, the main window) and `restart` (the pane's restart).
#[tauri::command]
pub async fn smoke_terminal(
    pane_id: String,
    action: String,
    text: Option<String>,
    host: tauri::State<'_, Arc<Host>>,
) -> Result<Value, String> {
    if host.smoke.is_none() {
        return Err("smoke only".into());
    }
    let terminals = terminals(&host)?;
    let view = || crate::surface_terminal::natives().view(&pane_id).ok_or_else(|| "the pane has no native view".to_string());
    match action.as_str() {
        "state" => Ok(terminals.smoke_state(&host, &pane_id)),
        "native" => match crate::surface_terminal::natives().view(&pane_id) {
            Some(view) => {
                let state = view.inspect().await.map_err(|e| e.to_string())?;
                let text = view.text().await.unwrap_or_default();
                let bounds = state.bounds;
                Ok(json!({
                    "view": true, "visible": state.visible, "text": text,
                    "bounds": {"x": bounds.x, "y": bounds.y, "width": bounds.width, "height": bounds.height},
                }))
            }
            None => Ok(json!({"view": false})),
        },
        "type" => view()?.send_text(text.as_deref().unwrap_or("")).map(|()| Value::Null).map_err(|e| e.to_string()),
        "snapshot" => Ok(view()?.snapshot().await.map_err(|e| e.to_string())?.map(Value::String).unwrap_or(Value::Null)),
        "resize" => resize_window(&host, text.as_deref().unwrap_or("restore")),
        "restart" => {
            terminals.pane(&host, Some(&pane_id))?.restart().await;
            Ok(Value::Null)
        }
        "close" => {
            let closed = terminals.smoke_close(&pane_id).await;
            {
                let mut view = host.view();
                let mut next = view.snapshot.clone();
                next.sessions_mut().retain(|s| s.id() != pane_id);
                view.replace(next);
            }
            host.schedule();
            if let Ok(folder) = workspace_folder() {
                let _ = std::fs::remove_dir_all(folder);
            }
            Ok(json!(closed))
        }
        other => Err(format!("unknown smoke action {other}")),
    }
}
