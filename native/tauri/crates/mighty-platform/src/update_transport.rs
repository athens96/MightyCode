//! The app update's HTTPS (W9, plan §1.3, D7; C# `AppUpdateService`'s `SocketsHttpHandler{AllowAutoRedirect=false, UseProxy=false}`).
//! Owner: W9-B (the port and the Windows WinHTTP implementation); W9-A's `app_update::service` is its one caller.
//!
//! Not [`crate::HttpsSend`]: that one takes the system proxy, refuses redirects and answers a `String` body, which suits the account
//! usage and push but not a 512 MiB package. Contract:
//! - only `https://` addresses: anything else is refused (`Invalid`) before a connection is made;
//! - no proxy, no cookies, no automatic logon, TLS from the system;
//! - a redirect is never followed: a 3xx comes back with its `Location`, and the caller follows it by C#'s rule (https only, 5 hops);
//! - `get_capped` reads at most `cap` bytes of a body (more: `TooLarge`), within `timeout`;
//! - `download` writes a 2xx body to `destination` as it arrives (created or truncated), counting: past `limit` it stops with
//!   `TooLarge`, calls `progress` with the bytes written so far, stops when `cancel` is set (`Refused("cancelled")`), and ends within
//!   `timeout`; a non-2xx answer writes nothing and comes back with its status. Removing a partial file is the caller's.
//! - no URL is ever logged or put in an error.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::{BoxFuture, Result};

/// An answer to a GET.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateResponse {
    pub status: u16,
    /// A 3xx's `Location`, as sent.
    pub location: Option<String>,
    /// A 2xx's body (empty otherwise, or for a download, which writes it to its file).
    pub body: Vec<u8>,
}

/// Bytes written so far, called as they arrive (the caller rounds to whole percents).
pub type DownloadProgress = Arc<dyn Fn(u64) + Send + Sync>;

/// A package download.
#[derive(Clone)]
pub struct DownloadRequest {
    pub url: String,
    pub destination: PathBuf,
    /// The most bytes taken (the asset's size, at most 512 MiB).
    pub limit: u64,
    pub timeout: Duration,
    pub progress: DownloadProgress,
    pub cancel: Arc<AtomicBool>,
}

/// The URL is left out: it may carry a token.
impl std::fmt::Debug for DownloadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadRequest")
            .field("destination", &self.destination)
            .field("limit", &self.limit)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

/// What a download answered: the status, a 3xx's `Location`, and how many bytes it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    pub status: u16,
    pub location: Option<String>,
    pub written: u64,
}

pub trait UpdateTransport: Send + Sync {
    fn get_capped(&self, url: &str, cap: usize, timeout: Duration) -> BoxFuture<'_, Result<UpdateResponse>>;
    fn download(&self, request: DownloadRequest) -> BoxFuture<'_, Result<Downloaded>>;
}
