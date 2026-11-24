//! MC-48: the browser check — a Chromium-family browser already installed on this computer (Chrome, Edge, Chromium, Brave, found at
//! their known places; never downloaded), started headless on a private profile folder with DevTools on a loopback port it picks,
//! visiting each address of this worktree's dev server ([`mighty_delegation::verify::allowed_browser_url`]: localhost within the pane's
//! port block or its run action's address, nothing else) and stopped afterwards with everything it started. The screenshots are kept
//! in `<data>/verify-screenshots` (the newest [`SCREENSHOTS_KEPT`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mighty_delegation::verify::{self, BrowserReport, BrowserScope, PageFindings};
use mighty_platform::SpawnRequest;

use super::cdp::{self, Cdp, PageLimits};
use crate::host::Host;

/// The screenshots kept.
pub const SCREENSHOTS_KEPT: usize = 100;
/// How long the browser may take to open its DevTools port.
const START_LIMIT: Duration = Duration::from_secs(10);

/// The time limits of one check: the agent's tool must answer within the MCP call's 30 s.
#[derive(Debug, Clone, Copy)]
pub struct CheckLimits {
    pub start: Duration,
    pub page: PageLimits,
}

impl CheckLimits {
    pub const VERIFY: CheckLimits =
        CheckLimits { start: START_LIMIT, page: PageLimits { load: Duration::from_secs(20), settle: Duration::from_millis(1500) } };
    pub const TOOL: CheckLimits = CheckLimits {
        start: Duration::from_secs(8),
        page: PageLimits { load: Duration::from_secs(5), settle: Duration::from_millis(700) },
    };
}

/// The browser installed here: its name and program.
pub fn installed() -> Option<(String, PathBuf)> {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    };
    let home = std::env::home_dir();
    let windows_dirs: Vec<PathBuf> =
        ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"].iter().filter_map(std::env::var_os).map(PathBuf::from).collect();
    let candidates = verify::browser_candidates(os, home.as_deref(), &windows_dirs);
    verify::find_browser(&candidates, |path| path.is_file())
}

/// The screenshot folder.
pub fn screenshot_dir(host: &Host) -> PathBuf {
    host.data_dir.join("verify-screenshots")
}

/// Whether `name` is a screenshot name this check writes (`<letters, digits, -, _>.png`, no folder).
pub fn screenshot_name_ok(name: &str) -> bool {
    name.len() <= 120
        && name.ends_with(".png")
        && name.len() > 4
        && name[..name.len() - 4].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn keep_newest(dir: &Path, kept: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(screenshot_name_ok))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, path) in files.into_iter().skip(kept) {
        let _ = std::fs::remove_file(path);
    }
}

