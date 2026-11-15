//! M-G: the files pane's ports (plan §1.7). `FileHandles`: an open file's path from `fcntl(F_GETPATH)`, `single_link` when `fstat`
//! gives `st_nlink == 1 && S_IFREG`, `asks_handles` true. `PdfRaster`: the first page through `CGPDFDocument`, sized as on Windows.
//! `Desktop`: show in Finder (`activateFileViewerSelectingURLs`), open with the default app (`NSWorkspace openURL`, refusing an app
//! bundle or an executable file, as Windows refuses programs), a picture on the pasteboard as PNG and TIFF. Contract
//! `mighty_platform::files`. Until M-G a path is walked again after an open (C# off Windows) and the rest refuses.

use std::path::Path;

/// The files pane's ports (stubs until M-G).
pub fn file_ports() -> mighty_platform::FilePorts {
    mighty_platform::FilePorts::unsupported()
}

/// An address handed to the default handler (`NSWorkspace openURL:`; not yet on the Mac: M-G).
pub fn open_url(_url: &str) -> std::result::Result<(), String> {
    Err("opening addresses is not available on this platform yet".into())
}

/// A folder shown in Finder (`activateFileViewerSelectingURLs:`; not yet on the Mac: M-G).
pub fn show_in_explorer(_folder: &Path) -> std::result::Result<(), String> {
    Err("showing a folder is not available on this platform yet".into())
}
