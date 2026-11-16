//! M-E: a browser pane as a WKWebView child webview of the main window (Tauri's `add_child`, the same path as Windows' WebView2
//! panes), laid over the page's slot. Its website data store is its workspace's (`data_store_identifier`, macOS 14+: a version-5 UUID
//! of the profile folder, [`super::store_identifier`]), so a workspace's panes share sign-ins and two workspaces never do (Step 0's
//! webkit spike). As C#'s pane sets its engine up: new windows are refused (`window.open`, with or without a click → none; the host
//! hears [`BrowserEvent::PopupRefused`]), downloads cancelled, no script dialog shows (WKWebView shows none without a UI delegate's
//! answer), each finished navigation tells the host where the view went, back and forward are WebKit's own, a press in the page
//! reports the view focused, and a pane never loads the app's own addresses. The picture a page overlay shows in the view's place is
//! `takeSnapshotWithConfiguration:` (PNG). There is no runtime to install (the pane is `ready` at once).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
use mighty_platform::{
    BoxFuture, BrowserEvent, BrowserOptions, BrowserSurface, DeviceRect, NativeSurface, PlatformError, Result, SurfaceState,
};
use objc2::MainThreadMarker;
use objc2_app_kit::NSImage;
use objc2_foundation::{NSDate, NSError, NSUUID};
use objc2_web_kit::{WKWebView, WKWebsiteDataStore};
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{Manager as _, PhysicalPosition, PhysicalSize, Rect, Url, Webview, WebviewUrl};

use super::{BLANK, refused};

/// Makes each webview's label unique, also for a pane that opens again before its old view is gone.
static NEXT: AtomicU64 = AtomicU64::new(1);
/// This run's stores are emptied when it ends (the smoke).
static TEMPORARY: AtomicBool = AtomicBool::new(false);
/// The stores a temporary run opened.
static STORES: Mutex<Vec<[u8; 16]>> = Mutex::new(Vec::new());

pub(super) fn set_temporary(temporary: bool) {
    TEMPORARY.store(temporary, Ordering::SeqCst);
}

