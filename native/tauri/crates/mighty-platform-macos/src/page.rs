//! M-G: the page's own picture for the help capture and the smoke (`WKWebView takeSnapshotWithConfiguration:`, with a rectangle for
//! an element's box), where Windows uses WebView2's `CapturePreview` and the DevTools protocol (src-tauri `help_capture.rs` through
//! `platform.rs`). Swift `HelpCaptureDiagnostics.swift` pictured the SwiftUI window with `cacheDisplay`; here WebKit draws its own.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSView};
use objc2_foundation::{NSDictionary, NSError, NSPoint, NSRect, NSSize};
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};

/// How long WebKit's picture is waited for once asked.
const ANSWER: Duration = Duration::from_secs(30);

type Answer = Result<Vec<u8>, String>;

/// A rectangle in CSS pixels (x and y from the page's top left) as the view's own points: scaled by the page's zoom and, where the
/// view is not flipped (y up), measured from its bottom.
pub fn view_rect(clip: (f64, f64, f64, f64), zoom: f64, flipped: bool, view_height: f64) -> (f64, f64, f64, f64) {
    let (x, y, width, height) = (clip.0 * zoom, clip.1 * zoom, clip.2 * zoom, clip.3 * zoom);
    if flipped { (x, y, width, height) } else { (x, view_height - y - height, width, height) }
}

/// The snapshot as PNG at its own pixels (the backing scale's: a Retina screen's picture is twice the points).
fn png(image: &NSImage) -> Answer {
    // A pool of its own: the TIFF and the bitmap of a whole page go as soon as the PNG is copied out.
    objc2::rc::autoreleasepool(|_| {
        let tiff = image.TIFFRepresentation().ok_or("the page's picture has no pixels")?;
        let rep = NSBitmapImageRep::imageRepWithData(&tiff).ok_or("the page's picture does not read")?;
        let properties = NSDictionary::new();
        // SAFETY: an empty property dictionary of the right key type.
        let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) };
        data.map(|data| data.to_vec()).ok_or_else(|| "the page's picture does not write as PNG".into())
    })
}

/// Asks WebKit for the picture on the main thread (where `with_webview` runs); the completion handler, also on the main thread,
/// answers once through `sender`.
fn take(webview: &WKWebView, clip: Option<(f64, f64, f64, f64)>, sender: Arc<Mutex<Option<tokio::sync::oneshot::Sender<Answer>>>>) {
    let Some(main) = objc2::MainThreadMarker::new() else {
        if let Some(sender) = sender.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = sender.send(Err("the page's picture was asked off the main thread".into()));
        }
        return;
    };
    // SAFETY: on the main thread (checked above; `with_webview` runs its closure there); the configuration and block live across the
    // call (WebKit copies the block).
    unsafe {
        let configuration = WKSnapshotConfiguration::new(main);
        if let Some(clip) = clip {
            let view: &NSView = webview;
            let rect = view_rect(clip, webview.pageZoom(), view.isFlipped(), view.bounds().size.height);
            configuration.setRect(NSRect::new(NSPoint::new(rect.0, rect.1), NSSize::new(rect.2, rect.3)));
        }
        let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let answer = match (image.as_ref(), error.as_ref()) {
                (Some(image), _) => png(image),
                (None, Some(error)) => Err(format!("WebKit's snapshot: {}", error.localizedDescription())),
                (None, None) => Err("WebKit's snapshot gave nothing".into()),
            };
            if let Some(sender) = sender.lock().unwrap_or_else(|p| p.into_inner()).take() {
                let _ = sender.send(answer);
            }
        });
        webview.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &block);
    }
}

/// The page's picture as PNG bytes: the whole page, or `clip` (x, y, width, height in CSS pixels) only.
pub async fn snapshot(page: &tauri::Webview, clip: Option<(f64, f64, f64, f64)>) -> Result<Vec<u8>, String> {
    let (sender, answer) = tokio::sync::oneshot::channel::<Answer>();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let asking = sender.clone();
    page.with_webview(move |platform| {
        let pointer = platform.inner().cast::<WKWebView>();
        // SAFETY: the page's own WKWebView (wry's), alive while its webview is; retained for the call.
        match unsafe { Retained::retain(pointer) } {
            Some(webview) => take(&webview, clip, asking),
            None => {
                if let Some(sender) = asking.lock().unwrap_or_else(|p| p.into_inner()).take() {
                    let _ = sender.send(Err("the page has no WebKit view".into()));
                }
            }
        }
    })
    .map_err(|error| format!("the page's picture: {error}"))?;
    let result = tokio::time::timeout(ANSWER, answer).await.map_err(|_| "WebKit's snapshot did not answer".to_string())?;
    drop(sender);
    result.map_err(|_| "WebKit's snapshot was dropped".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box in CSS pixels becomes the view's points: scaled by the page's zoom, and from the bottom where the view's y goes up.
    #[test]
    fn a_css_box_is_the_views_rectangle() {
        assert_eq!(view_rect((10.0, 20.0, 100.0, 50.0), 1.0, true, 900.0), (10.0, 20.0, 100.0, 50.0));
        assert_eq!(view_rect((10.0, 20.0, 100.0, 50.0), 1.25, true, 900.0), (12.5, 25.0, 125.0, 62.5));
        assert_eq!(view_rect((10.0, 20.0, 100.0, 50.0), 1.0, false, 900.0), (10.0, 830.0, 100.0, 50.0));
    }
}
