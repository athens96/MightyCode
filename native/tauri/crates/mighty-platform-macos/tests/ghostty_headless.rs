//! M-D: the Ghostty bridge's ABI through a headless surface (plan M-D "Done when": no window: write, read text, free), on the main
//! thread as libghostty and AppKit want it, so with no test harness (`harness = false`). It runs where libghostty is linked (the Mac
//! with `MIGHTY_MAC_NATIVE=1`, `cfg(mighty_ghostty)`, as native-tauri.yml's Mac leg builds); elsewhere the crate is empty and this
//! binary says so.
//!
//! Beyond the round trip: one write of far more bells and titles than libghostty's mailbox holds goes in through the surface's writer
//! while the main thread ticks, and never hangs (the review's HIGH: a write made on the main thread would wait for a tick only that
//! thread could run); a program's OSC 52 read gets no reply and its write never reaches the clipboard (`clipboard-read = deny`,
//! `clipboard-write = ask` answered no); the close stops the writer and frees the surface.

fn main() {
    #[cfg(all(target_os = "macos", mighty_ghostty))]
    {
        let written = "headless-\x1b[1mmarker\x1b[0m-42\r\n한글 ok\r\n";
        let flood: String = (0..300).map(|i| format!("\x07\x1b]2;title-{i}\x07")).collect();
        let started = std::time::Instant::now();
        let report =
            mighty_platform_macos::terminal_surface::headless_round_trip(written, &flood, "typed-42").expect("the headless surface");
        assert!(report.text.contains("headless-marker-42"), "the bytes were parsed into the grid: {:?}", report.text);
        assert!(report.text.contains("한글 ok"), "UTF-8 reached the grid: {:?}", report.text);
        let input = String::from_utf8_lossy(&report.input).into_owned();
        assert!(input.contains("typed-42"), "typed text came back through receive_buffer: {report:?}");
        assert!(!input.contains("52;"), "a program's clipboard read got no reply: {input:?}");
        assert_eq!(report.clipboard_writes, 0, "a program's clipboard write was refused");
        assert!(report.grids.iter().any(|(columns, rows)| *columns > 10 && *rows > 5), "receive_resize reported the grid: {report:?}");
        assert!(report.titles > 0, "the flood went through: {report:?}");
        assert!(report.freed, "the close stopped the writer and freed the surface");
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "nothing hung");
        println!(
            "ghostty headless: grid {:?}, {} input bytes, {} bells and {} titles through, text ok, freed in {:?}",
            report.grids.last(),
            report.input.len(),
            report.bells,
            report.titles,
            started.elapsed()
        );
    }
    #[cfg(not(all(target_os = "macos", mighty_ghostty)))]
    println!("ghostty headless: libghostty is not linked into this build (the Mac with MIGHTY_MAC_NATIVE=1 runs it)");
}
