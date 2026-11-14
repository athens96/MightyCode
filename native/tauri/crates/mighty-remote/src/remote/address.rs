//! A remote workspace's address (C# `RemoteAddress`, `RemotePeerPolicy`, `RemoteRoute`, R\RemoteAddress.cs; plan §1.6): `Parse`
//! (default port 43137), the peer policy (Tailscale only), and the route probe (a UDP connect showing the route leaves this PC's
//! Tailscale address, else `notViaTailscale`). Owner: W8-D.
//!
//! At rest (W8 Step 0): the default port only.

pub const DEFAULT_PORT: u16 = crate::remote::wire::protocol::DEFAULT_PORT;
