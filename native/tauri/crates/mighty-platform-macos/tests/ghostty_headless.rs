//! M-D: the Ghostty bridge's ABI through a headless surface (plan M-D "Done when": no window: write, read text, free), on the main
//! thread as libghostty and AppKit want it, so with no test harness (`harness = false`). It runs where libghostty is linked (the Mac
//! with `MIGHTY_MAC_NATIVE=1`, `cfg(mighty_ghostty)`, as native-tauri.yml's Mac leg builds); elsewhere the crate is empty and this
//! binary says so.

fn main() {
    #[cfg(all(target_os = "macos", mighty_ghostty))]
    {
        let written = "headless-\x1b[1mmarker\x1b[0m-42\r\n한글 ok\r\n";
        let report = mighty_platform_macos::terminal_surface::headless_round_trip(written, "typed-42").expect("the headless surface");
        assert!(report.text.contains("headless-marker-42"), "the bytes were parsed into the grid: {:?}", report.text);
        assert!(report.text.contains("한글 ok"), "UTF-8 reached the grid: {:?}", report.text);
        assert!(String::from_utf8_lossy(&report.input).contains("typed-42"), "typed text came back through receive_buffer: {report:?}");
        assert!(report.grids.iter().any(|(columns, rows)| *columns > 10 && *rows > 5), "receive_resize reported the grid: {report:?}");
        println!("ghostty headless: grid {:?}, {} input bytes, text ok", report.grids.last(), report.input.len());
    }
    #[cfg(not(all(target_os = "macos", mighty_ghostty)))]
    println!("ghostty headless: libghostty is not linked into this build (the Mac with MIGHTY_MAC_NATIVE=1 runs it)");
}
