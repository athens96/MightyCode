//! The direct path's words on Windows (C# `MobileDirectStrings`, `MobileDirectStatusText`, R\MobileDirectStrings.cs:11-77,
//! R\MobileDirect.cs:189-199; plan §1.4): the 21 `mobileRemote.direct.windows.*` keys only the Windows app says (this PC instead of this
//! Mac, the firewall rule's row, why the path could not listen). The switch, the port label and the status line reuse the Mac's
//! `mobileRemote.direct.*` keys. No raw OS text and no unlisted code ever reaches the screen. Owner: W8-C.

use mighty_core::locale::Locale;
use mighty_platform::{FirewallChange, FirewallRule};

use super::status::{ACCESS_DENIED, ADAPTERS_UNREADABLE, ADDRESS_IN_USE, DirectState, MobileDirectStatus};

/// `MobileDirectStrings.Keys`.
pub const KEYS: [&str; 21] = [
    "mobileRemote.direct.windows.description",
    "mobileRemote.direct.windows.portHint",
    "mobileRemote.direct.windows.firewall.description",
    "mobileRemote.direct.windows.firewall.present",
    "mobileRemote.direct.windows.firewall.absent",
    "mobileRemote.direct.windows.firewall.unknown",
    "mobileRemote.direct.windows.firewall.addButton",
    "mobileRemote.direct.windows.firewall.removeButton",
    "mobileRemote.direct.windows.firewall.cancelled",
    "mobileRemote.direct.windows.firewall.failedTemplate",
    "mobileRemote.direct.windows.firewall.badProgram",
    "mobileRemote.direct.windows.firewall.noPorts",
    "mobileRemote.direct.windows.firewall.badPorts",
    "mobileRemote.direct.windows.firewall.timeout",
    "mobileRemote.direct.windows.firewall.queryFailed",
    "mobileRemote.direct.windows.firewall.noSystemDirectory",
    "mobileRemote.direct.windows.firewall.failedUnknown",
    "mobileRemote.direct.windows.error.addressInUse",
    "mobileRemote.direct.windows.error.accessDenied",
    "mobileRemote.direct.windows.error.bindFailed",
    "mobileRemote.direct.windows.error.adaptersUnreadable",
];

pub fn description(locale: &Locale) -> String {
    locale.get("mobileRemote.direct.windows.description")
}

pub fn port_hint(locale: &Locale) -> String {
    locale.get("mobileRemote.direct.windows.portHint")
}

pub fn firewall_description(locale: &Locale) -> String {
    locale.get("mobileRemote.direct.windows.firewall.description")
}

/// `MobileDirectStrings.FirewallState`.
pub fn firewall_state(locale: &Locale, rule: &FirewallRule) -> String {
    locale.get(match rule {
        FirewallRule::Present { .. } => "mobileRemote.direct.windows.firewall.present",
        FirewallRule::Missing => "mobileRemote.direct.windows.firewall.absent",
        FirewallRule::Unreadable => "mobileRemote.direct.windows.firewall.unknown",
    })
}

/// `MobileDirectStrings.FirewallRecheckButton`: the row's button while the rule could not be read, the setup screen's own words.
pub fn firewall_recheck_button(locale: &Locale) -> String {
    locale.get("setup.button.recheck")
}

/// `MobileDirectStrings.FirewallButton`: remove while the rule is there, add otherwise.
pub fn firewall_button(locale: &Locale, rule: &FirewallRule) -> String {
    locale.get(match rule {
        FirewallRule::Present { .. } => "mobileRemote.direct.windows.firewall.removeButton",
        _ => "mobileRemote.direct.windows.firewall.addButton",
    })
}

/// `MobileDirectStrings.Error`: why the path could not listen, each code its own words; anything else (no code, or text that is not
/// one) says the general bind failure.
pub fn error(locale: &Locale, code: Option<&str>) -> String {
    locale.get(match code {
        Some(ADDRESS_IN_USE) => "mobileRemote.direct.windows.error.addressInUse",
        Some(ACCESS_DENIED) => "mobileRemote.direct.windows.error.accessDenied",
        Some(ADAPTERS_UNREADABLE) => "mobileRemote.direct.windows.error.adaptersUnreadable",
        _ => "mobileRemote.direct.windows.error.bindFailed",
    })
}

/// A netsh exit code as C# quotes it: ASCII digits (never a sign) that fit an `int`.
fn exit_code(detail: &str) -> Option<i32> {
    (1..=10).contains(&detail.len()).then_some(())?;
    detail.bytes().all(|b| b.is_ascii_digit()).then_some(())?;
    detail.parse::<i32>().ok()
}

/// `MobileDirectStrings.FirewallOutcome`: what a change that did not happen says; none after one that did. Every detail code has its
/// own words; only a netsh exit code is quoted, as the template's number; a launch error's words and any unlisted code say the
/// unknown-failure words.
pub fn firewall_outcome(locale: &Locale, change: &FirewallChange) -> Option<String> {
    Some(match change {
        FirewallChange::Applied(_) => return None,
        FirewallChange::Declined => locale.get("mobileRemote.direct.windows.firewall.cancelled"),
        FirewallChange::Refused(detail) => locale.get(match detail.as_str() {
            "noPorts" => "mobileRemote.direct.windows.firewall.noPorts",
            "badPorts" => "mobileRemote.direct.windows.firewall.badPorts",
            "badProgram" => "mobileRemote.direct.windows.firewall.badProgram",
            "noSystemDirectory" => "mobileRemote.direct.windows.firewall.noSystemDirectory",
            _ => "mobileRemote.direct.windows.firewall.failedUnknown",
        }),
        FirewallChange::Failed(detail) => match detail.as_str() {
            "timeout" => locale.get("mobileRemote.direct.windows.firewall.timeout"),
            "queryFailed" => locale.get("mobileRemote.direct.windows.firewall.queryFailed"),
            other => match exit_code(other) {
                Some(code) => locale.get_with("mobileRemote.direct.windows.firewall.failedTemplate", &[("error", &code.to_string())]),
                None => locale.get("mobileRemote.direct.windows.firewall.failedUnknown"),
            },
        },
    })
}

/// `MobileDirectStatusText.Detail`: the line under the direct switch, in the Mac's words, with the Windows words for why it could not
/// listen; nothing while off.
pub fn status_detail(locale: &Locale, status: &MobileDirectStatus) -> String {
    match status.state {
        DirectState::Disabled => locale.get("mobileRemote.direct.status.off"),
        DirectState::NotFound => locale.get("mobileRemote.direct.status.notFound"),
        DirectState::Listening => {
            locale.get_with("mobileRemote.direct.status.listeningTemplate", &[("addresses", &status.endpoints.join(", "))])
        }
        DirectState::Failed => {
            locale.get_with("mobileRemote.direct.status.failedTemplate", &[("error", &error(locale, status.error.as_deref()))])
        }
        DirectState::Off => String::new(),
    }
}
