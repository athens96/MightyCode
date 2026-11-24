//! MC-54: 진단 묶음 내보내기 (Settings › 정보 › 진단) and the turn audit's copy.
//!
//! The bundle is made only when the person presses the button and saved only where the save dialog says: the app's logs, its
//! version, each agent CLI's installed version, the OS and architecture and a settings summary, every text through the
//! [`redact::Redactor`], with `manifest.json` listing what is in it (bundle.rs). Nothing is sent anywhere.
//!
//! `auditCopy {text}`: the turn audit's Markdown (made by the page, mighty/audit.ts, from the run's saved records) put on the clipboard
//! through the same redactor, so a copied audit carries neither a key nor the user's home folder.

pub mod bundle;
pub mod redact;
pub mod summary;

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{Value, json};

use crate::host::{Host, Registry, lock};
use crate::methods::text;

/// The settings row's state: a bundle being made, the last one saved, or why the last one failed.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub running: bool,
    pub saved: Option<Saved>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Saved {
    pub path: PathBuf,
    pub files: usize,
    pub redacted: usize,
}

static STATUS: LazyLock<Mutex<Status>> = LazyLock::new(|| Mutex::new(Status::default()));

pub fn status() -> Status {
    lock(&STATUS).clone()
}

fn set(host: &Host, change: impl FnOnce(&mut Status)) {
    change(&mut lock(&STATUS));
    crate::native::touch(host, crate::settings::SURFACE);
}

/// The logs a bundle carries, by their names in it: the host's (and its previous file), the daemon's, the start-up and performance
/// logs.
fn log_sources(host: &Host) -> Vec<bundle::LogSource> {
    let source = |name: &str, path: PathBuf| bundle::LogSource { name: format!("logs/{name}"), path };
    let logs = host.log.dir();
    vec![
        source("host.log", logs.join("host.log")),
        source("host.log.1", logs.join("host.log.1")),
        source("daemon.log", crate::daemon::files::folder(&host.data_dir).join(crate::daemon::files::LOG_FILE)),
        source("startup.log", host.data_dir.join("startup.log")),
        source("perf.log", host.data_dir.join("perf.log")),
    ]
}

fn inputs(host: &Host) -> bundle::Inputs {
    let settings = summary::settings_summary(&host.view().snapshot);
    let clis = crate::setup::runtimes(host)
        .into_iter()
        .map(|runtime| bundle::Cli { id: runtime.id, name: runtime.name, available: runtime.available, version: runtime.version })
        .collect();
    bundle::Inputs {
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        app_version: host.version.clone(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        os_version: crate::platform::update_installer().system_version(),
        clis,
        settings,
        logs: log_sources(host),
    }
}

/// Where the bundle goes: the OS's save dialog over the app's window, a zip with the time in its name; None when cancelled.
async fn pick_place(host: &Host) -> Option<PathBuf> {
    use tauri::Manager as _;
    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let name = bundle::suggested_name(chrono::Local::now());
    let mut dialog = host.app.dialog().file().set_file_name(&name).add_filter("Zip", &["zip"]);
    if let Some(window) = host.app.get_webview_window(crate::host::WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    dialog.save_file(move |path| {
        let _ = sender.send(path);
    });
    receiver.await.ok().flatten().and_then(|path| path.into_path().ok())
}

/// 진단 묶음 내보내기: the place asked, the bundle made and written off the async workers, the row told how it went.
pub async fn export(host: Arc<Host>) -> Result<(), String> {
    if status().running {
        return Ok(());
    }
    let Some(mut path) = pick_place(&host).await else { return Ok(()) };
    if path.extension().is_none() {
        path.set_extension("zip");
    }
    set(&host, |s| {
        s.running = true;
        s.error = None;
    });
    let inputs = inputs(&host);
    let target = path.clone();
    let made = tauri::async_runtime::spawn_blocking(move || {
        let built = bundle::build(&inputs, &redact::Redactor::for_this_user());
        bundle::write_zip(&target, &built).map(|()| (built.files.len(), built.tally.total())).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|made| made);
    match made {
        Ok((files, redacted)) => {
            host.log.line(&format!("diagnostics: bundle saved ({files} files, {redacted} values redacted)"));
            set(&host, |s| {
                s.running = false;
                s.saved = Some(Saved { path, files, redacted });
            });
        }
        Err(error) => {
            let reason = host.locale.get_with("diagnostics.settings.failed", &[("reason", &error)]);
            set(&host, |s| {
                s.running = false;
                s.error = Some(reason);
            });
        }
    }
    Ok(())
}

/// The folder of the last saved bundle shown in Finder (Explorer on Windows).
pub async fn show_saved(host: Arc<Host>) -> Result<(), String> {
    let _ = host;
    let Some(folder) = status().saved.and_then(|s| s.path.parent().map(PathBuf::from)) else { return Ok(()) };
    tauri::async_runtime::spawn_blocking(move || crate::platform::show_in_explorer(&folder)).await.map_err(|error| error.to_string())?
}

/// `auditCopy {text}` → `{copied, redacted}`: the audit's Markdown, redacted, on the clipboard.
fn audit_copy(args: &Value) -> Result<Value, String> {
    let markdown = text(args, "text").unwrap_or_default();
    if markdown.is_empty() {
        return Ok(json!({ "copied": false, "redacted": 0 }));
    }
    let (clean, tally) = redact::Redactor::for_this_user().redact(markdown);
    crate::platform::clipboard().write_text(&clean).map_err(|error| error.to_string())?;
    Ok(json!({ "copied": true, "redacted": tally.total() }))
}

pub fn register(methods: &mut Registry) {
    methods.add("auditCopy", |_host: Arc<Host>, args| async move {
        tauri::async_runtime::spawn_blocking(move || audit_copy(&args)).await.map_err(|error| error.to_string())?
    });
}
