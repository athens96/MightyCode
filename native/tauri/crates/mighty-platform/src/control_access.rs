//! MC-43: who may reach a running app's control API (the `mighty` CLI's pipe) on Windows, and how the CLI's console is set up. Pure
//! rules only; `mighty-platform-windows` makes the pipe and reads the tokens, src-tauri's control client picks its words from them
//! when the pipe refuses it (docs/windows-cli.md).
//!
//! The rule: the pipe is this user's account alone (its owner and its one access entry are the token's *user* SID, never the token's
//! default owner, which an elevated token sets to the Administrators group), remote clients are refused, and the pipe carries the
//! app's own integrity level as a mandatory label that refuses both reading and writing from below. So:
//! - an app and a CLI at the same level connect (as before);
//! - an elevated CLI reaches the user's normal app (it gains nothing from it, and gives the app nothing it could not reach already);
//! - a normal CLI never reaches an elevated app: any program of the user could otherwise make that app's agents run commands as an
//!   administrator (a UAC bypass). The CLI says so and how to go on, instead of "not running";
//! - another account, an elevated one included, never connects (before, an elevated app's pipe let in the Administrators group);
//! - a sandboxed run (MC-42: low integrity, write-restricted) never reaches the control pipe: the label refuses it, and its
//!   restricting SIDs are not on the pipe's list. Only the agents' pipe lets the logon session's low processes in, unchanged.

use std::cmp::Ordering;

/// A Windows token's mandatory integrity level, from its label's RID (`SECURITY_MANDATORY_*_RID`), in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Integrity {
    Untrusted,
    Low,
    /// Medium, and medium-plus (0x2100).
    Medium,
    High,
    /// System and protected processes.
    System,
}

impl Integrity {
    /// The level of a label's RID: below 0x1000 untrusted, then low (0x1000), medium (0x2000, and 0x2100), high (0x3000), system.
    pub fn from_rid(rid: u32) -> Integrity {
        match rid {
            0..0x1000 => Integrity::Untrusted,
            0x1000..0x2000 => Integrity::Low,
            0x2000..0x3000 => Integrity::Medium,
            0x3000..0x4000 => Integrity::High,
            _ => Integrity::System,
        }
    }

    /// Whether this is an elevated (administrator's) token's level or above.
    pub fn elevated(self) -> bool {
        self >= Integrity::High
    }

    /// The level's SDDL name in a mandatory label ACE.
    fn sddl(self) -> &'static str {
        match self {
            Integrity::Untrusted => "S-1-16-0",
            Integrity::Low => "LW",
            Integrity::Medium => "ME",
            Integrity::High => "HI",
            Integrity::System => "SI",
        }
    }
}

/// Whether `sid` reads as a SID's text (`S-1-` and digits and dashes), so it can go into SDDL as it is.
fn is_sid_text(sid: &str) -> bool {
    sid.strip_prefix("S-1-").is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'-'))
}

/// The control pipe's security descriptor (SDDL) for an app whose token's user is `user_sid` and whose level is `app`: that user the
/// owner and the one entry (full control of a pipe, `0x1f019f`), and a mandatory label at the app's level with no-write-up and
/// no-read-up. None for a user that is not a SID's text.
pub fn control_pipe_sddl(user_sid: &str, app: Integrity) -> Option<String> {
    if !is_sid_text(user_sid) {
        return None;
    }
    Some(format!("O:{user_sid}D:(A;;0x1f019f;;;{user_sid})S:(ML;;NWNR;;;{})", app.sddl()))
}

/// Whether the rule lets a CLI reach the app its profile's endpoint names. The pipe enforces it (its label and its list); the CLI only
/// uses this to say why the pipe refused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Allowed by the elevation rule (a refusal then has another cause).
    Allowed,
    /// The app runs elevated and this CLI does not: refused, with [`NEEDS_ELEVATION`], never read as "not running".
    NeedsElevation,
}

/// Whether a CLI at `cli`'s elevation may reach an app at `app`'s: never upwards.
pub fn reach(app_elevated: bool, cli_elevated: bool) -> Reach {
    match app_elevated.cmp(&cli_elevated) {
        Ordering::Greater => Reach::NeedsElevation,
        Ordering::Less | Ordering::Equal => Reach::Allowed,
    }
}

/// What the CLI says when the app runs as an administrator and the CLI does not (the CLI's words are English, as its usage).
pub const NEEDS_ELEVATION: &str = "Mighty Code is running as administrator and this terminal is not. An app started as administrator \
    takes commands only from a terminal that is too (otherwise any program of yours could make its agents run commands as \
    administrator). Run mighty from a terminal opened with 'Run as administrator', or start Mighty Code normally.";

