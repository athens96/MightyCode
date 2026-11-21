//! M-C: the system proxy for a URL, as URLSession finds it (plan D17; Swift's relay socket and push go through URLSession, which reads
//! the same settings). Contract `mighty_platform::proxy` (`ProxyResolver`).
//!
//! `CFNetworkCopySystemProxySettings` (System Settings › Network › Proxies, and a configuration profile's), then
//! `CFNetworkCopyProxiesForURL`, which applies the bypass list and the "exclude simple host names" switch and lists what to try in
//! order: no proxy, an HTTP or HTTPS proxy (a `CONNECT` tunnel either way), or automatic configuration (a PAC address, or a script),
//! which is run with `CFNetworkExecuteProxyAutoConfigurationURL`/`…Script` on the lookup's own thread and run loop (10 s at most; WPAD
//! is a PAC address the system found). A SOCKS proxy is passed over (neither .NET's nor this app's stack speaks it here); a PAC that
//! cannot be run is passed over too, so the next entry (usually "no proxy") decides, as URLSession goes on to it.
//!
//! The URL asked about is `http(s)://host[:port]/` only — never the path or the query, which carry the relay's host token — and it
//! is never logged nor put into an error. Loopback and `localhost` always go straight.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, TCFType, kCFAllocatorDefault};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::error::CFErrorRef;
use core_foundation::number::CFNumber;
use core_foundation::runloop::{CFRunLoop, CFRunLoopSource, CFRunLoopSourceRef, kCFRunLoopDefaultMode};
use core_foundation::string::{CFString, CFStringRef, kCFStringEncodingUTF8};
use core_foundation::url::{CFURL, CFURLCreateWithBytes, CFURLRef};
use mighty_platform::{BoxFuture, PlatformError, Proxy, ProxyResolver, Result};

/// How long an automatic configuration script may run before it counts as one that cannot be run.
const PAC_TIMEOUT: Duration = Duration::from_secs(10);

type PacCallback = extern "C" fn(client: *mut c_void, proxies: CFArrayRef, error: CFErrorRef);

#[repr(C)]
struct StreamClientContext {
    version: isize,
    info: *mut c_void,
    retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<extern "C" fn(*const c_void)>,
    copy_description: Option<extern "C" fn(*const c_void) -> CFStringRef>,
}

#[link(name = "CFNetwork", kind = "framework")]
unsafe extern "C" {
    fn CFNetworkCopySystemProxySettings() -> CFDictionaryRef;
    fn CFNetworkCopyProxiesForURL(url: CFURLRef, settings: CFDictionaryRef) -> CFArrayRef;
    fn CFNetworkExecuteProxyAutoConfigurationURL(
        script_url: CFURLRef,
        target: CFURLRef,
        callback: PacCallback,
        context: *mut StreamClientContext,
    ) -> CFRunLoopSourceRef;
    fn CFNetworkExecuteProxyAutoConfigurationScript(
        script: CFStringRef,
        target: CFURLRef,
        callback: PacCallback,
        context: *mut StreamClientContext,
    ) -> CFRunLoopSourceRef;
    static kCFProxyTypeKey: CFStringRef;
    static kCFProxyHostNameKey: CFStringRef;
    static kCFProxyPortNumberKey: CFStringRef;
    static kCFProxyAutoConfigurationURLKey: CFStringRef;
    static kCFProxyAutoConfigurationJavaScriptKey: CFStringRef;
    static kCFProxyTypeNone: CFStringRef;
    static kCFProxyTypeHTTP: CFStringRef;
    static kCFProxyTypeHTTPS: CFStringRef;
    static kCFProxyTypeAutoConfigurationURL: CFStringRef;
    static kCFProxyTypeAutoConfigurationJavaScript: CFStringRef;
}

/// Whether `url` is TLS, its host (lower case, no brackets), and what the system is asked about: `http(s)://host[:port]/` only, the
/// scheme a PAC script knows (`ws` as `http`, `wss` as `https`). The same rule as Windows' (mighty-platform-windows proxy.rs `target`).
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