pub async fn open(window: &tauri::Window, options: BrowserOptions) -> Result<Box<dyn BrowserSurface>> {
    if !supported() {
        return Err(PlatformError::unsupported("SurfaceFactory", "open_browser (WebKit's data stores need macOS 14)"));
    }
    let url = super::first_address(window, &options.url)?;
    let label = super::label(&options.pane_id, NEXT.fetch_add(1, Ordering::Relaxed));
    let store = super::store_identifier(&options.data_dir);
    if TEMPORARY.load(Ordering::SeqCst) {
        let mut stores = STORES.lock().unwrap_or_else(|p| p.into_inner());
        if !stores.contains(&store) {
            stores.push(store);
        }
    }
    let BrowserOptions { bounds, listener, .. } = options;
    let (window, dev_server, events) = (window.clone(), super::dev_server(window), listener.clone());
    // add_child waits for the main thread: never on the main thread itself, never on an async worker.
    let webview = tauri::async_runtime::spawn_blocking(move || {
        let (popups, downloads, loads) = (events.clone(), events.clone(), events);
        // Its workspace's store, files dropped on it the page's own (as Windows' panes), never the app's own addresses; a new window
        // or a download is refused and the host hears of it; each finished navigation is reported with the address WebKit shows.
        let builder = WebviewBuilder::new(label, WebviewUrl::External(Url::parse(BLANK).expect("about:blank")))
            .data_store_identifier(store)
            .disable_drag_drop_handler()
            .focused(false)
            .on_navigation(move |url| !super::app_address(url, dev_server.as_ref()))
            .on_new_window(move |_, _| {
                if let Some(listener) = &popups {
                    listener.send(BrowserEvent::PopupRefused);
                }
                NewWindowResponse::Deny
            })
            .on_download(move |_, event| {
                if matches!(event, DownloadEvent::Requested { .. })
                    && let Some(listener) = &downloads
                {
                    listener.send(BrowserEvent::DownloadRefused);
                }
                false
            })
            .on_page_load(move |_, payload| {
                if payload.event() == PageLoadEvent::Finished
                    && let Some(listener) = &loads
                {
                    listener.send(BrowserEvent::Navigated { url: Some(payload.url().to_string()), success: true });
                }
            });
        window.add_child(builder, PhysicalPosition::new(bounds.x, bounds.y), PhysicalSize::new(bounds.width.max(1), bounds.height.max(1)))
    })
    .await
    .map_err(|error| refused("the browser view", error))?
    .map_err(|error| refused("the browser view", error))?;
    // From here a failure closes the view it made: no shown view is left without its pane's surface.
    let started = async {
        let view = on_view(&webview, "the browser view", move |view, mtm| {
            // SAFETY: the live view's configuration (its preferences and its user content controller are the view's own, shared by the
            // configuration's copy), on the main thread, before its first real address.
            unsafe {
                let configuration = view.configuration();
                // Nothing of the app's in a page from the web (review M-E 1): Tauri's scripts (its IPC and invoke key) and its message
                // handlers, which every Tauri webview gets, go before the first address loads.
                let content = configuration.userContentController();
                content.removeAllUserScripts();
                content.removeAllScriptMessageHandlers();
                // A page's `window.open` reaches the refusal (and the host's count) even without a click, as WebView2's
                // NewWindowRequested does; WebKit's own blocker would drop it unseen.
                configuration.preferences().setJavaScriptCanOpenWindowsAutomatically(true);
            }
            if let Some(listener) = listener {
                super::watch_focus(view, listener, mtm);
            }
            view as *const WKWebView as usize
        })
        .await?;
        if url.as_str() != BLANK {
            webview.navigate(url).map_err(|error| refused("navigate", error))?;
        }
        Ok(view)
    };
    match started.await {
        Ok(view) => Ok(Box::new(WebKitBrowser { webview, view }) as Box<dyn BrowserSurface>),
        Err(error) => {
            // Its presses forgotten before it goes (the monitor would otherwise ask a view that is gone).
            let _ = webview.with_webview(|platform| {
                super::forget_focus(platform.inner() as usize);
            });
            let _ = webview.close();
            Err(error)
        }
    }
}

/// Whether WebKit gives each workspace its own data store here (macOS 14's `dataStoreForIdentifier:`; below it wry would share the
/// default store between workspaces).
pub fn supported() -> bool {
    objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion().majorVersion >= 14
}

thread_local! {
    /// A closing view's navigation delegate and focus listeners, kept until its close is known to have happened (main thread only).
    static CLOSING: std::cell::RefCell<Vec<Closing>> = const { std::cell::RefCell::new(Vec::new()) };
}

struct Closing {
    view: usize,
    delegate: Option<objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_web_kit::WKNavigationDelegate>>>,
    listeners: Vec<mighty_platform::BrowserListener>,
}

/// Runs `work` with the view's WKWebView on the main thread and waits for its answer.
async fn on_view<T: Send + 'static>(
    webview: &Webview,
    what: &str,
    work: impl FnOnce(&WKWebView, MainThreadMarker) -> T + Send + 'static,
) -> Result<T> {
    let (sender, answer) = tokio::sync::oneshot::channel();
    webview
        .with_webview(move |platform| {
            let Some(mtm) = MainThreadMarker::new() else { return };
            // SAFETY: wry's WKWebView, alive for the call, on the main thread.
            let view = unsafe { &*(platform.inner() as *const WKWebView) };
            let _ = sender.send(work(view, mtm));
        })
        .map_err(|error| refused(what, error))?;
    answer.await.map_err(|error| refused(what, error))
}

