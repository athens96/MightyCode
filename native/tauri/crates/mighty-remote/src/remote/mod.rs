//! Remote workspaces v2 (C# R\Remote*.cs but the mirror and the key vault, Core\DesktopService.Remote.cs; docs/remote-workspaces.md;
//! plan §1.6, `native/contracts/fixtures/remote-v2/`): the wire, the events, validation, the strict HTTP/1.1 server and client over
//! tokio TCP (D7), the sharing host and the client that runs a remote workspace's panes (`mighty_run::service::RemoteRunner`). The
//! remote mirror is W8-G's. Owner: W8-D.

pub mod address;
pub mod client;
pub mod command_shell;
pub mod events;
pub mod features;
pub mod host;
pub mod http_server;
pub mod saved;
pub mod strings;
pub mod text;
pub mod transport;
pub mod validation;
pub mod wire;
