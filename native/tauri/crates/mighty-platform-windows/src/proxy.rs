//! The system proxy on Windows (plan §0.2, D5; .NET's `HttpClient.DefaultProxy` on Windows): the current user's settings
//! (`WinHttpGetIEProxyConfigForCurrentUser`: a manual proxy and its bypass list), then automatic detection and a PAC script
//! (`WinHttpGetProxyForUrl`), for the relay's WebSocket and push. Owner: W8-A.
//!
//! The URL asked about is never logged nor put into an error (the relay's carries the host token). A `ws://`/`wss://` URL is asked about
//! as the `http://`/`https://` one a PAC script knows. Loopback and `localhost` always go straight, as .NET's do. Automatic
//! configuration that cannot be run falls back to the manual proxy, then to none.

use std::ptr::{null, null_mut};

use mighty_platform::{BoxFuture, PlatformError, Proxy, ProxyResolver, Result};
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_NAMED_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_AUTO_DETECT_TYPE_DHCP, WINHTTP_AUTO_DETECT_TYPE_DNS_A,
    WINHTTP_AUTOPROXY_AUTO_DETECT, WINHTTP_AUTOPROXY_CONFIG_URL, WINHTTP_AUTOPROXY_OPTIONS, WINHTTP_CURRENT_USER_IE_PROXY_CONFIG,
    WINHTTP_PROXY_INFO, WinHttpCloseHandle, WinHttpGetIEProxyConfigForCurrentUser, WinHttpGetProxyForUrl, WinHttpOpen,
};

use crate::win::wide;

#[derive(Debug, Default)]
pub struct WinHttpProxy;

impl ProxyResolver for WinHttpProxy {
    fn resolve(&self, url: &str) -> BoxFuture<'_, Result<Proxy>> {
        let url = url.to_string();
        Box::pin(async move {
            // WPAD may take seconds: off the async workers.
            tokio::task::spawn_blocking(move || resolve(&url))
                .await
                .map_err(|_| PlatformError::Io(std::io::Error::other("the proxy lookup's thread ended")))?
        })
    }
}

/// A WinHTTP-allocated string, freed with `GlobalFree`.
struct Owned(*mut u16);

impl Owned {
    fn text(&self) -> Option<String> {
        if self.0.is_null() {
            return None;
        }
        // SAFETY: WinHTTP returned a NUL-terminated wide string at this address, alive until it is freed below.
        let length = (0..).take_while(|&at| unsafe { *self.0.add(at) } != 0).count();
        let units = unsafe { std::slice::from_raw_parts(self.0, length) };
        Some(String::from_utf16_lossy(units)).filter(|text| !text.trim().is_empty())
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by WinHTTP with GlobalAlloc; freed once.
            unsafe { GlobalFree(self.0.cast()) };
        }
    }
}

/// Whether `url` is TLS, its host (lower case, no brackets), and what a PAC script is asked about: `http(s)://host[:port]/` only, the
/// scheme it knows (`ws` as `http`, `wss` as `https`) and never the path or query (the relay's carries the host token; review L1).
fn target(url: &str) -> Option<(bool, String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let secure = match scheme.to_ascii_lowercase().as_str() {
        "wss" | "https" => true,
        "ws" | "http" => false,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = match authority.strip_prefix('[') {
        Some(inside) => inside.split_once(']')?.0,
        None => authority.rsplit_once(':').map_or(authority, |(host, _)| host),
    };
    (!host.is_empty()).then(|| (secure, host.to_ascii_lowercase(), format!("{}://{authority}/", if secure { "https" } else { "http" })))
}

fn resolve(url: &str) -> Result<Proxy> {
    let Some((secure, host, asked)) = target(url) else { return Ok(Proxy::Direct) };
    if host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
        return Ok(Proxy::Direct);
    }
    let mut config = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    // SAFETY: `config` is written by the call; its strings are freed by `Owned`.
    let read = unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut config) } != 0;
    let (auto_url, manual, bypass) = (Owned(config.lpszAutoConfigUrl), Owned(config.lpszProxy), Owned(config.lpszProxyBypass));
    let auto_detect = read && config.fAutoDetect != 0;
    if (auto_detect || auto_url.text().is_some())
        && let Some(found) = automatic(&asked, auto_detect, auto_url.text().as_deref())
    {
        return Ok(found.and_then(|list| pick(&list, secure)).unwrap_or(Proxy::Direct));
    }
    let Some(list) = manual.text() else { return Ok(Proxy::Direct) };
    if bypass.text().is_some_and(|bypass| bypassed(&bypass, &host)) {
        return Ok(Proxy::Direct);
    }
    Ok(pick(&list, secure).unwrap_or(Proxy::Direct))
}

