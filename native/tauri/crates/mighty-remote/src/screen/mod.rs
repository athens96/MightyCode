//! Screen share's protocol (C# R\ScreenShare{Hub,Policy,Availability,ReferenceScene}.cs, R\ScreenClipboard.cs; plan §1.8, D11),
//! switched off ([`ENABLED`]): while off `/m1/screen-share/*` answers 503, `screenShare` is advertised nowhere, `screen-*` frames are
//! dropped, no `turn-credentials-request` is sent, and the settings box says it is paused (the app builds no hub). What is here is
//! the platform-independent protocol and policy, driven by the checks over a fake platform: [`policy`] (records, grants, limits, the
//! control key), [`wire`] (the control channel's frames and the routes' JSON), [`clipboard`], [`hub`] and [`scene`]. The native
//! capture, input, hooks, tap marker, reference-scene window and the WebRTC transport come after 0.5 (D11). Owner: W8-F.

pub mod clipboard;
pub mod hub;
pub mod policy;
pub mod scene;
pub mod wire;

/// `ScreenShareAvailability.Enabled` (R\ScreenShareAvailability.cs:15): the Cloudflare relay has no TURN.
pub const ENABLED: bool = false;
