//! M-A: the installed font families for Settings › 글자. Swift `TextAppearance.swift` (the fixed-pitch families from CoreText plus
//! Swift's known monospaced list); contract `mighty_platform::fonts`.

use std::sync::Arc;

use mighty_platform::FontCatalog;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `FontCatalog` (Unsupported until M-A).
pub fn font_catalog() -> Arc<dyn FontCatalog> {
    Arc::new(Unsupported)
}
