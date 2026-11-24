//! C# `WorkMapService` (WorkMapSummary.cs:193-260): what the work map needs from a running app — the model of a workspace, and the one
//! summary a workspace may be making at a time. The page and (later) the phone both come here, so they see the same `summarizing` and
//! the same errors.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, Workspace};
use mighty_run::model::CliCommand;

use crate::originals::FileOriginals;
use crate::summary::{self, Runner, SummaryError, WorkMapSummaryStore};
use crate::{Git, LocalRepository, Repository, WorkMapModel, WorkMapVersion};

/// A local workspace's folder (owned, so the map reads it the same way it reads an SSH workspace's).
struct Here {
    git: Git,
    root: PathBuf,
}

impl Repository for Here {
    fn run<'a>(&'a self, arguments: &'a [&'a str], timeout: f64) -> mighty_platform::BoxFuture<'a, Option<crate::git::GitResult>> {
        Box::pin(async move { LocalRepository { git: &self.git, root: &self.root }.run(arguments, timeout).await })
    }

    fn read_first<'a>(&'a self, relative: &'a str, limit: usize) -> mighty_platform::BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move { LocalRepository { git: &self.git, root: &self.root }.read_first(relative, limit).await })
    }
}

/// `WorkMapStart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkMapStart {
    Started,
    Busy,
    UnknownKey,
    Unavailable,
}

pub type Finder = Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<CliCommand>> + Send>> + Send + Sync>;
pub type Changed = Arc<dyn Fn(&str) + Send + Sync>;
/// MC-47: an SSH workspace's folder on its machine, as the work map reads it; Err when the machine cannot be reached.
pub type SshRepositories =
    Arc<dyn Fn(&Workspace) -> mighty_platform::BoxFuture<'static, Result<Arc<dyn Repository>, String>> + Send + Sync>;

#[derive(Default)]
struct Busy {
    summarizing: HashMap<String, String>,
    errors: HashMap<String, HashMap<String, String>>,
}

pub struct WorkMapService {
    store: WorkMapSummaryStore,
    git: Git,
    snapshot: Arc<dyn Fn() -> AppSnapshot + Send + Sync>,
    originals: FileOriginals,
    find_claude: Finder,
    runner: Runner,
    locale: Locale,
    busy: Mutex<Busy>,
    changed: Mutex<Option<Changed>>,
    ssh: Mutex<Option<SshRepositories>>,
}

