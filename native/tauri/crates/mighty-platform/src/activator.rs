//! The toast activator (W9, plan §1.8, D10; W6 review M3; C# `MainWindow.CompletionNotification.cs`, which used the Windows App SDK's
//! `AppNotificationManager`). Owner: W9-F.
//!
//! A click on a completion toast left in Action Center after the app quit starts the app through COM: `register` writes the AUMID's
//! `CustomActivator` and the CLSID's `LocalServer32` (`"<exe>" -Embedding`) under HKCU — a change to the machine, so only where the app
//! may change it, never in a test or a smoke — and `serve` registers the running app's class object (`CoRegisterClassObject`,
//! `REGCLS_MULTIPLEUSE`) for the process's life, so COM hands `INotificationActivationCallback::Activate(aumid, args)` to it, and starts
//! `MightyCode.exe -Embedding` only when no instance has it. The Mac answers `Unsupported` until its stage.

use std::path::Path;
use std::sync::Arc;

use crate::{Result, Subscription};

/// One activation: the AUMID and the toast's arguments as COM handed them (`sessionId=<url-encoded>`).
pub type ToastActivation = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub trait ToastActivator: Send + Sync {
    /// Registers `aumid` with the activator `clsid` (`{…}`) started as `"<exe>" -Embedding` (HKCU).
    fn register(&self, aumid: &str, clsid: &str, exe: &Path) -> Result<()>;
    /// Serves `clsid`'s class object until the subscription is dropped, calling `callback` with each activation.
    fn serve(&self, clsid: &str, callback: ToastActivation) -> Result<Subscription>;
}

/// The app's AppUserModelID, its real identity (tauri.conf.json `identifier`, the Mac's bundle id; plan D4): the toasts' sender, the
/// key `Software\Classes\AppUserModelId\<id>` and the activator's registration.
pub const AUMID: &str = "dev.mightycode.native";

/// The toast activator's class id (`Software\Classes\CLSID\{…}\LocalServer32`, the AUMID's `CustomActivator`). Generated once for
/// Mighty Code's Rust app (W9-F, plan §1.8) and fixed from then on: an installed app's registration and the toasts it left in Action
/// Center name it, so it never changes. C#'s Windows App SDK registration used a class of its own (its leftover is tolerated: args.rs
/// accepts `----AppNotificationActivated:`).
pub const CLSID: &str = "{E9E66915-2599-4971-9584-4825CAC5DEEE}";

/// The pane a toast's activation names: its arguments are the toast's `launch` (`sessionId=<url-encoded id>`, notify.rs's XML as C#
/// wrote it), read as the Windows App SDK's `AppNotificationActivatedEventArgs.Arguments` (`key=value` pairs split at `;`) with the
/// value decoded as C#'s `WebUtility.UrlDecode` undoes the `UrlEncode` it was written with (`+` a space, `%XX` a UTF-8 byte, a `%`
/// not followed by two hex digits kept). None when no `sessionId` is named.
pub fn session_id(arguments: &str) -> Option<String> {
    let value = arguments.split(';').find_map(|pair| pair.split_once('=').filter(|(key, _)| *key == "sessionId").map(|(_, v)| v))?;
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|b| (*b as char).to_digit(16));
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if hex(index + 1).is_some() && hex(index + 2).is_some() => {
                decoded.push((hex(index + 1).unwrap_or(0) * 16 + hex(index + 2).unwrap_or(0)) as u8);
                index += 2;
            }
            other => decoded.push(other),
        }
        index += 1;
    }
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The toast's own arguments name the pane, decoded as C# encoded them (notify.rs's `url_encode` is `WebUtility.UrlEncode`).
    #[test]
    fn the_session_is_read_from_the_toast_arguments() {
        assert_eq!(session_id("sessionId=5a0e0b0b-0000-4000-8000-00000000b10b").as_deref(), Some("5a0e0b0b-0000-4000-8000-00000000b10b"));
        assert_eq!(session_id("sessionId=a+b%2Fc").as_deref(), Some("a b/c"));
        assert_eq!(session_id("sessionId=%ED%95%9C%2").as_deref(), Some("한%2"), "UTF-8 bytes decoded, a short escape kept");
        assert_eq!(session_id("action=x;sessionId=%2Fp").as_deref(), Some("/p"), "pairs split at ;");
        assert_eq!(session_id("sessionId=").as_deref(), Some(""));
        assert_eq!(session_id(""), None);
        assert_eq!(session_id("SessionId=x"), None, "the key as C# names it");
        assert_eq!(session_id("other=sessionId=x"), None);
    }

    #[test]
    fn the_class_id_is_one_fixed_guid() {
        let inner = CLSID.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')).expect("braced");
        let groups: Vec<usize> = inner.split('-').map(str::len).collect();
        assert_eq!(groups, [8, 4, 4, 4, 12]);
        assert!(inner.chars().all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
    }
}
