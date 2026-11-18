//! The bundled styles (C# `StyleRegistry.cs`'s bundled manifests): `ouroboros.json` and `claude-plan.json`, the frozen Swift engine's
//! own files (`native/macos/Sources/MightyCore/Resources/Styles/`, tag `mighty-style-engine-v7`, `scripts/check-style-freeze.sh`),
//! embedded as they are and never edited. Owner: W9-D.

/// `ouroboros.json`, byte for byte the frozen file.
pub const OUROBOROS: &[u8] = include_bytes!("../../../../macos/Sources/MightyCore/Resources/Styles/ouroboros.json");

/// `claude-plan.json`, byte for byte the frozen file.
pub const CLAUDE_PLAN: &[u8] = include_bytes!("../../../../macos/Sources/MightyCore/Resources/Styles/claude-plan.json");

/// The bundled manifests by file name, in the registry's order.
pub const MANIFESTS: [(&str, &[u8]); 2] = [("ouroboros.json", OUROBOROS), ("claude-plan.json", CLAUDE_PLAN)];
