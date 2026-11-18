//! W9 Step 0: the frozen files this crate reads are where it reads them, and the bundled manifests are those files byte for byte (the
//! style freeze: embedded as they are, never a copy that could drift).

use std::path::PathBuf;

/// The repository's root, from this crate's folder (`native/tauri/crates/mighty-styles`).
fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

#[test]
fn the_bundled_manifests_are_the_frozen_files() {
    let folder = repository().join("native/macos/Sources/MightyCore/Resources/Styles");
    for (name, bytes) in mighty_styles::bundled::MANIFESTS {
        let frozen = std::fs::read(folder.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(bytes, frozen.as_slice(), "{name} is embedded as the frozen file");
        assert!(serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|value| value.is_object()), "{name} is a JSON object");
    }
}

#[test]
fn the_goldens_and_the_conformance_corpus_are_read_in_place() {
    let styles = repository().join("styles");
    let goldens = std::fs::read_dir(styles.join("golden")).expect("styles/golden").count();
    assert!(goldens > 0, "styles/golden holds the panel goldens");
    for folder in ["conformance/valid", "conformance/invalid", "fixtures"] {
        assert!(styles.join(folder).is_dir(), "styles/{folder}");
    }
    assert!(styles.join("conformance/provenance.json").is_file(), "styles/conformance/provenance.json");
}
