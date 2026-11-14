//! Who gets a push (C# `MobilePushRecipients`, `MobilePushViewing`, `MobilePushWire`, R\MobilePush.cs:291-307; plan §1.3): every phone
//! with a registration whose `kinds` hold the kind, except one viewing that session (a view lasts 45 s); a direct phone's push still
//! goes through the relay URL. Owner: W8-A.

use std::collections::HashMap;
use std::time::Duration;

use crate::identity::PushDevice;
use crate::push::message::{KINDS, PushMessage};

/// `MobilePushRecipients.ViewingWindow`.
pub const VIEWING_WINDOW: Duration = Duration::from_secs(45);

/// C# `MobilePushViewing(SessionId, Until)`: "this phone has this session on screen", until `until` (milliseconds since the epoch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewing {
    pub session_id: String,
    pub until_ms: i64,
}

/// `MobilePushRecipients.Targets`: the phones that turned the message's kind on and are not viewing its session now.
pub fn targets<'a>(
    message: &PushMessage,
    devices: &'a [PushDevice],
    viewing: &HashMap<String, Viewing>,
    now_ms: i64,
) -> Vec<&'a PushDevice> {
    devices
        .iter()
        .filter(|d| d.kinds.contains(&message.kind))
        .filter(|d| !viewing.get(&d.device_id).is_some_and(|seen| seen.session_id == message.session_id && seen.until_ms > now_ms))
        .collect()
}

/// `MobilePushWire.ValidHandle`: a relay push handle is 32 random bytes, base64url without padding (43 characters).
pub fn valid_handle(value: &str) -> bool {
    mighty_platform::pairing::identity_file::valid_handle(value)
}

/// `MobilePushWire.Kinds`: the kinds this host knows, in its own order; a newer phone's unknown names are left out, not refused.
pub fn kinds<S: AsRef<str>>(values: &[S]) -> Vec<String> {
    KINDS.iter().filter(|k| values.iter().any(|v| v.as_ref() == **k)).map(|k| k.to_string()).collect()
}
