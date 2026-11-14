//! The client's strict HTTP/1.1 transport (C# `RemoteTransport`, R\RemoteTransport.cs; plan §1.6, D7): one request per connection over
//! tokio TCP, a fixed header set (the key only in `Authorization: Bearer`, never in a URL, a log or `Debug`), no proxy (`UseProxy =
//! false`), no redirect, at most 2 MiB read; the address resolved once (a literal, or MagicDNS within 5 s; every result Tailscale's,
//! IPv4 preferred). Owner: W8-D.
//!
//! At rest (W8 Step 0): empty.
