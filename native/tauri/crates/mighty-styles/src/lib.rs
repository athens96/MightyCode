//! Mighty Code's style engine (W9-D, `.omc/research/tauri/w9-plan.md` §1.7; docs/mighty-styles.md): a port of C#'s
//! `MightyCode.Core\Styles\*` (1,234 lines; itself a port), checked against the frozen Swift engine's files read in place — the bundled
//! manifests, the goldens `styles/golden`, the conformance corpus `styles/conformance` and the fixtures `styles/fixtures` — which this
//! crate only reads, never edits or regenerates (`scripts/check-style-freeze.sh`; a grep check). No Tauri, no `cfg(target_os)` (the
//! watcher's folder polling is plain std::fs). The app's half is src-tauri's `styles.rs`, `settings/styles.rs`, the dialogs D11–D13 and
//! the phone's style parts.
//!
//! | C# | here | owner |
//! |---|---|---|
//! | `StyleManifest.cs` (records) | [`manifest`] | D |
//! | `StyleManifest.cs` (decoder, validator) | [`decoder`], [`validator`] | D |
//! | `StyleRegistry.cs` (registry, bundled, approvals) | [`registry`], [`bundled`], [`trust`] | D |
//! | `StyleEvaluator.cs` | [`evaluator`] | D |
//! | `StyleStateEngine.cs` (`StyleFiles`) | [`state`], [`files`] | D |
//! | `StylePanelProjection.cs` | [`projection`] | D |
//! | `StylePresentation.cs`, `StyleEmoji.cs` | [`presentation`] | D |
//! | `StylePrerequisites.cs` | [`prerequisites`] | D |
//! | `StyleRunPermissions.cs` | [`permissions`] | D |
//! | `StyleChangeWatcher.cs` | [`watcher`] | D |
//!
//! Every check reads the frozen files where they are (`tests/`); the panel bytes are compared whole with the goldens.

pub mod bundled;
pub mod decoder;
pub mod evaluator;
pub mod files;
pub mod manifest;
pub mod permissions;
pub mod prerequisites;
pub mod presentation;
pub mod projection;
pub mod registry;
pub mod state;
pub mod trust;
pub mod validator;
pub mod watcher;

pub use evaluator::{StyleEvaluator, StyleFileState, StyleStateReading, StyleStateWidget, StyleTaskItem};
pub use manifest::{StyleAction, StyleGroup, StyleManifest, StyleManifestError, StylePhase};
pub use prerequisites::StylePrerequisiteResult;
pub use projection::StylePanel;
pub use registry::{RegisteredStyle, StyleRegistry, StyleRejection};
pub use trust::{PlainAccess, StyleTrustStore, TrustAccess};