/// The browser's DevTools address once it wrote `DevToolsActivePort` in its profile folder (the port, then the browser's path).
pub(crate) async fn devtools_address(profile: &Path, limit: Duration) -> Option<String> {
    let file = profile.join("DevToolsActivePort");
    let deadline = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < deadline {
        if let Ok(text) = tokio::fs::read_to_string(&file).await {
            let mut lines = text.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next())
                && let Ok(port) = port.trim().parse::<u16>()
                && path.starts_with("/devtools/browser/")
            {
                return Some(format!("ws://127.0.0.1:{port}{}", path.trim()));
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

/// The browser's start: headless, its own profile, DevTools on loopback only, nothing of the person's browser used.
fn launch_request(program: &Path, profile: &Path) -> SpawnRequest {
    let mut args = vec!["--headless=new".to_string()];
    args.extend(private_args(profile));
    args.extend(["--mute-audio".to_string(), "--hide-scrollbars".into(), "--window-size=1280,800".into(), "about:blank".into()]);
    SpawnRequest::new(program, args, profile)
}

/// What every browser this app starts gets (MC-48's check, MC-7's preview): DevTools on a loopback port it picks and its own profile
/// folder, no extensions, sync, first-run pages or background traffic. `--remote-allow-origins` is never given, so no web page (the
/// previewed one included) can open the DevTools socket: the browser refuses a connection that carries an `Origin`.
pub(crate) fn private_args(profile: &Path) -> Vec<String> {
    vec![
        "--remote-debugging-port=0".to_string(),
        "--remote-debugging-address=127.0.0.1".into(),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-extensions".into(),
        "--disable-background-networking".into(),
        "--disable-component-update".into(),
        "--disable-default-apps".into(),
        "--disable-sync".into(),
        "--metrics-recording-only".into(),
    ]
}

/// A started browser's output read and dropped, so a full pipe never stops it; its input closed.
pub(crate) fn drop_output(child: &mut mighty_platform::Child) {
    drop(child.stdin.take());
    for reader in [child.stdout.take(), child.stderr.take()].into_iter().flatten() {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let mut reader = reader;
            let mut buffer = [0u8; 4096];
            while matches!(reader.read(&mut buffer).await, Ok(n) if n > 0) {}
        });
    }
}

/// Removes a browser's profile folder once it let go of it (a moment after it ends).
pub(crate) async fn remove_profile(profile: &Path) {
    for _ in 0..10 {
        if std::fs::remove_dir_all(profile).is_ok() || !profile.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Visits `targets` (already checked against the pane's ports) in the installed browser. A browser that is not installed or does not
/// start is said plainly in `unavailable`. Screenshots are saved as `<prefix>-<n>.png`.
pub async fn check(host: &Arc<Host>, targets: &[url::Url], limits: CheckLimits, prefix: &str) -> BrowserReport {
    let Some((name, program)) = installed() else {
        return BrowserReport::unavailable(host.locale.get("verify.browser.none"));
    };
    let profile = host.data_dir.join("verify-browser").join(mighty_core::wire::new_id());
    if let Err(error) = std::fs::create_dir_all(&profile) {
        return BrowserReport::unavailable(error.to_string());
    }
    let mut child = match host.processes.spawn(launch_request(&program, &profile)).await {
        Ok(child) => child,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&profile);
            return BrowserReport::unavailable(host.locale.get_with("verify.browser.startFailed", &[("reason", &error.to_string())]));
        }
    };
    let control = child.control();
    drop_output(&mut child);
    let mut report = BrowserReport { browser: Some(name), pages: Vec::new(), unavailable: None };
    match devtools_address(&profile, limits.start).await {
        None => report.unavailable = Some(host.locale.get_with("verify.browser.startFailed", &[("reason", "no DevTools port")])),
        Some(address) => match Cdp::connect(&address, limits.start).await {
            Err(error) => report.unavailable = Some(host.locale.get_with("verify.browser.startFailed", &[("reason", &error)])),
            Ok(mut client) => {
                let dir = screenshot_dir(host);
                let _ = std::fs::create_dir_all(&dir);
                for (index, target) in targets.iter().enumerate() {
                    let (mut page, shot): (PageFindings, _) = cdp::check_page(&mut client, target.as_str(), limits.page).await;
                    if let Some(bytes) = shot {
                        let file = format!("{prefix}-{}.png", index + 1);
                        if screenshot_name_ok(&file) && std::fs::write(dir.join(&file), bytes).is_ok() {
                            page.screenshot = Some(file);
                        }
                    }
                    report.pages.push(page);
                }
                client.close().await;
                keep_newest(&dir, SCREENSHOTS_KEPT);
            }
        },
    }
    control.kill();
    let _ = control.close().await;
    remove_profile(&profile).await;
    report
}

/// The addresses the browser may visit for `base` and `paths` in `scope`; Err with the reason (said to the agent or on the card).
pub fn targets(base: &str, paths: &[String], scope: &BrowserScope) -> Result<Vec<url::Url>, String> {
    verify::browser_targets(base, paths, scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_names_never_name_a_folder() {
        assert!(screenshot_name_ok("abc-1.png") && screenshot_name_ok("A_b-2.png"));
        for bad in ["../x.png", "x/y.png", "x.jpg", ".png", "x\\y.png", "x y.png"] {
            assert!(!screenshot_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn the_browser_starts_headless_on_its_own_profile_with_loopback_devtools() {
        let request = launch_request(Path::new("/Applications/Chrome"), Path::new("/tmp/profile"));
        assert!(request.args.contains(&"--headless=new".to_string()));
        assert!(request.args.contains(&"--remote-debugging-address=127.0.0.1".to_string()));
        assert!(request.args.iter().any(|a| a == "--user-data-dir=/tmp/profile"));
    }

    #[tokio::test]
    async fn the_devtools_address_is_read_from_the_profile() {
        let dir = std::env::temp_dir().join(format!("mc-verify-devtools-{}", mighty_core::wire::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(devtools_address(&dir, Duration::from_millis(200)).await.is_none(), "none before the browser wrote it");
        std::fs::write(dir.join("DevToolsActivePort"), "9333\n/devtools/browser/abc\n").unwrap();
        assert_eq!(devtools_address(&dir, Duration::from_secs(1)).await.as_deref(), Some("ws://127.0.0.1:9333/devtools/browser/abc"));
        std::fs::write(dir.join("DevToolsActivePort"), "9333\n/elsewhere\n").unwrap();
        assert!(devtools_address(&dir, Duration::from_millis(200)).await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
