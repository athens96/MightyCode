//! The styles a pane may pick (C# `StyleRegistry` and `RegisteredStyle` in `StyleRegistry.cs`): the bundled ones, the profile's
//! `<data>\styles\*.json` and the workspace's `.claude\mighty-styles\*.json` (at most 32 each), each with its approval, and the files
//! that were refused. Owner: W9-D.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_core::locale::Locale;
use mighty_core::workspace_files::{real_path, relative, resolve};
use sha2::{Digest, Sha256};

use crate::decoder::{self, MAXIMUM_BYTES};
use crate::evaluator::StyleEvaluator;
use crate::manifest::{StyleManifest, StyleManifestError, ordinal, safe};
use crate::trust::{StyleTrustStore, TrustAccess};

/// C# `RegisteredStyle(Manifest, Source, Path, WorkspacePath, Hash, Approval, Bytes)`.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredStyle {
    pub manifest: Arc<StyleManifest>,
    /// `bundled`, `user` or `workspace`.
    pub source: String,
    pub path: String,
    pub workspace_path: Option<String>,
    pub hash: String,
    /// `preApproved`, `approved`, `revoked` or `pending`.
    pub approval: String,
    pub bytes: Arc<[u8]>,
}

impl RegisteredStyle {
    pub fn id(&self) -> &str {
        self.manifest.id()
    }
    pub fn runnable(&self) -> bool {
        matches!(self.approval.as_str(), "preApproved" | "approved")
    }
    pub fn evaluator(&self) -> StyleEvaluator<'_> {
        StyleEvaluator::new(&self.manifest)
    }
    /// A style decoded from `bytes` with its hash (the register button's candidate, the tests' fixtures).
    pub fn decoded(
        manifest: StyleManifest,
        source: &str,
        path: &str,
        workspace_path: Option<String>,
        approval: &str,
        bytes: &[u8],
    ) -> Self {
        RegisteredStyle {
            manifest: Arc::new(manifest),
            source: source.to_string(),
            path: path.to_string(),
            workspace_path,
            hash: hash(bytes),
            approval: approval.to_string(),
            bytes: Arc::from(bytes),
        }
    }
}

/// `Convert.ToHexStringLower(SHA256.HashData(bytes))`.
pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// C# `StyleRejection(Path, Code, Message)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleRejection {
    pub path: String,
    pub code: String,
    pub message: String,
}

/// C# `StyleRegistry`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StyleRegistry {
    pub styles: Vec<RegisteredStyle>,
    pub rejections: Vec<StyleRejection>,
    pub trust_locked: bool,
}

/// The bundled resource name C# lists a bundled style under (its `Path`).
fn bundled_path(file: &str) -> String {
    format!("MightyCode.Core.Styles.{file}")
}

impl StyleRegistry {
    /// `Runnable(id, hash)`: the runnable style of that id — a bundled one whatever the hash, any other with exactly these bytes.
    pub fn runnable(&self, id: Option<&str>, hash: Option<&str>) -> Option<&RegisteredStyle> {
        self.styles.iter().find(|s| Some(s.id()) == id && s.runnable() && (s.source == "bundled" || Some(s.hash.as_str()) == hash))
    }

    /// `StyleRegistry.Load(profile, workspace)`.
    pub fn load(profile: &Path, workspace: &Path, access: Arc<dyn TrustAccess>, locale: Locale) -> StyleRegistry {
        let trust = StyleTrustStore::new(&profile.join("style-trust"), access, locale);
        let mut registry = StyleRegistry::default();
        if let Err(e) = trust.load() {
            registry.trust_locked = true;
            registry.rejections.push(StyleRejection {
                path: trust.file_path().to_string_lossy().into_owned(),
                code: "E_TRUST_LOCKED".into(),
                message: e.0,
            });
        }
        let mut bundled: Vec<(&str, &[u8])> = crate::bundled::MANIFESTS.to_vec();
        bundled.sort_by(|a, b| ordinal(&bundled_path(a.0), &bundled_path(b.0)));
        for (file, bytes) in bundled {
            registry.add(&trust, bytes, "bundled", &bundled_path(file), None);
        }
        registry.scan(&trust, profile, "styles", "user", workspace);
        registry.scan(&trust, workspace, ".claude/mighty-styles", "workspace", workspace);
        registry
    }