impl WorkMapService {
    /// `store`: the folder the summaries are kept in (`<data>/work-map`); `originals`: the profile's `file-originals`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: PathBuf,
        git: Git,
        snapshot: Arc<dyn Fn() -> AppSnapshot + Send + Sync>,
        originals: PathBuf,
        find_claude: Finder,
        runner: Runner,
        locale: Locale,
    ) -> Arc<WorkMapService> {
        Arc::new(WorkMapService {
            store: WorkMapSummaryStore::new(store),
            git,
            snapshot,
            originals: FileOriginals::new(originals),
            find_claude,
            runner,
            locale,
            busy: Mutex::new(Busy::default()),
            changed: Mutex::new(None),
            ssh: Mutex::new(None),
        })
    }

    /// `Changed`: a workspace's summary ended (done or failed); the host pushes the model again.
    pub fn on_changed(&self, changed: Changed) {
        *self.changed.lock().unwrap_or_else(|p| p.into_inner()) = Some(changed);
    }

    /// MC-47: how an SSH workspace's folder is reached (the app's machines); without it an SSH workspace has no map.
    pub fn on_ssh(&self, repositories: SshRepositories) {
        *self.ssh.lock().unwrap_or_else(|p| p.into_inner()) = Some(repositories);
    }

    /// The repository `workspace`'s map reads: its folder here, or (MC-47) its folder on its SSH machine. Err with the map's reason.
    async fn repository(&self, workspace: &Workspace) -> Result<Arc<dyn Repository>, &'static str> {
        if workspace.ssh.is_none() {
            return Ok(Arc::new(Here { git: self.git.clone(), root: PathBuf::from(workspace.path()) }));
        }
        let repositories = self.ssh.lock().unwrap_or_else(|p| p.into_inner()).clone().ok_or("remote")?;
        repositories(workspace).await.map_err(|_| "sshUnreachable")
    }

    pub fn store(&self) -> &WorkMapSummaryStore {
        &self.store
    }

    pub fn is_local(workspace: &Workspace) -> bool {
        workspace.remote.is_none()
    }

    fn busy(&self) -> std::sync::MutexGuard<'_, Busy> {
        self.busy.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `ModelAsync`.
    pub async fn model(&self, workspace: &Workspace) -> WorkMapModel {
        if !Self::is_local(workspace) {
            return WorkMapModel::unavailable("remote");
        }
        let (summarizing, failed) = {
            let busy = self.busy();
            (busy.summarizing.get(workspace.id()).cloned(), busy.errors.get(workspace.id()).cloned().unwrap_or_default())
        };
        let state = (self.snapshot)();
        let root = workspace.path().to_string();
        let originals = &self.originals;
        let requests = crate::requests(state.sessions().iter().map(|s| s.as_ref()), workspace.id(), &root, &|pane, run, root| {
            originals.paths(pane, run, root)
        });
        let repository = match self.repository(workspace).await {
            Ok(repository) => repository,
            Err(reason) => return WorkMapModel::unavailable(reason),
        };
        crate::build_on(repository.as_ref(), &requests, &self.store.read(&root), summarizing.as_deref(), &failed).await
    }

    /// `StartAsync`: a version's summary started in the background, or why not.
    pub async fn start(self: &Arc<Self>, workspace: &Workspace, key: &str) -> WorkMapStart {
        if !Self::is_local(workspace) {
            return WorkMapStart::Unavailable;
        }
        {
            let mut busy = self.busy();
            if busy.summarizing.contains_key(workspace.id()) {
                return WorkMapStart::Busy;
            }
            busy.summarizing.insert(workspace.id().to_string(), key.to_string());
        }
        let model = self.model(workspace).await;
        if !model.available {
            self.release(workspace.id());
            return WorkMapStart::Unavailable;
        }
        let Some(version) = model.versions.into_iter().find(|v| v.key == key) else {
            self.release(workspace.id());
            return WorkMapStart::UnknownKey;
        };
        let service = self.clone();
        let workspace = workspace.clone();
        tokio::spawn(async move { service.summarize(&workspace, version).await });
        WorkMapStart::Started
    }

    fn release(&self, workspace_id: &str) {
        self.busy().summarizing.remove(workspace_id);
    }

    async fn summarize(&self, workspace: &Workspace, version: WorkMapVersion) {
        let outcome: Result<(), SummaryError> = async {
            let command = (self.find_claude)().await.ok_or(SummaryError::Cli)?;
            // Through the workspace reader (no link, nothing outside the folder), its first 64 KiB; an SSH workspace's over there
            // (MC-47: only this text and the log's goes into the prompt, and Claude runs here as for any workspace).
            let notes = match (version.version.as_deref(), self.repository(workspace).await) {
                (Some(name), Ok(repository)) => {
                    crate::read_start_of(repository.as_ref(), &format!("release/{name}-notes.md"), crate::RELEASE_FILE_BYTES).await
                }
                _ => None,
            };
            let prompt = summary::prompt(
                summary::language_name(self.locale.language()),
                version.version.as_deref().unwrap_or(&version.key),
                &version.commits,
                &version.requests,
                notes.as_deref(),
            );
            let answer = (self.runner)(command, prompt, summary::TIMEOUT).await?;
            let commits: Vec<(String, Vec<String>)> = version.commits.iter().map(|c| (c.sha.clone(), c.lanes.clone())).collect();
            let ids = version.requests.iter().map(|r| r.id.clone()).collect();
            let topics = summary::parse_answer(&answer, &commits, &ids, &self.locale.get("workMap.topic.other"))?;
            self.store.write(workspace.path(), &version.key, &version.range_end, &topics).map_err(|_| SummaryError::Output)
        }
        .await;
        {
            let mut busy = self.busy();
            let failed = busy.errors.entry(workspace.id().to_string()).or_default();
            match outcome {
                Ok(()) => {
                    failed.remove(&version.key);
                }
                Err(error) => {
                    failed.insert(version.key.clone(), summary::error_words(error, &self.locale));
                }
            }
            busy.summarizing.remove(workspace.id());
        }
        let changed = self.changed.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(changed) = changed {
            changed(workspace.id());
        }
    }
}
