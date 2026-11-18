//! Mac Step 0, spike D4 (.omc/research/tauri/mac-plan.md §3 0b-3): the browser pane with the engine off is a Tauri child webview
//! (WKWebView) whose website data store is its workspace's, through `WebviewBuilder::data_store_identifier` (macOS 14+).
//!
//! `webkit-spike --out <folder>` serves a page on 127.0.0.1 that sets a cookie, and opens three child webviews over the window's page:
//! A (store `a`) sets `spike=a` and then asks `/check`; B (store `b`) asks `/check`; C (store `a` again) asks `/check`. The server
//! records each request's `Cookie` header. It works when A and C send `spike=a` and B sends none (two stores share nothing; one store
//! is shared by its panes), and each child's WKWebView frame is where it was asked to be. The stores are cleared at the end.

fn main() {
    #[cfg(target_os = "macos")]
    run::main();
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("webkit-spike runs on macOS only");
        std::process::exit(2);
    }
}

#[cfg(target_os = "macos")]
mod run {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use objc2::runtime::AnyObject;
    use objc2_foundation::NSRect;
    use serde_json::{Value, json};
    use spike_common::{Answer, Seen};
    use tauri::webview::WebviewBuilder;
    use tauri::{LogicalPosition, LogicalSize, WebviewUrl};

    pub fn main() {
        let out = spike_common::out_dir("webkit-spike");
        let context = tauri::generate_context!();
        let app = tauri::Builder::default()
            .setup(move |app| {
                let window = tauri::window::WindowBuilder::new(app, "main").title("WebKit spike").inner_size(1000.0, 700.0).build()?;
                let page = WebviewBuilder::new("page", WebviewUrl::App("index.html".into()));
                window.add_child(page, LogicalPosition::new(0.0, 0.0), LogicalSize::new(1000.0, 700.0))?;
                std::thread::spawn(move || script(window, out));
                Ok(())
            })
            .build(context)
            .expect("the Tauri app");
        app.run(|_, _| {});
    }

    /// A store identifier of this run (a fresh one each run, so no store an earlier run left counts).
    fn store(tag: u8) -> [u8; 16] {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let mut id = [0u8; 16];
        id[..8].copy_from_slice(&(nanos as u64).to_le_bytes());
        id[8..12].copy_from_slice(&std::process::id().to_le_bytes());
        id[15] = tag;
        // A version-4 UUID's bits, as WebKit expects an NSUUID.
        id[6] = (id[6] & 0x0F) | 0x40;
        id[8] = (id[8] & 0x3F) | 0x80;
        id
    }

    fn wait_for(seen: &std::sync::Arc<std::sync::Mutex<Vec<Seen>>>, path: &str, limit: Duration) -> Option<Seen> {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if let Some(found) = seen.lock().unwrap().iter().find(|s| s.path == path).cloned() {
                return Some(found);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        None
    }

    /// The child's WKWebView frame in its superview (points), read on the main thread.
    fn frame(webview: &tauri::Webview) -> Value {
        let (tx, rx) = mpsc::channel();
        let asked = webview.with_webview(move |platform| {
            let view = platform.inner() as *mut AnyObject;
            // SAFETY: wry's WKWebView, alive for the call.
            let frame: NSRect = unsafe { objc2::msg_send![view, frame] };
            let _ = tx.send(json!([frame.origin.x, frame.origin.y, frame.size.width, frame.size.height]));
        });
        if let Err(error) = asked {
            return json!({"error": error.to_string()});
        }
        rx.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| json!({"error": "no answer"}))
    }

    fn script(window: tauri::Window, out: std::path::PathBuf) {
        let mut report = json!({});
        let served = spike_common::serve(|path| {
            let pane = path.split("pane=").nth(1).unwrap_or("").to_string();
            if path.starts_with("/set") {
                let mut answer = Answer::html(&format!(
                    "<html><body style='background:#ffffff'>set {pane}<script>setTimeout(() => location.href = '/check?pane={pane}', 300)</script></body></html>"
                ));
                answer.headers.push(format!("Set-Cookie: spike={pane}; Path=/; Max-Age=3600"));
                answer
            } else {
                Answer::html(&format!("<html><body style='background:#ffffff'>checked {pane}</body></html>"))
            }
        });
        let (base, seen) = match served {
            Ok(served) => served,
            Err(error) => {
                report["error"] = json!(format!("no server: {error}"));
                spike_common::write_report(&out, &report);
                std::process::exit(1);
            }
        };
        std::thread::sleep(Duration::from_millis(1500));
        let (store_a, store_b) = (store(0xA), store(0xB));
        let child = |label: &str, store: [u8; 16], url: String, x: f64| {
            let builder = WebviewBuilder::new(label, WebviewUrl::External(url.parse().unwrap())).data_store_identifier(store);
            window.add_child(builder, LogicalPosition::new(x, 80.0), LogicalSize::new(300.0, 200.0))
        };
        let a = child("pane-a", store_a, format!("{base}/set?pane=a"), 20.0);
        report["childA"] = json!(a.as_ref().map(|_| "opened").map_err(|e| e.to_string()));
        let a_check = wait_for(&seen, "/check?pane=a", Duration::from_secs(20));
        let b = child("pane-b", store_b, format!("{base}/check?pane=b"), 340.0);
        let c = child("pane-c", store_a, format!("{base}/check?pane=c"), 660.0);
        report["childB"] = json!(b.as_ref().map(|_| "opened").map_err(|e| e.to_string()));
        report["childC"] = json!(c.as_ref().map(|_| "opened").map_err(|e| e.to_string()));
        let b_check = wait_for(&seen, "/check?pane=b", Duration::from_secs(20));
        let c_check = wait_for(&seen, "/check?pane=c", Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(500));
        let frames: Vec<Value> = [&a, &b, &c].iter().map(|w| w.as_ref().map(frame).unwrap_or(Value::Null)).collect();
        let cookie = |s: &Option<Seen>| s.as_ref().and_then(|s| s.cookie.clone());
        let (ca, cb, cc) = (cookie(&a_check), cookie(&b_check), cookie(&c_check));
        let has = |c: &Option<String>, v: &str| c.as_deref().is_some_and(|c| c.split(';').any(|p| p.trim() == v));
        let isolated = a_check.is_some() && b_check.is_some() && !cb.as_deref().unwrap_or("").contains("spike=");
        let shared = has(&ca, "spike=a") && has(&cc, "spike=a");
        // Each frame (points) where it was asked: 300 × 200 (its origin is AppKit's, from the bottom).
        let sized = frames.iter().all(|f| f.as_array().is_some_and(|f| f.len() == 4 && f[2] == json!(300.0) && f[3] == json!(200.0)));
        report["requests"] = json!(seen.lock().unwrap().iter().map(|s| json!({"path": s.path, "cookie": s.cookie})).collect::<Vec<_>>());
        report["frames"] = json!(frames);
        report["verdict"] = json!({"aChecked": a_check.is_some(), "aCookie": ca, "bCookie": cb, "cCookie": cc,
            "storesIsolated": isolated, "storeSharedByItsPanes": shared, "framesSized": sized, "works": isolated && shared && sized});
        // The stores go (WKWebsiteDataStore of each identifier, emptied).
        for webview in [&a, &b].into_iter().flatten() {
            let _ = webview.clear_all_browsing_data();
        }
        let works = isolated && shared && sized;
        spike_common::write_report(&out, &report);
        std::process::exit(if works { 0 } else { 1 });
    }
}
