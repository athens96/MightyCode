//! The phone remote, remote workspaces v2 and screen share's protocol (W8, `.omc/research/tauri/w8-plan.md`; C# Core\Remote\*.cs): no
//! Tauri and no `cfg(target_os)`; every OS call goes through mighty-platform's ports (`Network`, `ProxyResolver`, `HttpsSend`,
//! `PairingStore`, `Firewall`, `SecretVault`, `ScreenCapture`, `RemoteInput`). The app's halves are src-tauri `mobile.rs`, `phone.rs`,
//! `direct.rs`, `remote.rs`.
//!
//! | Module | C# | Owner |
//! |---|---|---|
//! | [`relay`] | `RelayCrypto`, `RelayLink`, `MobileRelayHost` (cipher, link policy, host loop, one connection, status) | W8-A |
//! | [`identity`] | `MobileIdentity` (devices, auth, rotation, pairing URL, push registrations) over `PairingStore` | W8-A |
//! | [`push`] | `MobilePush`, `MobilePushHub` (seal, message, planner, recipients, transport, hub) | W8-A |
//! | [`phone`] | `MobileDesktopRouter`, `MobileMighty`, `MobileFlow`, `MobileRevisions`, `MobileUploads`, `MobileWorkspaceFiles`,
//! `PhoneAppUpdate` | W8-B |
//! | [`direct`] | `MobileDirect`, `MobileDirectListener`, `MobileDirectStrings` (wire, admission, upgrade, listener, status, words) | W8-C
//! |
//! | [`tailscale`] | `TailscaleAddresses` | W8-C |
//! | [`remote`] | `Remote*.cs` but the mirror and the key vault (wire, events, validation, text, words, command shell, features, address,
//! strict HTTP server and client, host, client, saved connections) | W8-D |
//! | [`screen`] | `ScreenShare{Hub,Policy,Availability,ReferenceScene}`, `ScreenClipboard` (switched off) | W8-F |
//!
//! W8 Step 0 wrote every module with its C# source and owner, and whole: the m1 router's desk (`phone::desk::PhoneDesk`), a
//! connection's handler (`relay::connection::{RequestHandler, MobileReply, MobileDevice}`) and remote v2's records (`remote::wire`).
//! Everything else is at rest: nothing connects, listens, writes or pushes until its part fills it.
//!
//! Gating (plan §4): no action here that reaches outside the process runs unless the app allows it (`Host::may_reach_relay`,
//! `Host::may_listen`, `--allow-state-write`); tests and the smoke use loopback, port 0, the fake relay and the fakes. Secrets (keys,
//! the host token, the pairing key, device tokens, push handles, token hashes, remote keys) never reach a log, an error or `Debug`, and
//! no URL is ever logged (the relay's carries the host token).
#![deny(clippy::await_holding_lock)]

pub mod direct;
pub mod identity;
pub mod phone;
pub mod push;
pub mod relay;
pub mod remote;
pub mod screen;
pub mod tailscale;
