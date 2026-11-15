//! The app page's own WebView2, for the developer modes that picture and drive it (src-tauri help_capture.rs and perf.rs, W9-H): C#
//! `CoreWebView2.CapturePreviewAsync` (the page's picture as a PNG) and `CallDevToolsProtocolMethodAsync` (the DevTools protocol, for
//! the reduced-motion emulation and a clipped `Page.captureScreenshot`). Script results come through Tauri's own `eval_with_callback`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_platform::{PlatformError, Result};
use tauri::Webview;
use webview2_com::Microsoft::Web::WebView2::Win32::{COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, ICoreWebView2};
use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, CapturePreviewCompletedHandler};
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::{IStream, STREAM_SEEK_SET};
use windows::core::HSTRING;

/// How long a picture or a protocol call is waited for once WebView2 took it.
const ANSWER: Duration = Duration::from_secs(30);

fn refused(what: &str, error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Refused(format!("{what}: {error}"))
}

/// Starts `work` on the UI thread that owns the page's WebView2; its completion handler answers through the returned sender, which
/// `answer` waits for (at most [`ANSWER`]).
async fn on_ui<T: Send + 'static>(
    webview: &Webview,
    what: &str,
    work: impl FnOnce(&ICoreWebView2, Arc<Mutex<Option<tokio::sync::oneshot::Sender<windows::core::Result<T>>>>>) -> windows::core::Result<()>
    + Send
    + 'static,
) -> Result<T> {
    let (sender, answer) = tokio::sync::oneshot::channel::<windows::core::Result<T>>();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let (started, failed) = (sender.clone(), sender.clone());
    webview
        .with_webview(move |platform| {
            // SAFETY: on the UI thread that owns the controller; the handler keeps what it needs until it runs.
            let begun = unsafe { platform.controller().CoreWebView2() }.and_then(|core| work(&core, started));
            if let Err(error) = begun
                && let Some(sender) = failed.lock().unwrap_or_else(|p| p.into_inner()).take()
            {
                let _ = sender.send(Err(error));
            }
        })
        .map_err(|error| refused(what, error))?;
    let result = tokio::time::timeout(ANSWER, answer).await.map_err(|error| refused(what, error))?.map_err(|error| refused(what, error))?;
    drop(sender);
    result.map_err(|error| refused(what, error))
}

/// C# `CapturePreviewAsync(Png, stream)`: the page's picture as WebView2 draws it, PNG bytes.
pub async fn capture_preview(webview: &Webview) -> Result<Vec<u8>> {
    on_ui(webview, "capture", |core, answer| {
        // SAFETY: on the UI thread that owns `core`; the stream lives in the completion closure until it is read.
        unsafe {
            let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
            let kept = stream.clone();
            let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
                let picture = result.and_then(|()| read_stream(&kept));
                if let Some(sender) = answer.lock().unwrap_or_else(|p| p.into_inner()).take() {
                    let _ = sender.send(picture);
                }
                Ok(())
            }));
            core.CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler)
        }
    })
    .await
}

/// C# `CallDevToolsProtocolMethodAsync(method, parameters)`: the protocol's answer as JSON text.
pub async fn devtools(webview: &Webview, method: &str, parameters: &str) -> Result<String> {
    let (method, parameters) = (HSTRING::from(method), HSTRING::from(parameters));
    on_ui(webview, "devtools", move |core, answer| {
        let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
            if let Some(sender) = answer.lock().unwrap_or_else(|p| p.into_inner()).take() {
                let _ = sender.send(result.map(|()| json));
            }
            Ok(())
        }));
        // SAFETY: on the UI thread that owns `core`.
        unsafe { core.CallDevToolsProtocolMethod(&method, &parameters, &handler) }
    })
    .await
}

/// An `IStream`'s bytes from its start.
///
/// # Safety
/// On the thread the stream may be used from.
unsafe fn read_stream(stream: &IStream) -> windows::core::Result<Vec<u8>> {
    unsafe {
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let mut bytes = Vec::new();
        let mut buffer = vec![0u8; 65_536];
        loop {
            let mut read = 0u32;
            stream.Read(buffer.as_mut_ptr().cast(), buffer.len() as u32, Some(&mut read as *mut u32)).ok()?;
            if read == 0 {
                return Ok(bytes);
            }
            bytes.extend_from_slice(&buffer[..read as usize]);
        }
    }
}
