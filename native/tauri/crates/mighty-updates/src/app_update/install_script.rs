//! The Mac's install helper (Swift `AppUpdateService.installScript`, AppUpdate.swift:298-404; plan §1.1, M-H) and what the app checks
//! before it starts it (`AppStore.prepareAppUpdateInstall` and `verifyAppUpdatePackage`, AppStore+AppUpdate.swift:408-447).
//!
//! [`install_script`] writes the same text as Swift's for the same inputs, byte for byte (`tools/parity-mac install-script`,
//! tests/app_update_mac_parity.rs): the single-quote rule, the extended-regex escaping of the running check, the headless modes left out
//! of it, the `REPLACED` form when the new bundle lands under another name. The script waits for the app's pid (600 × 0.5 s), copies the
//! staged bundle beside the installed one with `ditto`, leaves the update staged when the app runs again from there (install on quit),
//! swaps by two renames with the signals ignored and a rollback, unregisters the old and staged copies from LaunchServices, registers the
//! installed one and, with `relaunch`, opens it (`open -n`). The 0.4.x Mac app's own copy of this script installs the Rust bundle from
//! 0.4.x (plan fact 1); this port is the Rust app's own 0.5 → 0.5.x updater.
//!
//! [`prepare`] makes Swift's checks in Swift's order, each refusal in Swift's words: the running app is an `.app` in a folder this user
//! may write (`errorNotWritable`), the staged bundle still passes [`super::stage_mac::validate`] (`errorPreflight`), then the script, and,
//! when the profile says so (`update_rechecks_sha256`, Swift's "Rule 4"), the package's size and SHA-256 again (`errorInstall` around
//! `errorSizeMismatch` / `errorDigestMismatch`). Writing `install.sh` and `install.log` and the detached start are the platform's
//! (`UpdateInstaller::start_install_script`).

use std::path::{Component, Path, PathBuf};

use mighty_core::locale::Locale;

use super::plan::InstallPlan;

/// The app's own headless modes, which can outlive its window for a moment and never hold the bundle: the running check leaves them out
/// (Swift's `PaneMCPServerLocation.headlessArgument`, `DelegationMCPServer.headlessArgument`, `--verify-resources`).
pub const HEADLESS_ARGUMENTS: [&str; 3] = ["--agent-io-mcp", "--agent-delegation-mcp", "--verify-resources"];

/// LaunchServices' tool, as the script names it.
pub const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// The script's file and its log, beside the package (Swift `launchInstaller`).
pub const SCRIPT_NAME: &str = "install.sh";
pub const LOG_NAME: &str = "install.log";

/// Swift's `quote`: single quotes, each `'` inside written `'\''`.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Swift's `literalPattern`: every extended-regex character escaped, so `pgrep` and `grep -E` match the text literally.
fn literal_pattern(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if "\\.[]()*+?{}|^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A file URL's `path` as Swift prints it: the text in the file system's representation ([`file_system_representation`]), its
/// trailing `/` dropped (the root stays `/`).
fn url_path(path: &Path) -> String {
    let text = file_system_representation(&path.to_string_lossy());
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty() && text.starts_with('/') { "/".to_string() } else { trimmed.to_string() }
}

/// HFS+'s decomposition, which Foundation applies to a file URL's `path` (CI's parity: `Mïghty` comes back with U+0308 after the `i`):
/// canonical decomposition (NFD) of everything outside U+2000–U+2FFF, U+F900–U+FAFF and U+2F800–U+2FAFF, which stay as they are.
pub fn file_system_representation(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization as _;
    let kept = |c: char| matches!(c as u32, 0x2000..=0x2FFF | 0xF900..=0xFAFF | 0x2F800..=0x2FAFF);
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    for c in text.chars() {
        if kept(c) {
            out.extend(run.nfd());
            run.clear();
            out.push(c);
        } else {
            run.push(c);
        }
    }
    out.extend(run.nfd());
    out
}

/// `standardizedFileURL.path`: `.` dropped and `..` taken back, without asking the disk (links stay as written).
fn standardized(path: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    let absolute = path.has_root();
    for component in path.components() {
        match component {
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                if parts.pop().is_none() && !absolute {
                    parts.push("..".into());
                }
            }
            Component::Normal(name) => parts.push(file_system_representation(&name.to_string_lossy())),
        }
    }
    let joined = parts.join("/");
    if absolute { format!("/{joined}") } else { joined }
}

