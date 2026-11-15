//! M-G: the setup screen's files port: owner-only folders and files (0700/0600) and the refusal of links and other users' files
//! (Swift `SetupPlanTests.swift:163` "theScriptFolderAndFilesRefuseLinksAndOtherUsers"). Contract `mighty_platform::setup`.

use std::sync::Arc;

use mighty_platform::SetupFiles;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `SetupFiles` (Unsupported until M-G).
pub fn setup_files() -> Arc<dyn SetupFiles> {
    Arc::new(Unsupported)
}
