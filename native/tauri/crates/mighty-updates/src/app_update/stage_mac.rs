//! Unpacking and checking a Mac package (Swift `AppUpdateService.stage` and `validate`, AppUpdate.swift:252-296; plan §1.1, fact 1, M-H).
//!
//! The same rules the 0.4.x Mac app applies, which also decide whether it accepts the Rust package: the zip unpacked by
//! `/usr/bin/ditto -x -k` (a Rust zip reader could break a framework's links and the code signature's resources; the caller runs it,
//! `unzip`), exactly one `*.app` at the top of the staging folder (`bundleCount`), then [`validate`]: no symbolic link at the bundle,
//! its `Info.plist` or its `Contents/MacOS` (`bundleSymlink`), the bundle inside the folder once links are resolved (`bundleLocation`),
//! `CFBundleIdentifier` the expected one or its successor and `CFBundleExecutable` without `/` (`bundleIdentity`), the executable no link
//! and runnable (`bundleExecutable`, asked of the platform: `may_execute`). `Info.plist` is read as `PropertyListSerialization` reads it,
//! XML or binary. Checked against Swift's own verdict by `tools/parity-mac stage` (M-I, every package).

use std::future::Future;
use std::path::{Path, PathBuf};

use super::UpdateError;

/// The app's bundle id (Swift `AppStore.appBundleIdentifier`).
pub const BUNDLE_IDENTIFIER: &str = "dev.mightycode.native";

/// The id each app id hands its updates on to (Swift `successorBundleIdentifiers`): the last version under the old id installs the
/// first under the new one.
pub const SUCCESSOR_BUNDLE_IDENTIFIERS: [(&str, &str); 1] = [("dev.mightyclaude.native", "dev.mightycode.native")];

fn key(name: &'static str) -> UpdateError {
    UpdateError::key(name)
}

/// Swift `AppUpdateService.accepts(candidate, for: expected)`: the same id, or the one `expected` hands on to.
pub fn accepts(candidate: Option<&str>, expected: &str) -> bool {
    let Some(candidate) = candidate else { return false };
    candidate == expected || SUCCESSOR_BUNDLE_IDENTIFIERS.iter().any(|(from, to)| *from == expected && *to == candidate)
}

/// The `.app` a Mac executable runs from: `<bundle>.app/Contents/MacOS/<exe>` gives `<bundle>.app`; anything else (a development build
/// in `target/debug`) none.
pub fn bundle_of(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let named = |path: &Path, name: &str| path.file_name().and_then(|n| n.to_str()) == Some(name);
    (named(macos, "MacOS") && named(contents, "Contents") && bundle.extension().and_then(|e| e.to_str()) == Some("app"))
        .then(|| bundle.to_path_buf())
}

/// The entries of `folder` whose extension is `app` (Swift's `pathExtension == "app"`, hidden ones too), in name order.
pub fn bundles(folder: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> =
        std::fs::read_dir(folder).map(|entries| entries.flatten().map(|entry| entry.path()).collect()).unwrap_or_default();
    found.retain(|path| path.extension().and_then(|e| e.to_str()) == Some("app"));
    found.sort();
    found
}

/// `resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink`: the last component is a link (a path that is not there is none).
fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// `resolvingSymlinksInPath()`: the canonical path, or the path as given when it cannot be resolved.
fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Swift `AppUpdateService.validate(app:within:expectedBundleIdentifier:)`, run at staging and again right before the install.
pub fn validate(app: &Path, within: Option<&Path>, expected: &str, may_execute: &dyn Fn(&Path) -> bool) -> Result<(), UpdateError> {
    let plist = app.join("Contents").join("Info.plist");
    let binary_directory = app.join("Contents").join("MacOS");
    if [app, plist.as_path(), binary_directory.as_path()].into_iter().any(is_link) {
        return Err(key("appUpdate.error.bundleSymlink"));
    }
    // Swift's `hasPrefix(folder + "/")`: strictly below the folder, compared by components.
    if let Some(folder) = within
        && let (app, folder) = (resolved(app), resolved(folder))
        && (app == folder || !app.starts_with(&folder))
    {
        return Err(key("appUpdate.error.bundleLocation"));
    }
    let info = std::fs::read(&plist).ok().and_then(|bytes| plist::Value::from_reader(std::io::Cursor::new(bytes)).ok());
    let info = info.as_ref().and_then(plist::Value::as_dictionary);
    let identifier = info.and_then(|info| info.get("CFBundleIdentifier")).and_then(plist::Value::as_string);
    let executable = info.and_then(|info| info.get("CFBundleExecutable")).and_then(plist::Value::as_string);
    let Some(executable) = executable.filter(|executable| accepts(identifier, expected) && !executable.contains('/')) else {
        return Err(key("appUpdate.error.bundleIdentity"));
    };
    let binary = binary_directory.join(executable);
    if is_link(&binary) || !may_execute(&binary) {
        return Err(key("appUpdate.error.bundleExecutable"));
    }
    Ok(())
}

/// Swift `AppUpdateService.stage(package:expectedBundleIdentifier:)`, into `destination` (Swift: `staged` beside the package), which is
/// deleted first and made afresh: `unzip(package, destination)` runs `/usr/bin/ditto -x -k <package> <destination>` (Err with its
/// standard error's text: `appUpdate.error.unzip`), then the one bundle is found and validated. The answer is the bundle.
pub async fn stage<U, F, E>(package: &Path, destination: &Path, expected: &str, may_execute: E, unzip: U) -> Result<PathBuf, UpdateError>
where
    U: FnOnce(PathBuf, PathBuf) -> F,
    F: Future<Output = Result<(), String>>,
    E: Fn(&Path) -> bool,
{
    let _ = std::fs::remove_dir_all(destination);
    std::fs::create_dir_all(destination).map_err(|error| UpdateError::Plain(error.to_string()))?;
    if let Err(error) = unzip(package.to_path_buf(), destination.to_path_buf()).await {
        return Err(UpdateError::Key { key: "appUpdate.error.unzip", args: vec![("error", error.trim().to_string())] });
    }
    let apps = bundles(destination);
    let [app] = apps.as_slice() else {
        return Err(UpdateError::Key { key: "appUpdate.error.bundleCount", args: vec![("count", apps.len().to_string())] });
    };
    validate(app, Some(destination), expected, &may_execute)?;
    Ok(app.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_is_three_folders_up_from_its_executable() {
        assert_eq!(
            bundle_of(Path::new("/Applications/MightyCode.app/Contents/MacOS/MightyCode")),
            Some(PathBuf::from("/Applications/MightyCode.app"))
        );
        assert_eq!(bundle_of(Path::new("/Users/me/target/debug/MightyCode")), None);
        assert_eq!(bundle_of(Path::new("/Applications/MightyCode/Contents/MacOS/MightyCode")), None);
        assert_eq!(bundle_of(Path::new("/x.app/Contents/Resources/MightyCode")), None);
    }

    /// AppUpdateTests.swift:218 "theOldIdInstallsTheRenamedAppUnderItsNewName" (its identity half): the renamed id takes over from
    /// the old one, never the other way round.
    #[test]
    fn the_old_id_hands_on_to_the_new_one_only() {
        assert!(accepts(Some("dev.mightycode.native"), "dev.mightyclaude.native"));
        assert!(accepts(Some("dev.mightycode.native"), "dev.mightycode.native"));
        assert!(!accepts(Some("dev.mightyclaude.native"), "dev.mightycode.native"));
        assert!(!accepts(Some("com.example.other"), "dev.mightyclaude.native"));
        assert!(!accepts(None, "dev.mightyclaude.native"));
    }
}
