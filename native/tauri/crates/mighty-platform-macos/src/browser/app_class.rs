//! M-E: CEF's `CrAppProtocol` on tao's `NSApplication` subclass (`TaoApp`), added at run time before CEF initializes: the methods
//! `isHandlingSendEvent` and `setHandlingSendEvent:`, and `sendEvent:` wrapped to set the flag while the original runs (Swift
//! `MightyApplication.swift`, which subclassed `NSApplication` itself).

use mighty_platform::{PlatformError, Result};

/// Adds the methods to `NSApp`'s class (Unsupported until M-E). Call on the main thread, before the run loop.
pub fn add_cef_methods() -> Result<()> {
    Err(PlatformError::unsupported("Browser", "add_cef_methods"))
}