/// Swift's `AppUpdateService.installScript(stagedApp:destination:replacing:runningExecutable:pid:relaunch:)`, byte for byte.
///
/// `replacing` is the installed app when the new bundle lands under another name beside it (`MightyCode.app` → `Mighty Code.app`; one
/// equal to `destination` once standardized counts as none). `running_executable` is the installed app's executable, the one the
/// running check looks for (Swift's default `MightyCode`).
pub fn install_script(
    staged_app: &Path,
    destination: &Path,
    replacing: Option<&Path>,
    running_executable: &str,
    pid: i32,
    relaunch: bool,
) -> String {
    let replaced = replacing.filter(|replacing| standardized(replacing) != standardized(destination));
    let installed = if replaced.is_none() { "$DESTINATION" } else { "$REPLACED" };
    let checked = url_path(replaced.unwrap_or(destination));
    let executable = format!("^{}( |$)", literal_pattern(&format!("{checked}/Contents/MacOS/{running_executable}")));
    let headless = format!(" ({})( |$)", HEADLESS_ARGUMENTS.map(literal_pattern).join("|"));
    let running_check = if relaunch {
        String::new()
    } else {
        format!(
            "if pgrep -lf {} 2>/dev/null | grep -Ev -- {} | grep -q .; then\n  echo \"Mighty Code is running again; leaving the update for the next quit.\" >&2\n  rm -rf \"$NEW\"\n  exit 0\nfi\n",
            quote(&executable),
            quote(&headless)
        )
    };
    let replaced_line = replaced.map(|replaced| format!("REPLACED={}\n", quote(&url_path(replaced)))).unwrap_or_default();
    let destination_too = if replaced.is_none() { "" } else { " && [ ! -e \"$DESTINATION\" ]" };
    let clear_destination = if replaced.is_none() {
        ""
    } else {
        "if [ -e \"$DESTINATION\" ]; then \"$LSREGISTER\" -u \"$DESTINATION\" >/dev/null 2>&1 || true; rm -rf \"$DESTINATION\"; fi\n"
    };
    let last = if relaunch { "open -n \"$DESTINATION\"" } else { "true" };
    let source = quote(&url_path(staged_app));
    let destination = quote(&url_path(destination));
    let mut script = String::new();
    script.push_str("#!/bin/sh\n");
    script.push_str("# MightyCode update helper: swap the bundle only after the app has quit.\n");
    script.push_str("set -u\n");
    script.push_str("PATH=/usr/bin:/bin; export PATH\n");
    script.push_str(&format!("SOURCE={source}\n"));
    script.push_str(&format!("DESTINATION={destination}\n"));
    script.push_str(&format!("{replaced_line}PID={pid}\n"));
    script.push_str("NEW=\"$DESTINATION.update-new\"\n");
    script.push_str(&format!("OLD=\"{installed}.update-old\"\n"));
    script.push_str(&format!("LSREGISTER={LSREGISTER}\n"));
    script.push_str(&format!("if [ ! -e \"{installed}\" ]{destination_too} && [ -d \"$OLD\" ]; then\n"));
    script.push_str("  echo \"Putting back the app an interrupted update left aside.\" >&2\n");
    script.push_str(&format!(
        "  mv \"$OLD\" \"{installed}\" || {{ echo \"Could not put back the previous version; it is at $OLD.\" >&2; exit 1; }}\n"
    ));
    script.push_str("fi\n");
    script.push_str("for _ in $(seq 1 600); do kill -0 \"$PID\" 2>/dev/null || break; sleep 0.5; done\n");
    script.push_str("if kill -0 \"$PID\" 2>/dev/null; then echo \"Mighty Code did not quit; skipping the update.\" >&2; exit 1; fi\n");
    script.push_str("sleep 2\n");
    script.push_str("[ -d \"$SOURCE\" ] || { echo \"No app to install: $SOURCE\" >&2; exit 1; }\n");
    script.push_str("rm -rf \"$NEW\"\n");
    script.push_str("ditto \"$SOURCE\" \"$NEW\" || { echo \"Could not copy the new app.\" >&2; rm -rf \"$NEW\"; exit 1; }\n");
    script.push_str(&format!("{running_check}trap '' HUP INT TERM\n"));
    script.push_str("if [ -e \"$OLD\" ]; then \"$LSREGISTER\" -u \"$OLD\" >/dev/null 2>&1 || true; rm -rf \"$OLD\"; fi\n");
    script.push_str(
        "if [ -e \"$OLD\" ]; then echo \"Could not remove the leftover $OLD; skipping the update.\" >&2; rm -rf \"$NEW\"; exit 1; fi\n",
    );
    script.push_str(&format!("if [ -d \"{installed}\" ]; then\n"));
    script.push_str(&format!(
        "  mv \"{installed}\" \"$OLD\" || {{ echo \"Could not back up the current app.\" >&2; rm -rf \"$NEW\"; exit 1; }}\n"
    ));
    script.push_str("fi\n");
    script.push_str(&format!("{clear_destination}if ! mv \"$NEW\" \"$DESTINATION\"; then\n"));
    script.push_str("  echo \"Could not replace the app; restoring the previous version.\" >&2\n");
    script.push_str("  rm -rf \"$NEW\"\n");
    script.push_str("  if [ -d \"$OLD\" ]; then\n");
    script.push_str(&format!("    rm -rf \"{installed}\"\n"));
    script.push_str(&format!("    mv \"$OLD\" \"{installed}\" || echo \"Could not restore the previous version; it is at $OLD.\" >&2\n"));
    script.push_str("  fi\n");
    script.push_str("  exit 1\n");
    script.push_str("fi\n");
    script.push_str("trap - HUP INT TERM\n");
    script.push_str("if [ -e \"$OLD\" ]; then\n");
    script.push_str("  \"$LSREGISTER\" -u \"$OLD\" >/dev/null 2>&1 || true\n");
    script.push_str("  rm -rf \"$OLD\" || echo \"Could not remove the previous version at $OLD.\" >&2\n");
    script.push_str("fi\n");
    script.push_str("\"$LSREGISTER\" -u \"$SOURCE\" >/dev/null 2>&1 || true\n");
    script.push_str("rm -rf \"$(dirname \"$SOURCE\")\"\n");
    script.push_str("\"$LSREGISTER\" -f \"$DESTINATION\" >/dev/null 2>&1 || true\n");
    script.push_str("echo \"Installed: $DESTINATION\"\n");
    script.push_str(last);
    script
}