    fn add(&mut self, trust: &StyleTrustStore, bytes: &[u8], source: &str, path: &str, root: Option<String>) {
        let manifest = match decoder::decode(bytes, source, None) {
            Ok(m) => m,
            Err(e) => return self.reject(path, e),
        };
        if self.styles.iter().any(|s| s.id() == manifest.id()) {
            let id = manifest.id().to_string();
            return self.reject(path, StyleManifestError::new("E_ID_COLLISION", &id));
        }
        let pending = if source == "bundled" { "preApproved" } else { "pending" };
        let mut style = RegisteredStyle::decoded(manifest, source, path, root, pending, bytes);
        if !self.trust_locked {
            match trust.state(&style) {
                Ok(state) => style.approval = state.to_string(),
                Err(e) => {
                    self.rejections.push(StyleRejection { path: path.to_string(), code: "E_TRUST_LOCKED".into(), message: e.0 });
                    return;
                }
            }
        }
        self.styles.push(style);
    }

    fn reject(&mut self, path: &str, e: StyleManifestError) {
        self.rejections.push(StyleRejection { path: path.to_string(), code: e.code, message: e.message });
    }

    fn scan(&mut self, trust: &StyleTrustStore, root: &Path, folder: &str, source: &str, workspace: &Path) {
        let root = real_path(root).unwrap_or_else(|| root.to_path_buf());
        let Some(folder) = resolve(folder, &root).filter(|f| f.is_dir()) else { return };
        let listing = match std::fs::read_dir(&folder) {
            Ok(listing) => listing,
            Err(e) => {
                return self.rejections.push(StyleRejection {
                    path: folder.to_string_lossy().into_owned(),
                    code: "E_READ".into(),
                    message: safe(&e.to_string(), 64),
                });
            }
        };
        let mut paths: Vec<PathBuf> =
            listing.filter_map(|e| e.ok()).filter(|e| e.file_type().is_ok_and(|t| !t.is_dir())).take(1024).map(|e| e.path()).collect();
        paths.sort_by(|a, b| ordinal(&a.to_string_lossy(), &b.to_string_lossy()));
        for path in paths.into_iter().filter(|p| p.to_string_lossy().ends_with(".json")).take(32) {
            let rel =
                relative(&path, &root).unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            if let Some(bytes) = crate::files::read(&root, &rel, MAXIMUM_BYTES + 1) {
                let real = real_path(&path).unwrap_or(path.clone()).to_string_lossy().into_owned();
                let place = (source == "workspace")
                    .then(|| real_path(workspace).unwrap_or_else(|| workspace.to_path_buf()).to_string_lossy().into_owned());
                self.add(trust, &bytes, source, &real, place);
            }
        }
    }

    /// `Unchanged(style)`: the exact place read again right before an approval or a use, its bytes still the approved ones.
    pub fn unchanged(style: &RegisteredStyle) -> bool {
        if style.source == "bundled" {
            return true;
        }
        let path = Path::new(&style.path);
        let root = match &style.workspace_path {
            Some(w) => PathBuf::from(w),
            None => path.parent().map(Path::to_path_buf).unwrap_or_default(),
        };
        let rel = relative(path, &root).unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        crate::files::read(&root, &rel, MAXIMUM_BYTES).is_some_and(|bytes| hash(&bytes) == style.hash)
    }
}

/// The Mighty views read a style through mighty-graph's `StructureStyle` (C# `StyleEvaluator` and its manifest).
impl mighty_graph::structure::StructureStyle for RegisteredStyle {
    fn id(&self) -> &str {
        self.manifest.id()
    }
    fn phases(&self) -> &[crate::manifest::StylePhase] {
        &self.manifest.phases
    }
    fn plan_state(&self) -> bool {
        crate::manifest::text(self.manifest.rule("phase"), "kind") == Some("planState")
    }
    fn current_phase(&self, plan_stage: Option<&str>) -> Option<&crate::manifest::StylePhase> {
        StyleEvaluator::new(&self.manifest).current_phase::<&str>(&[], None, plan_stage)
    }
    fn prompt_phase(&self, prompt: &str) -> Option<String> {
        let evaluator = StyleEvaluator::new(&self.manifest);
        match evaluator.recognised_action(prompt) {
            Some(action) => action.phase.clone(),
            None => {
                let name = crate::evaluator::recognised_name(&self.manifest, prompt);
                crate::manifest::items(&self.manifest.root, "aliases")
                    .iter()
                    .find(|a| crate::manifest::text(a, "name").map(str::to_string) == name)
                    .and_then(|a| crate::manifest::text(a, "phase"))
                    .map(str::to_string)
            }
        }
    }
}
