//! The direct path's constants (C# `MobileDirect`, R\MobileDirect.cs; plan §1.4): plain `ws://<endpoint>/ws?v=1` (the relay handshake
//! inside it, the phone pinning the host key), port 43139 (1024-65535), one listener per Tailscale address with backlog 36, at most 8
//! endpoints advertised (IPv6 in brackets), connection ids `direct-…`. Owner: W8-C.

use std::time::Duration;

/// `MobileDirect.DefaultPort` (mighty-core's `MOBILE_DIRECT_PORT`).
pub const DEFAULT_PORT: u16 = mighty_core::model::MOBILE_DIRECT_PORT as u16;
pub const BACKLOG: u32 = 36;
pub const MAXIMUM_ENDPOINTS: usize = 8;
/// One deadline covers the socket, hello and auth.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);
/// Tailscale's addresses are looked at again this often (and on every address change).
pub const REDETECT_INTERVAL: Duration = Duration::from_secs(15);
/// A direct connection's id starts so; relay notices never open or close one.
pub const CONNECTION_PREFIX: &str = "direct-";
