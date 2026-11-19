//! The Mac's install script (Swift `SetupScript`, `SetupScriptLabels`, SetupPlan.swift:44-196): one bash script built from the
//! catalog's `macos` entries only, every value in POSIX single quotes, so nothing the user typed or a file outside the bundled catalog
//! reaches it. It runs in a terminal pane of the window, where the user watches it and answers what a step asks (a password for an
//! installer's sudo, Homebrew's Enter, the Command Line Tools' system window): the app never adds sudo and never sees a password.
//! Every step runs in a subshell of its own, so a failed one is said and the next one runs.

use std::collections::BTreeMap;

use mighty_run::strings;

use crate::setup::catalog::{SetupCatalog, SetupItem, SetupPlatform, SetupPlatformEntry, SetupProbe, SetupRecipe};
use crate::setup::probe::{SKILL_DONE_MARKER, SKILL_INSTALLING_MARKER, XCODE_SELECT};
use crate::setup::script::{NPM_REGISTRY, SetupScriptStep, StepLabel};

/// curl over https only, redirects included, TLS 1.2 or newer (Swift `SetupScript.curl`).
pub const CURL: &str = "/usr/bin/curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL";
/// How long the script waits for the system window to finish the Command Line Tools (Swift `outsideWaitSeconds`).
pub const OUTSIDE_WAIT_SECONDS: u32 = 1800;
/// The shell the script runs in.
pub const BASH: &str = "/bin/bash";

/// The words the Mac's script prints, in the app's language.
pub struct MacScriptLabels {
    pub step: StepLabel,
    pub failed: String,
    /// Said before a step that waits for a system window.
    pub waiting: String,
    pub done: String,
}

impl MacScriptLabels {
    pub fn localized() -> MacScriptLabels {
        MacScriptLabels {
            step: Box::new(|index, count, title| format!("[{index}/{count}] {title}")),
            failed: strings::get("setup.script.failed"),
            waiting: strings::get("setup.script.waiting"),
            done: strings::get("setup.script.done"),
        }
    }
}

/// POSIX single quotes: the text is taken literally; a quote inside closes the quotes, is escaped and opens them again.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Where the official installers put the CLIs (Swift `CLIEnvironmentResolver.wellKnownFolders`, mighty-platform-macos
/// `environment::well_known_folders`): appended after the PATH the shell already has.
pub fn well_known_folders(home: &str) -> Vec<String> {
    vec![
        format!("{home}/.local/bin"),
        format!("{home}/.npm-global/bin"),
        format!("{home}/.bun/bin"),
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
    ]
}

/// The home folder the script's preamble names: the CLI environment's `HOME`, else this process's.
pub fn home(environment: &BTreeMap<String, String>) -> String {
    environment.get("HOME").filter(|home| !home.is_empty()).cloned().or_else(|| std::env::var("HOME").ok()).unwrap_or_default()
}

/// The lines that run before any step, shown in the confirmation too (Swift `SetupScript.preamble`): the home folder as the working
/// folder (not a project whose settings could steer an install), the installers' folders after the PATH, npm's public registry.
pub fn preamble(home: &str) -> Vec<String> {
    let folders: String = well_known_folders(home).iter().map(|folder| format!(":{}", quote(folder))).collect();
    vec![
        "cd \"$HOME\" || exit 1".to_string(),
        format!("export PATH=\"$PATH\"{folders}"),
        format!("export npm_config_registry={}", quote(NPM_REGISTRY)),
    ]
}

/// The steps for `ids` in that order; items that only open a page, or have no Mac entry, are left out.
pub fn steps(ids: &[String], catalog: &SetupCatalog, title: &dyn Fn(&SetupItem) -> String) -> Vec<SetupScriptStep> {
    ids.iter()
        .filter_map(|id| {
            let item = catalog.item(id)?;
            let entry = item.entry(SetupPlatform::MacOS)?;
            let body = body(entry)?;
            Some(SetupScriptStep { item_id: id.clone(), title: title(item), body, waits_outside: waits_outside(entry) })
        })
        .collect()
}

/// The step hands the install to a system window (the Command Line Tools' `xcode-select --install`): the script waits for it.
pub fn waits_outside(entry: &SetupPlatformEntry) -> bool {
    matches!(entry.install, SetupRecipe::Argv { finishes_outside: true, .. }) && entry.probe == SetupProbe::CommandLineTools
}

/// The absolute path the script calls an argv program by (Swift `absolutePrograms`).
fn absolute(program: &str) -> &str {
    if program == "xcode-select" { XCODE_SELECT } else { program }
}