/// One entry of a proxy list, as this app can use it.
#[derive(Debug, Clone, PartialEq)]
enum Entry {
    Direct,
    Http {
        host: String,
        port: u16,
    },
    /// Run this PAC address, then use what it lists.
    Script(CFURL),
    /// Run this PAC script text.
    ScriptText(CFString),
    /// Something this app cannot use (SOCKS, FTP): the next entry decides.
    Unusable,
}

/// The string a CF constant holds.
///
/// # Safety
/// `value` is one of CFNetwork's constant strings.
unsafe fn constant(value: CFStringRef) -> CFString {
    // SAFETY: the caller's constant, never released.
    unsafe { CFString::wrap_under_get_rule(value) }
}

fn entry(dictionary: &CFDictionary<CFString, CFType>) -> Entry {
    // SAFETY: CFNetwork's key and value constants.
    let (kind_key, host_key, port_key, url_key, script_key) = unsafe {
        (
            constant(kCFProxyTypeKey),
            constant(kCFProxyHostNameKey),
            constant(kCFProxyPortNumberKey),
            constant(kCFProxyAutoConfigurationURLKey),
            constant(kCFProxyAutoConfigurationJavaScriptKey),
        )
    };
    let Some(kind) = dictionary.find(&kind_key).and_then(|value| value.downcast::<CFString>()) else { return Entry::Unusable };
    // SAFETY: CFNetwork's type constants.
    let is = |value: CFStringRef| kind == unsafe { constant(value) };
    if is(unsafe { kCFProxyTypeNone }) {
        return Entry::Direct;
    }
    if is(unsafe { kCFProxyTypeHTTP }) || is(unsafe { kCFProxyTypeHTTPS }) {
        let host = dictionary.find(&host_key).and_then(|value| value.downcast::<CFString>()).map(|host| host.to_string());
        let port = dictionary.find(&port_key).and_then(|value| value.downcast::<CFNumber>()).and_then(|port| port.to_i64());
        let host = host.map(|host| host.trim().trim_start_matches('[').trim_end_matches(']').to_string()).filter(|h| !h.is_empty());
        return match (host, port.and_then(|port| u16::try_from(port).ok()).filter(|port| *port != 0)) {
            (Some(host), Some(port)) => Entry::Http { host, port },
            (Some(host), None) => Entry::Http { host, port: 80 },
            _ => Entry::Unusable,
        };
    }
    if is(unsafe { kCFProxyTypeAutoConfigurationURL }) {
        return dictionary.find(&url_key).and_then(|value| value.downcast::<CFURL>()).map_or(Entry::Unusable, Entry::Script);
    }
    if is(unsafe { kCFProxyTypeAutoConfigurationJavaScript }) {
        return dictionary.find(&script_key).and_then(|value| value.downcast::<CFString>()).map_or(Entry::Unusable, Entry::ScriptText);
    }
    Entry::Unusable
}

fn entries(list: &CFArray<CFType>) -> Vec<Entry> {
    list.iter().filter_map(|item| item.downcast::<CFDictionary>()).map(|d| entry(&untyped(d))).collect()
}

/// A dictionary of CF values keyed by strings (what CFNetwork's proxy dictionaries are).
fn untyped(dictionary: CFDictionary) -> CFDictionary<CFString, CFType> {
    // SAFETY: the same object, reinterpreted with the key and value types its documentation names; retained once more by the wrap.
    unsafe { CFDictionary::wrap_under_get_rule(dictionary.as_concrete_TypeRef()) }
}

fn url_of(text: &str) -> Option<CFURL> {
    // SAFETY: the bytes of `text` with their length; a null base.
    let url =
        unsafe { CFURLCreateWithBytes(kCFAllocatorDefault, text.as_ptr(), text.len() as isize, kCFStringEncodingUTF8, std::ptr::null()) };
    // SAFETY: a URL this call created (or null).
    (!url.is_null()).then(|| unsafe { CFURL::wrap_under_create_rule(url) })
}

struct SendCfArray(CFArray<CFType>);

// SAFETY: CFArray produced by CFNetwork PAC resolution holds immutable proxy dictionaries; synchronized under Mutex and released via
// CFRelease (which is thread-safe).
unsafe impl Send for SendCfArray {}