/// Empties the stores a temporary run opened (WebKit's own data of every type), then removes them (macOS 14's
/// `removeDataStoreForIdentifier:`, which refuses a store a view still uses), waiting up to 3 s for WebKit each time. On the main
/// thread, as the app ends; what became of them, for the log.
pub(super) fn clear_temporary_stores() -> Option<String> {
    let stores = std::mem::take(&mut *STORES.lock().unwrap_or_else(|p| p.into_inner()));
    let mtm = MainThreadMarker::new()?;
    if stores.is_empty() {
        return None;
    }
    let count = stores.len();
    let left = Arc::new(AtomicUsize::new(stores.len()));
    let removed = Arc::new(AtomicUsize::new(0));
    let wait = |left: &AtomicUsize| {
        let deadline = Instant::now() + Duration::from_secs(3);
        while left.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            // SAFETY: CoreFoundation's own mode constant.
            CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, Duration::from_millis(10), true);
        }
    };
    // SAFETY: WebKit's class methods on the main thread; each completion block only counts down.
    unsafe {
        let types = WKWebsiteDataStore::allWebsiteDataTypes(mtm);
        let since = NSDate::distantPast();
        for id in &stores {
            let store = WKWebsiteDataStore::dataStoreForIdentifier(&NSUUID::from_bytes(*id), mtm);
            let left = left.clone();
            let done = RcBlock::new(move || {
                left.fetch_sub(1, Ordering::SeqCst);
            });
            store.removeDataOfTypes_modifiedSince_completionHandler(&types, &since, &done);
        }
    }
    wait(&left);
    let emptied = count - left.load(Ordering::SeqCst);
    left.store(count, Ordering::SeqCst);
    for id in &stores {
        let (left, removed) = (left.clone(), removed.clone());
        let done = RcBlock::new(move |error: *mut NSError| {
            if error.is_null() {
                removed.fetch_add(1, Ordering::SeqCst);
            }
            left.fetch_sub(1, Ordering::SeqCst);
        });
        // SAFETY: WebKit's class method on the main thread; the block only counts.
        unsafe { WKWebsiteDataStore::removeDataStoreForIdentifier_completionHandler(&NSUUID::from_bytes(*id), &done, mtm) };
    }
    wait(&left);
    Some(format!(
        "WebKit stores of this run: {emptied} of {count} emptied, {} removed (a store a view still holds stays, empty)",
        removed.load(Ordering::SeqCst)
    ))
}

struct WebKitBrowser {
    webview: Webview,
    /// Its WKWebView's address (the focus monitor's and the close's key).
    view: usize,
}

impl NativeSurface for WebKitBrowser {
    /// The slot in device pixels as Tauri's physical pixels (its child webviews divide by the window's backing scale).
    fn place(&self, rect: DeviceRect) -> Result<()> {
        let bounds =
            Rect { position: PhysicalPosition::new(rect.x, rect.y).into(), size: PhysicalSize::new(rect.width, rect.height).into() };
        self.webview.set_bounds(bounds).map_err(|error| refused("place", error))
    }

    fn set_visible(&self, visible: bool) -> Result<()> {
        if visible { self.webview.show() } else { self.webview.hide() }.map_err(|error| refused("show", error))
    }

    fn focus(&self) -> Result<()> {
        self.webview.set_focus().map_err(|error| refused("focus", error))
    }

