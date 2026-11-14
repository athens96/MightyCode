//! A push's message (C# `MobilePushMessage` and its hand encoder, R\MobilePush.cs:145-194; plan §1.3): fields in Swift's
//! `JSONEncoder` order (`v:1, id, kind, hostId, hostName, workspace, title, summary, sessionId, [permission:{requestId,runId,buttons}],
//! at`), escaping `" \ /`, `\b \f \n \r \t`, other C0 as `\u00xx`, all else raw UTF-8; over 2048 bytes the summary empties, still over the
//! push is skipped. Owner: W8-A.
//!
//! At rest (W8 Step 0): the kinds and buttons.

/// `kind`.
pub const KINDS: [&str; 6] = ["completed", "failed", "permission", "question", "plan", "signin"];
/// `permission.buttons`.
pub const BUTTONS: [&str; 4] = ["claude", "antigravity", "antigravityOnce", "none"];
