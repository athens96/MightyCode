//! MC-3: work that keeps running after the app closes. A background daemon (the app's own exe as `--daemon`) owns agent runs'
//! children and terminal panes' shells; the app is its client.
//!
//! - [`server`]: the daemon's engine over the platform's own process and PTY hosts, any listener, per-process output rings, the idle
//!   exit.
//! - [`client`]: the app's one connection: requests by id, each attached process's events through a cursor.
//! - [`owner`]: the process-owner switch. The app's hosts are wrapped once; only a run's child and a terminal pane's shell go to the
//!   daemon, and only while one is connected (in-process otherwise, exactly as before).
//!
//! The wire is mighty-core's [`mighty_core::daemon::protocol`]; the app hosts the rest (src-tauri `daemon`: the exe mode, the lock,
//! the endpoint and token files, the setting, the quit).

pub mod client;
pub mod owner;
pub mod server;