/// Where the new bundle lands: beside the installed app, under the staged bundle's own name (AppStore+AppUpdate.swift:421-422).
pub fn destination(installed: &Path, staged: &Path) -> PathBuf {
    let parent = installed.parent().unwrap_or(Path::new("/"));
    staged.file_name().map(|name| parent.join(name)).unwrap_or_else(|| installed.to_path_buf())
}

/// What [`prepare`] made: the script, and the folder it and its log go into (the package's).
#[derive(Clone, PartialEq, Eq)]
pub struct MacInstall {
    pub script: String,
    pub folder: PathBuf,
}

/// The paths are left out: they name the user.
impl std::fmt::Debug for MacInstall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacInstall").field("script", &self.script.len()).finish_non_exhaustive()
    }
}

/// The Mac's checks before the helper starts (Swift `prepareAppUpdateInstall` then `verify`), in Swift's order and words: `plan`'s
/// install folder is the running `.app` and its folder is writable (`may_write`); the staged bundle passes
/// [`super::stage_mac::validate`] within the package's folder (`may_execute` asks for the executable); the script for this process
/// (`pid`, its executable's name `running_executable`, `plan.relaunch`); then, with `recheck`, the package's size and SHA-256 on disk
/// (read here: call it off any thread that must stay responsive).
pub fn prepare(
    plan: &InstallPlan,
    running_executable: &str,
    pid: i32,
    recheck: bool,
    may_write: &dyn Fn(&Path) -> bool,
    may_execute: &dyn Fn(&Path) -> bool,
    locale: &Locale,
) -> Result<MacInstall, String> {
    let installed = &plan.install_directory;
    let parent = installed.parent().unwrap_or(Path::new("/"));
    if installed.extension().and_then(|e| e.to_str()) != Some("app") || !may_write(parent) {
        return Err(locale.get_with("settings.appUpdate.errorNotWritable", &[("path", &url_path(installed))]));
    }
    let staged = &plan.staged_directory;
    let folder = plan.package_path.parent().map(Path::to_path_buf).unwrap_or_default();
    if let Err(error) = super::stage_mac::validate(staged, Some(&folder), super::stage_mac::BUNDLE_IDENTIFIER, may_execute) {
        return Err(locale.get_with("settings.appUpdate.errorPreflight", &[("error", &error.message(locale))]));
    }
    let script = install_script(staged, &destination(installed, staged), Some(installed), running_executable, pid, plan.relaunch);
    if recheck {
        let refused = |key: &str| locale.get_with("settings.appUpdate.errorInstall", &[("error", &locale.get(key))]);
        let size = std::fs::metadata(&plan.package_path).map(|m| i64::try_from(m.len()).unwrap_or(i64::MAX)).unwrap_or(-1);
        if size != plan.size {
            return Err(refused("settings.appUpdate.errorSizeMismatch"));
        }
        let digest = super::service::sha256(&plan.package_path)
            .map_err(|error| locale.get_with("settings.appUpdate.errorInstall", &[("error", &error.to_string())]))?;
        if !digest.eq_ignore_ascii_case(&plan.sha256) {
            return Err(refused("settings.appUpdate.errorDigestMismatch"));
        }
    }
    Ok(MacInstall { script, folder })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_read_as_a_file_urls_path() {
        assert_eq!(url_path(Path::new("/a/b/")), "/a/b");
        assert_eq!(url_path(Path::new("/")), "/");
        assert_eq!(standardized(Path::new("/a/./b/../c/")), "/a/c");
        assert_eq!(standardized(Path::new("/../a")), "/a");
        assert_eq!(literal_pattern("/A (1) [b].app/x$^|"), "/A \\(1\\) \\[b\\]\\.app/x\\$\\^\\|");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(url_path(Path::new("/M\u{ef}ghty \u{c571}.app/")), "/Mi\u{308}ghty \u{110b}\u{1162}\u{11b8}.app");
        assert_eq!(file_system_representation("\u{2126}\u{f900}"), "\u{2126}\u{f900}", "HFS+ leaves these ranges alone");
    }
}