/// What one Mac entry runs, or None when nothing runs in the terminal (Swift `SetupScript.body`).
pub fn body(entry: &SetupPlatformEntry) -> Option<String> {
    match &entry.install {
        SetupRecipe::Argv { arguments, .. } => {
            let (program, rest) = arguments.split_first()?;
            let command =
                std::iter::once(absolute(program)).chain(rest.iter().map(String::as_str)).map(quote).collect::<Vec<_>>().join(" ");
            // The system window finishes the install; later steps wait for it, up to half an hour. Control-C ends only this wait
            // (`render` keeps the script going).
            if waits_outside(entry) {
                return Some(format!(
                    "{command}; trap 'break' INT; mighty_waited=0; until {XCODE_SELECT} -p >/dev/null 2>&1 || [ \"$mighty_waited\" -ge {OUTSIDE_WAIT_SECONDS} ]; do sleep 5; mighty_waited=$((mighty_waited + 5)); done; trap - INT"
                ));
            }
            Some(command)
        }
        // Fetched first and handed to the shell as its script, so the script's own questions (a password, a yes/no) still read the
        // terminal. The options the app gives it follow a name for `$0`.
        SetupRecipe::OfficialScript { url, shell } => {
            let interpreter = if shell == "sh" { "/bin/sh" } else { BASH };
            let tail = SetupCatalog::script_arguments(url.as_str())
                .map(|options| format!(" mighty-install {}", options.iter().map(|o| quote(o)).collect::<Vec<_>>().join(" ")))
                .unwrap_or_default();
            Some(format!("mighty_script=\"$({CURL} {})\" && {interpreter} -c \"$mighty_script\"{tail}", quote(url.as_str())))
        }
        SetupRecipe::BrewFormula { name } => Some(format!("brew install {}", quote(name))),
        SetupRecipe::BrewCask { name } => Some(format!("brew install --cask {}", quote(name))),
        // A package that takes over another's command removes that one first; one that is not there is no error.
        SetupRecipe::NpmGlobal { package } => {
            let install = format!("npm install -g {}", quote(package));
            Some(match SetupCatalog::npm_replaces(package) {
                Some(old) => format!("npm uninstall -g {} >/dev/null 2>&1; {install}", quote(old)),
                None => install,
            })
        }
        // An already added marketplace is fine; the install decides. The marketplace, not the app, decides the plugin's version.
        SetupRecipe::ClaudePlugin { marketplace, plugin } => {
            Some(format!("claude plugin marketplace add {}; claude plugin install {} --scope user", quote(marketplace), quote(plugin)))
        }
        // An existing clone moves to the pinned commit; otherwise the commit is fetched into a temporary folder that is moved into
        // place only when complete. The folder counts once its setup finished (the probe's markers).
        SetupRecipe::Skill { repository, folder, commit, setup } => {
            let destination = format!("\"$HOME\"/{}", quote(&format!(".claude/skills/{folder}")));
            let (url, sha) = (quote(repository.as_str()), quote(commit));
            let installing = format!("{destination}/{}", quote(SKILL_INSTALLING_MARKER));
            let done = format!("{destination}/{}", quote(SKILL_DONE_MARKER));
            let mut body = format!(
                "if [ -d {destination}/.git ]; then git -C {destination} fetch -q --depth 1 {url} {sha} && git -C {destination} checkout -q --detach FETCH_HEAD; \
                 else mighty_tmp=\"$(mktemp -d)\" && git init -q \"$mighty_tmp/repo\" && git -C \"$mighty_tmp/repo\" fetch -q --depth 1 {url} {sha} && git -C \"$mighty_tmp/repo\" checkout -q --detach FETCH_HEAD \
                 && mkdir -p \"$HOME\"/{skills} && mv \"$mighty_tmp/repo\" {destination}; fi && rm -f {done} && : > {installing}",
                skills = quote(".claude/skills"),
            );
            if let Some(setup) = setup {
                body.push_str(&format!(" && (cd {destination} && {})", quote(&format!("./{setup}"))));
            }
            body.push_str(&format!(" && : > {done}"));
            Some(body)
        }
        // A Windows install, and a page the app opens: nothing runs in the Mac's terminal.
        SetupRecipe::Winget { .. } | SetupRecipe::VendorPage { .. } => None,
    }
}

/// The whole script (Swift `SetupScript.render`). `started_marker` is made first thing and `done_marker` when the script ends for any
/// reason (finished, a failed step, Control-C, the pane closed); both are folders made with mkdir, which never follows a link that is
/// already there. LF line ends, one after the last line.
pub fn render(steps: &[SetupScriptStep], labels: &MacScriptLabels, home: &str, started_marker: &str, done_marker: &str) -> String {
    let mut lines = vec![
        "#!/bin/bash".to_string(),
        format!("mkdir -- {} 2>/dev/null", quote(started_marker)),
        "# Written by Mighty Code from its bundled tool catalog (setup-catalog.json).".to_string(),
        format!("mighty_marker={}", quote(done_marker)),
        "trap 'mkdir -- \"$mighty_marker\" 2>/dev/null' EXIT".to_string(),
    ];
    lines.extend(preamble(home));
    lines.push("mighty_failed=0".to_string());
    for (index, step) in steps.iter().enumerate() {
        lines.push(format!("printf '\\n\\033[1m%s\\033[0m\\n' {}", quote(&(labels.step)(index + 1, steps.len(), &step.title))));
        if step.waits_outside {
            lines.push(format!("printf '%s\\n' {}", quote(&labels.waiting)));
            // Control-C reaches the whole group: this shell only notes it, and the step's own trap ends the wait.
            lines.push("trap ':' INT".to_string());
        }
        lines.push(format!("if ( {} ); then :; else mighty_failed=1; printf '%s\\n' {}; fi", step.body, quote(&labels.failed)));
        if step.waits_outside {
            lines.push("trap - INT".to_string());
        }
    }
    lines.push(format!("printf '\\n%s\\n' {}", quote(&labels.done)));
    lines.push("exit $mighty_failed".to_string());
    lines.join("\n") + "\n"
}

/// The command the terminal pane runs: bash on the written script (the path is never read as code).
pub fn launch_arguments(script_path: &str) -> Vec<String> {
    vec![BASH.to_string(), script_path.to_string()]
}

/// The script of a pane's launch ([`launch_arguments`]), for the Terminal.app window it falls back to without a local workspace:
/// the script file alone (its `#!/bin/bash` line runs it there). None for any other launch.
pub fn terminal_app_arguments(pane_arguments: &[String]) -> Option<Vec<String>> {
    match pane_arguments {
        [bash, script] if bash == BASH => Some(vec![script.clone()]),
        _ => None,
    }
}