    /// WebKit's `takeSnapshotWithConfiguration:` of what the view shows, as a PNG `data:` URL.
    fn snapshot(&self) -> BoxFuture<'_, Result<Option<String>>> {
        Box::pin(async move {
            let (sender, answer) = tokio::sync::oneshot::channel::<Option<Vec<u8>>>();
            let sender = Arc::new(Mutex::new(Some(sender)));
            on_view(&self.webview, "snapshot", move |view, _| {
                let handler = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
                    // SAFETY: WebKit's image (or null), alive for the call.
                    let png = unsafe { image.as_ref() }.and_then(super::png_of_image);
                    if let Some(sender) = sender.lock().unwrap_or_else(|p| p.into_inner()).take() {
                        let _ = sender.send(png);
                    }
                });
                // SAFETY: the whole view at its own size (no configuration), on the main thread.
                unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &handler) };
            })
            .await?;
            let png = tokio::time::timeout(Duration::from_secs(2), answer)
                .await
                .map_err(|error| refused("snapshot", error))?
                .map_err(|error| refused("snapshot", error))?;
            Ok(png.filter(|png| !png.is_empty()).map(|png| super::data_url(&png)))
        })
    }

    /// What AppKit says, not what was last asked: the WKWebView's frame in the window's content view, in device pixels, and the
    /// address it shows (read here: wry's own getter unwraps a view that has none yet).
    fn inspect(&self) -> BoxFuture<'_, Result<SurfaceState>> {
        Box::pin(async move {
            let ((bounds, visible), url) = on_view(&self.webview, "inspect", |view, _| {
                // SAFETY: the live view's address, on the main thread.
                let url = unsafe { view.URL() }.and_then(|url| url.absoluteString()).map(|text| text.to_string());
                (super::device_state(view), url)
            })
            .await?;
            Ok(SurfaceState { bounds, visible, url })
        })
    }

    fn close(&self) -> Result<()> {
        // On the main thread before the close reaches it (both go through its queue, in order): its presses forgotten, and its load
        // stopped with no navigation callback left (wry's page-load handler unwraps the address, which a view torn down mid-load has
        // none of). Both are kept until the close is known: a close that fails gives them back.
        let view = self.view;
        let _ = self.webview.with_webview(move |platform| {
            // SAFETY: wry's WKWebView on the main thread; its navigation delegate is a weak property (wry keeps its own reference).
            unsafe {
                let web_view = &*(platform.inner() as *const WKWebView);
                let delegate = web_view.navigationDelegate();
                web_view.stopLoading();
                web_view.setNavigationDelegate(None);
                let listeners = super::forget_focus(view);
                CLOSING.with(|closing| closing.borrow_mut().push(Closing { view, delegate, listeners }));
            }
        });
        let take = move || {
            CLOSING.with(|closing| {
                let mut closing = closing.borrow_mut();
                closing.iter().position(|entry| entry.view == view).map(|at| closing.remove(at))
            })
        };
        match self.webview.close() {
            Ok(()) => {
                // Let go of what was kept, once the close went through.
                let _ = self.webview.window().run_on_main_thread(move || drop(take()));
                Ok(())
            }
            Err(error) => {
                let _ = self.webview.with_webview(move |platform| {
                    if let Some(entry) = take() {
                        // SAFETY: wry's WKWebView, still open, on the main thread: its own delegate given back.
                        unsafe { (*(platform.inner() as *const WKWebView)).setNavigationDelegate(entry.delegate.as_deref()) };
                        super::rewatch_focus(view, entry.listeners);
                    }
                });
                Err(refused("close", error))
            }
        }
    }
}

impl BrowserSurface for WebKitBrowser {
    fn navigate(&self, url: &str) -> Result<()> {
        let url = Url::parse(url).map_err(|error| PlatformError::Invalid(format!("address: {error}")))?;
        // The navigation handler refuses the app's own addresses too; this says so to the caller.
        let dev_server = if tauri::is_dev() { self.webview.config().build.dev_url.clone() } else { None };
        if super::app_address(&url, dev_server.as_ref()) {
            return Err(PlatformError::Invalid("address: the app's own".into()));
        }
        self.webview.navigate(url).map_err(|error| refused("navigate", error))
    }

    fn go_back(&self) -> Result<()> {
        self.webview
            .with_webview(|platform| {
                // SAFETY: wry's WKWebView on the main thread.
                let _ = unsafe { (*(platform.inner() as *const WKWebView)).goBack() };
            })
            .map_err(|error| refused("back", error))
    }

    fn go_forward(&self) -> Result<()> {
        self.webview
            .with_webview(|platform| {
                // SAFETY: wry's WKWebView on the main thread.
                let _ = unsafe { (*(platform.inner() as *const WKWebView)).goForward() };
            })
            .map_err(|error| refused("forward", error))
    }

    fn reload(&self) -> Result<()> {
        self.webview.reload().map_err(|error| refused("reload", error))
    }
}
