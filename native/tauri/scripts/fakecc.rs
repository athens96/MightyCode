// A stand-in C compiler and archiver for mac-check.ps1: `cargo check`/`clippy --target aarch64-apple-darwin` on Windows runs build
// scripts that compile C or Objective-C (objc2-exception-helper, bundled SQLite) for a target this PC has no SDK for. A check links
// nothing, so an empty object and an empty archive are enough. Never used for a build that links.
use std::io::Write;

fn main() {
    let tool = std::env::args().next().unwrap_or_default().to_lowercase();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if tool.contains("fakear") {
        // ar <flags> <archive> <objects…>
        if let Some(archive) = args.iter().skip(1).find(|a| a.ends_with(".a") || a.ends_with(".lib")) {
            let _ = std::fs::write(archive, b"!<arch>\n");
        }
        return;
    }
    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("clang version 17.0.0 (mac-check stand-in)");
        return;
    }
    if args.iter().any(|a| a == "-E") {
        let _ = std::io::stdout().write_all(b"#define __clang__ 1\n");
        return;
    }
    let mut out = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "-o" {
            out = it.next().cloned();
        } else if let Some(rest) = arg.strip_prefix("-o").filter(|rest| !rest.is_empty()) {
            out = Some(rest.to_string());
        }
    }
    if let Some(out) = out {
        let _ = std::fs::write(out, b"");
    }
}
