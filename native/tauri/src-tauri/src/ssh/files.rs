//! MC-33: the files pane of an SSH workspace, read-only: a folder's names through `ssh … ls`, a file's first bytes (up to 1 MiB)
//! through `ssh … head -c`, each a short batch-mode ssh over the machine's shared connection. Saving, the editor's disk checks,
//! opening with the default app and showing in the file manager are not offered (files.rs answers them as for a read-only pane).

use mighty_core::locale::Locale;
use mighty_core::ssh::files as remote;
use mighty_files::listing::{FileEntry, ListError, Listing};
use mighty_files::preview::{self, PreviewData};
use mighty_run::ssh::SshLaunch;

use crate::host::Host;

/// The launch of `workspace_id`'s folder when it is an SSH workspace: None for any other, Err when its machine cannot be reached.
pub fn launch(host: &Host, workspace_id: &str) -> Option<Result<SshLaunch, String>> {
    let workspace = host.view().snapshot.workspace(workspace_id).filter(|w| w.is_ssh()).cloned()?;
    Some(super::launch_for(host, &workspace))
}

/// One remote folder's listing, as the files pane takes a local one.
pub async fn list(host: &Host, launch: &SshLaunch, relative: &str) -> Result<Listing, ListError> {
    let command = remote::list_command(&launch.dir, relative).ok_or(ListError::OutsideRoot)?;
    let (code, stdout) = super::exec(host, launch, command, 4 * 1024 * 1024).await.map_err(|_| ListError::Unreadable)?;
    let text = String::from_utf8_lossy(&stdout);
    if code != 0 && text.trim().is_empty() {
        return Err(ListError::NotDirectory);
    }
    let (entries, truncated) = remote::parse_listing(&text, relative);
    let entries = entries.into_iter().map(|e| FileEntry::new(&e.name, &e.relative_path, e.is_directory)).collect();
    Ok(Listing { entries: mighty_files::listing::sorted(entries), truncated })
}

/// One remote file's preview (its first bytes, marked cut past the cap).
pub async fn preview(host: &Host, launch: &SshLaunch, relative: &str, locale: &Locale) -> PreviewData {
    let failed = |key: &str| {
        let mut data = preview::from_bytes(relative, Vec::new(), false, locale);
        data.failure = Some(locale.get(key));
        data
    };
    let Some(command) = remote::read_command(&launch.dir, relative) else { return failed("files.preview.missing") };
    match super::exec(host, launch, command, remote::MAXIMUM_READ_BYTES + 1).await {
        Ok((0, bytes)) => {
            let (bytes, longer) = remote::cut(bytes, remote::MAXIMUM_READ_BYTES);
            preview::from_bytes(relative, bytes, longer, locale)
        }
        Ok(_) => failed("files.preview.missing"),
        Err(_) => failed("files.preview.failed"),
    }
}