/// What the PAC callback hands back to the lookup that waits on its run loop. Heap-allocated via `Arc` and retained/released by
/// CFNetwork through `StreamClientContext`, eliminating any stack use-after-free if CFNetwork delivers a late callback or teardown
/// after `PAC_TIMEOUT`.
struct PacResult {
    done: AtomicBool,
    list: Mutex<Option<SendCfArray>>,
}

extern "C" fn pac_retain(info: *const c_void) -> *const c_void {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !info.is_null() {
            // SAFETY: `info` came from `Arc::as_ptr(&result)` which stays alive across context registration.
            unsafe { Arc::increment_strong_count(info.cast::<PacResult>()) };
        }
    }));
    info
}

extern "C" fn pac_release(info: *const c_void) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !info.is_null() {
            // SAFETY: balances a matching `pac_retain` call on the same `Arc<PacResult>`.
            drop(unsafe { Arc::from_raw(info.cast::<PacResult>()) });
        }
    }));
}

extern "C" fn pac_done(client: *mut c_void, proxies: CFArrayRef, error: CFErrorRef) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if client.is_null() {
            return;
        }
        // SAFETY: `client` is an `Arc<PacResult>` retained by CFNetwork via `pac_retain` until the source is completely torn down.
        let result = unsafe { &*(client as *const PacResult) };
        if error.is_null() && !proxies.is_null() {
            // SAFETY: a list CFNetwork owns for this call; retained by the wrap.
            let wrapped = unsafe { CFArray::wrap_under_get_rule(proxies) };
            *result.list.lock().unwrap_or_else(|p| p.into_inner()) = Some(SendCfArray(wrapped));
        }
        result.done.store(true, Ordering::SeqCst);
        CFRunLoop::get_current().stop();
    }));
}

/// Runs an automatic configuration (its address or its text) for `target` on this thread's run loop; None when it cannot be run.
fn run_script(script: &Entry, target: &CFURL) -> Option<Vec<Entry>> {
    let result = Arc::new(PacResult { done: AtomicBool::new(false), list: Mutex::new(None) });
    let mut context = StreamClientContext {
        version: 0,
        info: Arc::as_ptr(&result).cast_mut().cast(),
        retain: Some(pac_retain),
        release: Some(pac_release),
        copy_description: None,
    };
    // SAFETY: the script and target live across the call; CFNetwork retains `result` via `pac_retain` and releases it via
    // `pac_release`, so `result` remains valid even if CFNetwork cleans up asynchronously after timeout.
    let source = unsafe {
        match script {
            Entry::Script(url) => {
                CFNetworkExecuteProxyAutoConfigurationURL(url.as_concrete_TypeRef(), target.as_concrete_TypeRef(), pac_done, &mut context)
            }
            Entry::ScriptText(text) => CFNetworkExecuteProxyAutoConfigurationScript(
                text.as_concrete_TypeRef(),
                target.as_concrete_TypeRef(),
                pac_done,
                &mut context,
            ),
            _ => return None,
        }
    };
    if source.is_null() {
        return None;
    }
    // SAFETY: a source CFNetwork created for this lookup.
    let source = unsafe { CFRunLoopSource::wrap_under_create_rule(source) };
    let run_loop = CFRunLoop::get_current();
    // SAFETY: the default mode constant.
    let mode = unsafe { kCFRunLoopDefaultMode };
    run_loop.add_source(&source, mode);
    let deadline = std::time::Instant::now() + PAC_TIMEOUT;
    while !result.done.load(Ordering::SeqCst) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        CFRunLoop::run_in_mode(mode, left.min(Duration::from_millis(250)), true);
    }
    run_loop.remove_source(&source, mode);
    // SAFETY: invalidate the source; any in-flight CFNetwork reference still safely holds `Arc<PacResult>` via `pac_retain`/`pac_release`.
    unsafe { core_foundation::runloop::CFRunLoopSourceInvalidate(source.as_concrete_TypeRef()) };
    let guard = result.list.lock().unwrap_or_else(|p| p.into_inner());
    guard.as_ref().map(|wrapped| entries(&wrapped.0))
}