/// What the CLI says when the app's pipe refused it and the app is not known to be elevated (an access check failed).
pub const REFUSED: &str = "Mighty Code is running for this profile but refused this connection";

/// What `mighty server start` adds when the CLI itself is elevated: the app it starts runs as administrator too.
pub const STARTS_ELEVATED: &str = "note: this terminal runs as administrator, so the app starts as administrator; a mighty command \
    from a normal terminal will not reach it.";

/// What one of the process's standard handles is when the `--cli` mode of the windowed exe starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdHandle {
    /// None was given (a windowed program started from a console without redirection).
    Missing,
    /// A handle Windows does not take (closed, or never valid here).
    Invalid,
    /// A file (`> out.json`, `< in.txt`).
    Disk,
    /// A pipe (`| jq`, a script's capture, the `mighty` command's own handles).
    Pipe,
    /// A character device: the console itself, or `nul`.
    Character,
}

/// Whether the `--cli` mode replaces this standard handle with the console it attached to (`CONIN$`/`CONOUT$`): only where it was given
/// none it can use, so a redirection (a file, a pipe, `nul`) and a console handle it inherited are kept as they are.
pub fn reopen_on_console(handle: StdHandle) -> bool {
    matches!(handle, StdHandle::Missing | StdHandle::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrity_levels_read_from_their_rids_in_order() {
        assert_eq!(Integrity::from_rid(0), Integrity::Untrusted);
        assert_eq!(Integrity::from_rid(0x1000), Integrity::Low);
        assert_eq!(Integrity::from_rid(0x2000), Integrity::Medium);
        assert_eq!(Integrity::from_rid(0x2100), Integrity::Medium, "medium-plus");
        assert_eq!(Integrity::from_rid(0x3000), Integrity::High);
        assert_eq!(Integrity::from_rid(0x4000), Integrity::System);
        assert_eq!(Integrity::from_rid(0x5000), Integrity::System, "protected");
        assert!(Integrity::Low < Integrity::Medium && Integrity::Medium < Integrity::High);
        assert!(!Integrity::Medium.elevated() && Integrity::High.elevated() && Integrity::System.elevated());
    }

    /// The pipe is the user's (owner and entry), with the app's level as a label that refuses reads and writes from below; never the
    /// Administrators group, never the logon session's SID, never a low label (the agents' pipe's MC-42 entries).
    #[test]
    fn the_control_pipe_is_the_users_alone_at_the_apps_level() {
        let user = "S-1-5-21-1004336348-1177238915-682003330-1001";
        let elevated = control_pipe_sddl(user, Integrity::High).unwrap();
        assert_eq!(elevated, format!("O:{user}D:(A;;0x1f019f;;;{user})S:(ML;;NWNR;;;HI)"));
        let normal = control_pipe_sddl(user, Integrity::Medium).unwrap();
        assert!(normal.ends_with("S:(ML;;NWNR;;;ME)"), "{normal}");
        for sddl in [&elevated, &normal] {
            assert!(!sddl.contains("S-1-5-32-544") && !sddl.contains(";BA)") && !sddl.contains("S-1-5-5-") && !sddl.contains(";LW)"));
            assert_eq!(sddl.matches("(A;").count(), 1, "one entry: {sddl}");
        }
        for odd in ["", "BA", "S-1-", "S-1-5-21)(A;;GA;;;WD", "s-1-5-21-1", "S-1-5-21-1 "] {
            assert_eq!(control_pipe_sddl(odd, Integrity::Medium), None, "{odd:?}");
        }
    }

    /// Never upwards: a normal CLI is refused by an elevated app; an elevated CLI reaches a normal one; equals reach each other.
    #[test]
    fn a_cli_never_reaches_an_app_above_it() {
        assert_eq!(reach(true, false), Reach::NeedsElevation);
        assert_eq!(reach(false, true), Reach::Allowed);
        assert_eq!(reach(true, true), Reach::Allowed);
        assert_eq!(reach(false, false), Reach::Allowed);
        assert!(!NEEDS_ELEVATION.starts_with("Mighty Code is not running"), "never read as not running (server start would launch)");
        assert!(!REFUSED.starts_with("Mighty Code is not running"));
    }

    /// Only a handle the exe cannot use is replaced by the console; a redirection or an inherited console stays.
    #[test]
    fn only_a_missing_or_invalid_standard_handle_goes_to_the_console() {
        assert!(reopen_on_console(StdHandle::Missing));
        assert!(reopen_on_console(StdHandle::Invalid));
        assert!(!reopen_on_console(StdHandle::Disk));
        assert!(!reopen_on_console(StdHandle::Pipe));
        assert!(!reopen_on_console(StdHandle::Character), "the console it inherited, or nul");
    }
}