/// `WinHttpGetProxyForUrl` with automatic detection and/or the PAC address: Some(None) for "go straight", Some(Some(list)) for a proxy
/// list, None when the automatic configuration could not be run.
fn automatic(url: &str, detect: bool, script: Option<&str>) -> Option<Option<String>> {
    let agent = wide("MightyCode");
    // SAFETY: a session for the lookup only, closed below.
    let session = unsafe { WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, null(), null(), 0) };
    if session.is_null() {
        return None;
    }
    let script = script.map(wide);
    let mut options = WINHTTP_AUTOPROXY_OPTIONS {
        dwFlags: (if detect { WINHTTP_AUTOPROXY_AUTO_DETECT } else { 0 })
            | (if script.is_some() { WINHTTP_AUTOPROXY_CONFIG_URL } else { 0 }),
        dwAutoDetectFlags: if detect { WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A } else { 0 },
        lpszAutoConfigUrl: script.as_ref().map_or(null(), |s| s.as_ptr()),
        lpvReserved: null_mut(),
        dwReserved: 0,
        fAutoLogonIfChallenged: 1,
    };
    let mut info = WINHTTP_PROXY_INFO::default();
    let address = wide(url);
    // SAFETY: every pointer is live for the call; `info`'s strings are freed by `Owned`.
    let found = unsafe { WinHttpGetProxyForUrl(session, address.as_ptr(), &mut options, &mut info) } != 0;
    let (list, _bypass) = (Owned(info.lpszProxy), Owned(info.lpszProxyBypass));
    // SAFETY: the session opened above.
    unsafe { WinHttpCloseHandle(session) };
    if !found {
        return None;
    }
    Some((info.dwAccessType == WINHTTP_ACCESS_TYPE_NAMED_PROXY).then(|| list.text()).flatten())
}

/// The proxy a list names for the scheme: `https=` for TLS, `http=` otherwise, else an entry with no scheme (the first that parses).
pub(crate) fn pick(list: &str, secure: bool) -> Option<Proxy> {
    let wanted = if secure { "https" } else { "http" };
    let entries: Vec<&str> = list.split([';', ' ', '\t', '\r', '\n']).map(str::trim).filter(|e| !e.is_empty()).collect();
    let scheme_of = |entry: &str| entry.split_once('=').map(|(scheme, _)| scheme.to_ascii_lowercase());
    let chosen = entries
        .iter()
        .find(|e| scheme_of(e).as_deref() == Some(wanted))
        .map(|e| e.split_once('=').map_or(*e, |(_, v)| v))
        .or_else(|| entries.iter().find(|e| !e.contains('=')).copied())?;
    let chosen = chosen.strip_prefix("http://").unwrap_or(chosen).trim_end_matches('/');
    let (host, port) = match chosen.rsplit_once(':') {
        Some((host, port)) if !host.ends_with(':') && port.bytes().all(|b| b.is_ascii_digit()) => (host, port.parse().ok()?),
        _ => (chosen, 80),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    (!host.is_empty()).then(|| Proxy::Http { host: host.to_string(), port })
}

/// The manual bypass list (`ProxyOverride`): `<local>` for a name without dots, otherwise patterns with `*`, without regard to case.
pub(crate) fn bypassed(list: &str, host: &str) -> bool {
    list.split([';', ',', ' ', '\t']).map(str::trim).filter(|e| !e.is_empty()).any(|entry| {
        if entry.eq_ignore_ascii_case("<local>") {
            return !host.contains('.') && !host.contains(':');
        }
        let pattern = entry.to_ascii_lowercase();
        let pattern = pattern.split_once("://").map_or(pattern.as_str(), |(_, rest)| rest);
        wildcard(pattern.as_bytes(), host.as_bytes())
    })
}

fn wildcard(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|skip| wildcard(rest, &text[skip..])),
        Some((first, rest)) => text.split_first().is_some_and(|(c, tail)| c == first && wildcard(rest, tail)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_list_names_the_proxy_for_the_scheme() {
        assert_eq!(pick("proxy.corp:8080", true), Some(Proxy::Http { host: "proxy.corp".into(), port: 8080 }));
        assert_eq!(pick("http=a:1;https=b:2", true), Some(Proxy::Http { host: "b".into(), port: 2 }));
        assert_eq!(pick("http=a:1;https=b:2", false), Some(Proxy::Http { host: "a".into(), port: 1 }));
        assert_eq!(pick("socks=s:1", true), None);
        assert_eq!(pick("proxy.corp", false), Some(Proxy::Http { host: "proxy.corp".into(), port: 80 }));
        assert_eq!(pick("[::1]:3128", true), Some(Proxy::Http { host: "::1".into(), port: 3128 }));
    }

    #[test]
    fn the_bypass_list_is_read_as_windows_reads_it() {
        assert!(bypassed("<local>", "intranet") && !bypassed("<local>", "relay.example"));
        assert!(bypassed("*.example;10.*", "relay.example") && bypassed("*.example;10.*", "10.1.2.3"));
        assert!(!bypassed("*.example", "relay.example.net"));
        assert!(bypassed("RELAY.example", "relay.example"));
    }

    #[test]
    fn a_relay_url_is_asked_about_as_the_http_one_and_loopback_goes_straight() {
        assert_eq!(
            target("wss://mighty-relay.example/ws?hostToken=x").map(|t| (t.0, t.1)),
            Some((true, "mighty-relay.example".to_string()))
        );
        assert!(target("ws://127.0.0.1:8787/ws").is_some_and(|t| !t.0 && t.2 == "http://127.0.0.1:8787/"));
        let asked = target("wss://relay.example:444/ws?serverId=a&hostToken=SECRET").unwrap().2;
        assert_eq!(asked, "https://relay.example:444/", "the PAC never sees the path or the token");
        assert_eq!(resolve("ws://127.0.0.1:9/ws").unwrap(), Proxy::Direct);
        assert_eq!(resolve("wss://localhost/ws").unwrap(), Proxy::Direct);
    }
}