/// The first usable answer of `list`, running automatic configuration where it says so (once: a PAC's own answer never runs another).
fn decide(list: Vec<Entry>, target: &CFURL, scripts: bool) -> Option<Proxy> {
    for item in list {
        match item {
            Entry::Direct => return Some(Proxy::Direct),
            Entry::Http { host, port } => return Some(Proxy::Http { host, port }),
            Entry::Script(_) | Entry::ScriptText(_) if scripts => {
                if let Some(found) = run_script(&item, target).and_then(|answer| decide(answer, target, false)) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

fn resolve(url: &str) -> Result<Proxy> {
    let Some((_, host, asked)) = target(url) else { return Ok(Proxy::Direct) };
    if host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
        return Ok(Proxy::Direct);
    }
    let Some(target) = url_of(&asked) else { return Ok(Proxy::Direct) };
    // SAFETY: a dictionary the call creates (or null when there are no settings).
    let settings = unsafe { CFNetworkCopySystemProxySettings() };
    if settings.is_null() {
        return Ok(Proxy::Direct);
    }
    // SAFETY: created above; released by the wrap.
    let settings: CFDictionary = unsafe { CFDictionary::wrap_under_create_rule(settings) };
    // SAFETY: the URL and settings live across the call; the list it creates is released by the wrap.
    let list = unsafe { CFNetworkCopyProxiesForURL(target.as_concrete_TypeRef(), settings.as_concrete_TypeRef()) };
    if list.is_null() {
        return Ok(Proxy::Direct);
    }
    // SAFETY: created above.
    let list: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(list) };
    Ok(decide(entries(&list), &target, true).unwrap_or(Proxy::Direct))
}

/// The Mac's `ProxyResolver`.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacProxy;

impl ProxyResolver for MacProxy {
    fn resolve(&self, url: &str) -> BoxFuture<'_, Result<Proxy>> {
        let url = url.to_string();
        Box::pin(async move {
            // A PAC (or WPAD) may take seconds, and runs on a run loop: a blocking thread of its own.
            tokio::task::spawn_blocking(move || resolve(&url))
                .await
                .map_err(|_| PlatformError::Io(std::io::Error::other("the proxy lookup's thread ended")))?
        })
    }
}

/// The Mac's `ProxyResolver`.
pub fn proxy() -> Arc<dyn ProxyResolver> {
    Arc::new(MacProxy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relay_url_is_asked_about_as_the_http_one_and_loopback_goes_straight() {
        assert_eq!(
            target("wss://mighty-relay.example/ws?hostToken=x").map(|t| (t.0, t.1)),
            Some((true, "mighty-relay.example".to_string()))
        );
        assert!(target("ws://127.0.0.1:8787/ws").is_some_and(|t| !t.0 && t.2 == "http://127.0.0.1:8787/"));
        let asked = target("wss://relay.example:444/ws?serverId=a&hostToken=SECRET").unwrap().2;
        assert_eq!(asked, "https://relay.example:444/", "the system never sees the path or the token");
        assert_eq!(resolve("ws://127.0.0.1:9/ws").unwrap(), Proxy::Direct);
        assert_eq!(resolve("wss://localhost/ws").unwrap(), Proxy::Direct);
        assert_eq!(resolve("ftp://example.com/").unwrap(), Proxy::Direct, "a scheme the system is not asked about");
    }

    fn dictionary(pairs: &[(CFStringRef, CFType)]) -> CFDictionary<CFString, CFType> {
        // SAFETY: CFNetwork's key constants.
        let pairs: Vec<(CFString, CFType)> = pairs.iter().map(|(key, value)| (unsafe { constant(*key) }, value.clone())).collect();
        CFDictionary::from_CFType_pairs(&pairs)
    }

    /// The entries CFNetwork lists are read as this app can use them: none, an HTTP or HTTPS proxy with its port (80 when it names
    /// none), a PAC address or script, and anything else as unusable.
    #[test]
    fn the_system_s_proxy_entries_are_read_as_this_app_uses_them() {
        // SAFETY: CFNetwork's constants.
        unsafe {
            let kind = |value: CFStringRef| (kCFProxyTypeKey, constant(value).as_CFType());
            assert_eq!(entry(&dictionary(&[kind(kCFProxyTypeNone)])), Entry::Direct);
            let http = dictionary(&[
                kind(kCFProxyTypeHTTPS),
                (kCFProxyHostNameKey, CFString::new("proxy.corp").as_CFType()),
                (kCFProxyPortNumberKey, CFNumber::from(3128).as_CFType()),
            ]);
            assert_eq!(entry(&http), Entry::Http { host: "proxy.corp".into(), port: 3128 });
            let no_port = dictionary(&[kind(kCFProxyTypeHTTP), (kCFProxyHostNameKey, CFString::new("[::1]").as_CFType())]);
            assert_eq!(entry(&no_port), Entry::Http { host: "::1".into(), port: 80 });
            assert_eq!(entry(&dictionary(&[kind(kCFProxyTypeHTTP)])), Entry::Unusable, "a proxy with no host");
            let socks = dictionary(&[(kCFProxyTypeKey, CFString::new("kCFProxyTypeSOCKS").as_CFType())]);
            assert_eq!(entry(&socks), Entry::Unusable);
            let pac = dictionary(&[
                kind(kCFProxyTypeAutoConfigurationURL),
                (kCFProxyAutoConfigurationURLKey, url_of("http://wpad.corp/wpad.dat").unwrap().as_CFType()),
            ]);
            assert!(matches!(entry(&pac), Entry::Script(_)));
            assert_eq!(entry(&dictionary(&[])), Entry::Unusable);
        }
    }

    /// The first usable entry decides; an unusable one is passed over; a PAC script's own answer is taken (run here on this thread's
    /// run loop from its text: no network is reached), and a script that cannot be run leaves the decision to the next entry.
    #[test]
    fn the_first_usable_entry_decides_and_a_script_is_run() {
        let target = url_of("https://relay.example/").unwrap();
        let http = Entry::Http { host: "p".into(), port: 1 };
        assert_eq!(
            decide(vec![Entry::Unusable, http.clone(), Entry::Direct], &target, true),
            Some(Proxy::Http { host: "p".into(), port: 1 })
        );
        assert_eq!(decide(vec![Entry::Unusable], &target, true), None);
        let script = |text: &str| Entry::ScriptText(CFString::new(text));
        let proxied = script("function FindProxyForURL(url, host) { return \"PROXY pac.corp:8080; DIRECT\"; }");
        assert_eq!(decide(vec![proxied, Entry::Direct], &target, true), Some(Proxy::Http { host: "pac.corp".into(), port: 8080 }));
        let direct = script("function FindProxyForURL(url, host) { return \"DIRECT\"; }");
        assert_eq!(decide(vec![direct, http.clone()], &target, true), Some(Proxy::Direct));
        let broken = script("this is not a script (");
        assert_eq!(decide(vec![broken, Entry::Direct], &target, true), Some(Proxy::Direct), "a script that cannot run is passed over");
    }

    /// The live lookup answers without reaching the URL, whatever this Mac's settings are (CI's runner has none: Direct).
    #[tokio::test]
    async fn the_live_lookup_answers() {
        let answer = MacProxy.resolve("wss://relay.example.invalid/ws?hostToken=x").await.unwrap();
        assert!(matches!(answer, Proxy::Direct | Proxy::Http { .. }), "{answer:?}");
    }

    /// P1-FFI-1: `PacResult` is heap-allocated via `Arc` and `pac_retain`/`pac_release` keep its strong count balanced, so late
    /// callbacks or async teardown after timeout cannot cause a stack UAF.
    #[test]
    fn pac_context_retain_and_release_manage_arc_lifetime() {
        let result = Arc::new(PacResult { done: AtomicBool::new(false), list: Mutex::new(None) });
        let raw = Arc::as_ptr(&result).cast::<c_void>();
        assert_eq!(Arc::strong_count(&result), 1);
        let retained = pac_retain(raw);
        assert_eq!(retained, raw);
        assert_eq!(Arc::strong_count(&result), 2);
        // Simulate caller dropping its Arc while CFNetwork still holds a retained copy.
        let weak = Arc::downgrade(&result);
        drop(result);
        assert!(weak.upgrade().is_some(), "retained CFNetwork reference keeps PacResult alive");
        // Calling pac_done with null proxies/error marks done cleanly without unwinding.
        pac_done(retained.cast_mut(), std::ptr::null(), std::ptr::null_mut());
        assert!(weak.upgrade().unwrap().done.load(Ordering::SeqCst));
        pac_release(retained);
        assert!(weak.upgrade().is_none(), "pac_release drops the final reference cleanly");
    }
}
