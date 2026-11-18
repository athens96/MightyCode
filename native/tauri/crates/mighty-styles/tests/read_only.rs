//! The style freeze (`scripts/check-style-freeze.sh`): this crate reads the frozen files in place and never writes one. Only the trust
//! store writes, and only under the profile's `style-trust` folder; the frozen files are reached by `include_bytes!` in `bundled.rs` and
//! by the checks' reads.

use std::path::PathBuf;

#[test]
fn the_crate_never_writes_a_frozen_file() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let writes = ["fs::write", "File::create", "create_new(", "fs::rename", "remove_file", "remove_dir", "create_dir", "OpenOptions"];
    for entry in std::fs::read_dir(&source).expect("src") {
        let path = entry.expect("entry").path();
        let name = path.file_name().expect("name").to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).expect("read");
        for (number, line) in text.lines().enumerate() {
            if writes.iter().any(|w| line.contains(w)) {
                assert_eq!(name, "trust.rs", "{name}:{} writes: {line}", number + 1);
            }
            if line.contains("include_bytes!") {
                assert_eq!(name, "bundled.rs", "{name}:{} embeds a file", number + 1);
            }
        }
    }
}
