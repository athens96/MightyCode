//! Mac Step 0, spike D1 (.omc/research/tauri/mac-plan.md §3 0b-1, with the user's choice: libghostty's C API driven from Rust).
//!
//! `ghostty-spike --route capi|swift --out <folder>` opens a Tauri window whose page (WKWebView) is a known blue, lays an NSView over
//! it, and draws Ghostty there with the host-managed I/O backend while Rust owns the PTY (`forkpty`):
//!
//! - `capi`: Rust calls libghostty's C API directly (`ghostty_app_new`, `ghostty_surface_new` on the NSView, `receive_buffer` →
//!   the PTY, the PTY → `ghostty_surface_write_buffer`, `receive_resize` → `TIOCSWINSZ`);
//! - `swift`: the same through a Swift static library over the vendored `GhosttyTerminal` (the fallback route, built only with
//!   `MIGHTY_SPIKE_SWIFT=1`), for comparison.
//!
//! It then checks, and writes to `report.json` with the pictures: the shell's output parsed into the grid (`spike-marker-42` read
//! back), text typed through Ghostty reaching the shell and coming back (`typed-42`), a resize reaching the PTY (`stty size` equal
//! to the grid Ghostty reported), pixels of the red block on screen over the page (the window server's picture), and the same in the
//! view's own picture and the renderer's IOSurface. Exit 0 when the route works.

#[cfg(target_os = "macos")]
mod pty;
#[cfg(target_os = "macos")]
mod run;
#[cfg(target_os = "macos")]
mod sys;

fn main() {
    #[cfg(target_os = "macos")]
    run::main();
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("ghostty-spike runs on macOS only");
        std::process::exit(2);
    }
}
