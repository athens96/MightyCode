//! MC-33: a connection to a Mighty Code server on an SSH machine, through an `ssh -L` forward rather than Tailscale or the relay.
//!
//! The app gives the client a way to open the way there ([`SshTunnel`], `ClientOptions::ssh_tunnel`): for a machine it starts (or
//! finds) the forward of a local loopback port to the far side's tunnel listener (mighty-remote's `RemoteHost::new_ssh_tunnel`) and
//! asks the far side's app for a one-time pairing code over the same ssh. The client then spends the code on `POST /v2/ssh/pair`
//! through the forward for the sharing's key, and reads `/v2/info` with that key as over Tailscale. Every connect and reconnect does
//! this again: the key is never written to the vault or the connections file (a server that restarted has a new one anyway).
//!
//! Such a connection is saved under [`ssh_address`] (`ssh:` and the machine's id). Its target is loopback, built here in code (never
//! from what the person typed), so no Tailscale route is asked for it.

use std::sync::Arc;

use mighty_platform::BoxFuture;
use mighty_run::strings::get;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::remote::address::RemoteAddress;
use crate::remote::client::{ClientError, RemoteClient};
use crate::remote::transport::{self, Method, RemoteTarget, Request};
use crate::remote::validation;
use crate::remote::wire::{RemoteWireInfo, protocol};

/// An SSH connection's address starts so.
pub const SSH_ADDRESS_PREFIX: &str = "ssh:";

/// What the app's [`SshTunnel`] opened: the local port of the forward and a fresh pairing code from the far side.
pub struct SshOpening {
    pub port: u16,
    pub code: Zeroizing<String>,
}

impl SshOpening {
    pub fn new(port: u16, code: String) -> SshOpening {
        SshOpening { port, code: Zeroizing::new(code) }
    }
}

/// The code is left out.
impl std::fmt::Debug for SshOpening {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshOpening").field("port", &self.port).finish_non_exhaustive()
    }
}

/// The app's way to a machine's server (its id): the forward and a code; Err with the words to show.
pub type SshTunnel = Arc<dyn Fn(String) -> BoxFuture<'static, Result<SshOpening, String>> + Send + Sync>;

/// MC-40: told (with the machine's id) when a connected SSH machine's connection was lost by its own requests — the server behind
/// the forward is gone or no longer knows the key (the mirror's or a run's request failed past its retries, or was answered 401),
/// while the ssh forward itself may still be up (the far side's app restarted on another port). The app pairs again over ssh and
/// reconnects with its reconnect's gaps. Never told for a person's 연결 끊기 or removal, for a connect or refresh that failed (the
/// reconnect's own tries among them), or while the client closes. Called outside every lock; brief (it only starts the app's work).
pub type SshLost = Arc<dyn Fn(String) + Send + Sync>;

/// The address an SSH machine's server connection is saved and shown under: `ssh:` and the machine's id.
pub fn ssh_address(machine: &str) -> String {
    format!("{SSH_ADDRESS_PREFIX}{machine}")
}

/// The machine of an SSH connection's address (`ssh:` and a 32-hex-digit id); None for any other address.
pub fn ssh_machine(address: &str) -> Option<&str> {
    address.strip_prefix(SSH_ADDRESS_PREFIX).filter(|id| mighty_core::wire::identifier(id))
}

pub fn is_ssh_address(address: &str) -> bool {
    ssh_machine(address).is_some()
}

/// The forward's local end: loopback, made here (the transport sends a key to loopback only for a target built so).
pub fn loopback_target(port: u16) -> RemoteTarget {
    RemoteTarget::new(RemoteAddress { host: "127.0.0.1".into(), port }, "127.0.0.1", port, true)
}

impl RemoteClient {
    /// The way to `machine`'s server opened by the app, the code spent for the key, and the server's /info read and checked as over
    /// Tailscale. The key is the caller's to keep in memory only.
    pub(crate) async fn reach_ssh(&self, machine: &str) -> Result<(RemoteTarget, Zeroizing<String>, RemoteWireInfo), ClientError> {
        let tunnel = self.0.options.ssh_tunnel.clone().ok_or_else(|| ClientError::Message(get("remote.ssh.error.unavailable")))?;
        let opening = tunnel(machine.to_string()).await.map_err(ClientError::Message)?;
        let target = loopback_target(opening.port);
        let path = format!("{}{}", protocol::PATH_PREFIX, crate::remote::host::SSH_PAIR_ROUTE);
        let data =
            transport::request(&target, &opening.code, Method::Post, &path, Request::default()).await.map_err(ClientError::Transport)?;
        let key = serde_json::from_slice::<Value>(&data)
            .ok()
            .and_then(|v| v.get("key").and_then(Value::as_str).map(|k| Zeroizing::new(k.to_string())))
            .filter(|k| validation::key(Some(k)))
            .ok_or_else(|| ClientError::Message(get("remote.connect.error.notMighty")))?;
        let data = transport::request(&target, &key, Method::Get, &format!("{}info", protocol::PATH_PREFIX), Request::default())
            .await
            .map_err(ClientError::Transport)?;
        let mut info = RemoteWireInfo::decode(&data).map_err(|_| ClientError::Message(get("remote.connect.error.notMighty")))?;
        info.runtime = validation::bounded(&info.runtime);
        validation::info(&info).map_err(|e| ClientError::Message(e.0))?;
        Ok((target, key, info))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ssh_address_names_one_machine() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(ssh_address(id), format!("ssh:{id}"));
        assert_eq!(ssh_machine(&ssh_address(id)), Some(id));
        assert!(!is_ssh_address("ssh:../x") && !is_ssh_address("relay:abc") && !is_ssh_address("100.64.0.1:43137"));
        let opening = SshOpening { port: 4000, code: Zeroizing::new("secret-code".into()) };
        assert!(!format!("{opening:?}").contains("secret-code"));
    }
}
