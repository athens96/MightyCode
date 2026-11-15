//! The update's state and coordinator (C# `AppUpdateState`, `AppUpdateKeptCheck`, `AppUpdateCoordinator`, AppUpdateCoordinator.cs:43-463;
//! plan §1.4). Owner: W9-A; W9-C's host drives it and draws its state.
//!
//! Phases `Idle…Failed`; `check` with the staged-version rules (the same version staged, or a newer one not for this architecture, keeps
//! Ready); `download` deferring while staging is busy; `install(relaunch)`; `install_on_quit`; `check_kept_update`;
//! `retry_deferred_staging`; `press`; `show_for_smoke`. The presentation is mighty-core's `app_update_view::describe` over
//! [`UpdateState::view`].
//!
//! Every operation takes `&self`: the state sits behind a lock that is never held across an `.await`, so a press (say "restart now")
//! can land while a check is still waiting for the network, as on C#'s dispatcher. Drive every call from one task (C# runs them on
//! one dispatcher; `tokio::join!`/`select!` or a `LocalSet` interleave them only at their awaits). Await an operation to its end
//! rather than dropping it: a dropped download deletes its version folder, but its phase stays where it was. The change callback
//! ([`Coordinator::set_on_change`], C#'s `StateChanged`) runs after each change with no lock held, on the thread that made the change
//! (a download's progress arrives on the transport's).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use mighty_core::app_update_view::SectionView;
use mighty_core::locale::Locale;
use tokio_util::sync::CancellationToken;

use super::manifest::Availability;
use super::plan::InstallPlan;
use super::service::{STAGED_FOLDER, Service, ServiceError};
use super::{Phase, State, UpdateError};

/// What a look for a kept update at start found (C# `AppUpdateKeptCheck`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeptCheck {
    /// Nothing kept, or the switch off.
    #[default]
    None,
    /// A kept update that must wait (staging busy, a check or download running).
    Waiting,
    Checked,
}

/// What the app update box shows now (C# `AppUpdateState`); not saved.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateState {
    pub phase: Phase,
    pub download_fraction: f64,
    pub availability: Option<Availability>,
    pub package_path: Option<PathBuf>,
    pub staged_directory: Option<PathBuf>,
    pub install_plan: Option<InstallPlan>,
    pub error_message: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    /// The last failure, kept beside Ready so a failed background check never takes a staged update away.
    pub last_error: Option<String>,
}

impl UpdateState {
    /// What `app_update_view::describe` reads of it.
    pub fn view(&self) -> State {
        State {
            phase: self.phase,
            version: self.availability.as_ref().map(|a| a.manifest.version.clone()),
            notes: self.availability.as_ref().and_then(|a| a.manifest.notes.clone()),
            download_fraction: self.download_fraction,
            checked_at: self.checked_at,
            error_message: self.error_message.clone(),
            last_error: self.last_error.clone(),
        }
    }
}

/// Starts the detached helper with the plan (C#'s `launchHelper`); its error's words become the failure.
pub type LaunchHelper = Arc<dyn Fn(&InstallPlan) -> Result<(), String> + Send + Sync>;

/// A live yes/no the app answers (the saved "automatic" switch; whether staging must wait).
pub type Flag = Arc<dyn Fn() -> bool + Send + Sync>;

/// Whether a process other than this one runs an image from under a folder (the platform's `UpdateInstaller::running_from(folder,
/// true)`); an error reads as busy.
pub type RunningFrom = Arc<dyn Fn(&Path) -> std::io::Result<bool> + Send + Sync>;

/// How the coordinator is built (C#'s constructor arguments, the defaults C#'s).
#[derive(Clone)]
pub struct Options {
    /// This build's version.
    pub current_version: String,
    /// The build's address (Windows rule 3: a typed one is then ignored); kept only when allowed.
    pub built_in_manifest_url: Option<String>,
    /// The package this machine needs (`x64`, `arm64`; C#'s default `AppUpdateBuild.Architecture`).
    pub architecture: String,
    /// The installed app's folder; None (a development run) stages without an install plan.
    pub install_directory: Option<PathBuf>,
    /// None: "restart now" fails with `noInstallPlan`, and quitting installs nothing.
    pub launch_helper: Option<LaunchHelper>,
    /// The saved switch (C#'s default: off).
    pub automatic: Flag,
    /// Whether staging must wait; None asks [`super::housekeeping::staging_busy`] over `running_from` (C#'s default).
    pub staging_busy: Option<Flag>,
    /// The process check the default `staging_busy` makes. The host passes the platform's; the default finds nothing running.
    pub running_from: RunningFrom,
    /// This process's id, the helper's `--wait-pid`.
    pub process_id: i32,
    /// The language a refusal is worded in (C# words it when it throws); [`Coordinator::set_locale`] changes it.
    pub locale: Locale,
}

impl Options {
    /// C#'s defaults for a build at `current_version`.
    pub fn new(current_version: &str) -> Options {
        Options {
            current_version: current_version.to_string(),
            built_in_manifest_url: None,
            architecture: super::build::architecture(std::env::consts::ARCH).to_string(),
            install_directory: None,
            launch_helper: None,
            automatic: Arc::new(|| false),
            staging_busy: None,
            running_from: Arc::new(|_| Ok(false)),
            process_id: i32::try_from(std::process::id()).unwrap_or(i32::MAX),
            locale: Locale::new("en"),
        }
    }
}

#[derive(Default)]
struct Inner {
    state: UpdateState,
    /// The work a shutdown or a cancel stops, with the number it was given (C# compares the token itself).
    work: Option<(u64, CancellationToken)>,
    works: u64,
    closing: bool,
    checking: bool,
    last_attempt: Option<DateTime<Utc>>,
    staging_deferred: bool,
}

impl Inner {
    fn start_work(&mut self) -> (u64, CancellationToken) {
        self.works += 1;
        let token = CancellationToken::new();
        self.work = Some((self.works, token.clone()));
        (self.works, token)
    }

    fn busy(&self) -> bool {
        matches!(self.state.phase, Phase::Checking | Phase::Downloading | Phase::Staging | Phase::Installing)
    }
}

type OnChange = Arc<dyn Fn() + Send + Sync>;

/// The state and its change callback, shared with a download's progress (which arrives from the transport).
struct Shared {
    inner: Mutex<Inner>,
    on_change: Mutex<Option<OnChange>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// C#'s `Mutate`: the change, the deferral dropped once the phase leaves Available, then the callback with no lock held.
    fn mutate(&self, apply: impl FnOnce(&mut UpdateState)) {
        {
            let mut inner = self.lock();
            apply(&mut inner.state);
            if inner.state.phase != Phase::Available {
                inner.staging_deferred = false;
            }
        }
        let callback = self.on_change.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

/// C# `AppUpdateCoordinator`: one profile's update from check to install.
pub struct Coordinator {
    service: Arc<Service>,
    options: Options,
    built_in_manifest_url: Option<String>,
    locale: Mutex<Locale>,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Coordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coordinator").field("state", &self.state()).finish_non_exhaustive()
    }
}

impl Coordinator {
    /// A coordinator at rest over `service`.
    pub fn new(service: Arc<Service>, options: Options) -> Coordinator {
        let built_in_manifest_url = options.built_in_manifest_url.clone().filter(|url| super::manifest::allowed(url));
        Coordinator {
            service,
            locale: Mutex::new(options.locale),
            options,
            built_in_manifest_url,
            shared: Arc::new(Shared { inner: Mutex::new(Inner::default()), on_change: Mutex::new(None) }),
        }
    }

    /// The state now (a copy).
    pub fn state(&self) -> UpdateState {
        self.shared.lock().state.clone()
    }

    /// C# `StateChanged`: called after every change.
    pub fn set_on_change(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.shared.on_change.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(callback);
    }

    /// The language later refusals are worded in.
    pub fn set_locale(&self, locale: Locale) {
        *self.locale.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = locale;
    }

    fn words(&self, error: &UpdateError) -> String {
        error.message(&self.locale.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// The service it runs over.
    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }

    /// C# `BuiltInManifestUrl`.
    pub fn built_in_manifest_url(&self) -> Option<&str> {
        self.built_in_manifest_url.as_deref()
    }

    /// C# `HasPublicKey`.
    pub fn has_public_key(&self) -> bool {
        self.service.verifies_signatures()
    }

    /// C# `Automatic`: the saved switch, as the app reports it.
    pub fn automatic(&self) -> bool {
        (self.options.automatic)()
    }

    /// C# `LastAttempt`: the last automatic attempt this run, kept in memory so a failed check waits for the next cycle.
    pub fn last_attempt(&self) -> Option<DateTime<Utc>> {
        self.shared.lock().last_attempt
    }

    /// C# `StagingDeferred`: a download that found staging busy left the update Available, with no error, to be staged later.
    pub fn staging_deferred(&self) -> bool {
        self.shared.lock().staging_deferred
    }

    /// C# `EffectiveManifestUrl` (Windows rule 3): the build's address when it has one, else a typed allowed address (trimmed).
    pub fn effective_manifest_url(&self, user_url: Option<&str>) -> Option<String> {
        if let Some(url) = &self.built_in_manifest_url {
            return Some(url.clone());
        }
        user_url.map(str::trim).filter(|url| super::manifest::allowed(url)).map(String::from)
    }

    /// C# `IsDue`: [`super::auto::check_due`].
    pub fn is_due(
        enabled: bool,
        has_public_key: bool,
        last_checked: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        last_attempt: Option<DateTime<Utc>>,
    ) -> bool {
        super::auto::check_due(enabled, has_public_key, last_checked, last_attempt, now)
    }

    /// C# `Describe(formatTime)`: the box for the state now.
    pub fn describe(&self, locale: &Locale, format_time: impl Fn(&DateTime<Utc>) -> String) -> SectionView {
        mighty_core::app_update_view::describe(
            locale,
            &self.state().view(),
            self.has_public_key(),
            self.built_in_manifest_url(),
            format_time,
            self.automatic(),
        )
    }

    fn fail(&self, message: String) {
        self.shared.mutate(|s| {
            s.phase = Phase::Failed;
            s.error_message = Some(message.clone());
            s.last_error = Some(message);
        });
    }

    /// C#'s `FailCheck`: a failed check never drops a staged update; beside Ready it is only `last_error`.
    fn fail_check(&self, message: String) {
        if self.shared.lock().state.phase == Phase::Ready {
            self.shared.mutate(|s| s.last_error = Some(message));
        } else {
            self.fail(message);
        }
    }

    /// C#'s `StagingMustWait`.
    fn staging_must_wait(&self) -> bool {
        match &self.options.staging_busy {
            Some(busy) => busy(),
            None => super::housekeeping::staging_busy(
                self.service.updates_directory(),
                self.options.install_directory.as_deref(),
                Utc::now(),
                &*self.options.running_from,
            ),
        }
    }

    /// C# `CheckAutomaticallyIfDueAsync`: checks when due and remembers the attempt; true when it checked.
    pub async fn check_automatically_if_due(
        &self,
        user_url: Option<&str>,
        enabled: bool,
        last_checked: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> bool {
        {
            let mut inner = self.shared.lock();
            if inner.closing || inner.busy() || inner.checking {
                return false;
            }
            if !Self::is_due(enabled, self.has_public_key(), last_checked, now, inner.last_attempt) {
                return false;
            }
            inner.last_attempt = Some(now);
        }
        self.check(user_url).await;
        true
    }

    /// C# `CheckKeptUpdateAsync`: an update an install on quit kept is checked against the signed manifest at once and, while it is
    /// the newest for this machine and the switch is on, staged again from the package on disk. While staging is busy, or a check or
    /// download runs, it waits: nothing changes, no attempt counts.
    pub async fn check_kept_update(&self, user_url: Option<&str>, now: DateTime<Utc>) -> KeptCheck {
        if self.shared.lock().closing || !self.automatic() || !self.has_public_key() {
            return KeptCheck::None;
        }
        if self.service.kept_version(&self.options.current_version).is_none() {
            return KeptCheck::None;
        }
        {
            let inner = self.shared.lock();
            if inner.busy() || inner.checking {
                return KeptCheck::Waiting;
            }
        }
        if self.staging_must_wait() {
            return KeptCheck::Waiting;
        }
        self.shared.lock().last_attempt = Some(now);
        self.check(user_url).await;
        KeptCheck::Checked
    }

    /// C# `RetryDeferredStagingAsync`: on the loop's ticks, with the switch on, a deferred download is tried again.
    pub async fn retry_deferred_staging(&self) {
        let due = {
            let inner = self.shared.lock();
            inner.staging_deferred && inner.state.phase == Phase::Available
        };
        if due && self.automatic() {
            self.download().await;
        }
    }

    /// C# `CheckAsync(userUrl)`.
    pub async fn check(&self, user_url: Option<&str>) {
        {
            let inner = self.shared.lock();
            if inner.closing || inner.busy() || inner.checking {
                return;
            }
        }
        if !self.has_public_key() {
            self.fail_check(self.words(&UpdateError::key("settings.appUpdate.noPublicKeyNoticeWindows")));
            return;
        }
        let Some(url) = self.effective_manifest_url(user_url) else {
            self.fail_check(self.words(&UpdateError::key("settings.appUpdate.errorNoManifestURL")));
            return;
        };
        // A staged update stays ready (and installable) while the check runs.
        if self.shared.lock().state.phase != Phase::Ready {
            self.shared.mutate(|s| {
                s.phase = Phase::Checking;
                s.error_message = None;
            });
        }
        let (id, token) = {
            let mut inner = self.shared.lock();
            inner.checking = true;
            inner.start_work()
        };
        let answer = tokio::select! {
            answer = self.service.check(Some(&url), &self.options.current_version) => answer,
            () = token.cancelled() => Err(ServiceError::Cancelled),
        };
        let download = self.after_check(answer);
        {
            let mut inner = self.shared.lock();
            inner.checking = false;
            if inner.work.as_ref().is_some_and(|(work, _)| *work == id) {
                inner.work = None;
            }
        }
        if download {
            self.download().await;
        }
    }

    /// The decisions after a check's answer (C# `CheckAsync`'s try and catches); true when a download follows.
    fn after_check(&self, answer: Result<Availability, ServiceError>) -> bool {
        let phase = self.shared.lock().state.phase;
        match answer {
            Ok(availability) => {
                // "Restart now" was pressed meanwhile: the install owns the state.
                if !matches!(phase, Phase::Ready | Phase::Checking) {
                    return false;
                }
                let state = self.state();
                let staged = (phase == Phase::Ready).then(|| state.availability.as_ref().map(|a| a.manifest.version.clone())).flatten();
                let architecture = &self.options.architecture;
                let new_version = &availability.manifest.version;
                let same_as_staged = staged.as_deref().is_some_and(|staged| {
                    super::version::normalized(staged).is_some()
                        && super::version::normalized(new_version).is_some()
                        && super::version::compare(staged, new_version) == std::cmp::Ordering::Equal
                });
                let newer_but_not_for_this_machine = availability.is_newer() && availability.manifest.asset(architecture).is_none();
                if staged.is_some() && (same_as_staged || newer_but_not_for_this_machine) {
                    // The version the manifest names is the one staged, or a newer one this machine cannot install: the staged update
                    // stays ready and nothing is downloaded again (M/AppUpdateAutoSteps.swift afterCheck).
                    self.shared.mutate(|s| {
                        s.checked_at = Some(Utc::now());
                        s.last_error = None;
                    });
                    return false;
                }
                if newer_but_not_for_this_machine {
                    // Nothing staged and the newer version cannot run here: the Mac's "blocked" outcome — the reason is the status,
                    // nothing is downloaded, and it is no failure to retry.
                    let reason = self.words(&UpdateError::Key {
                        key: "appUpdate.error.noPackageForArchitecture",
                        args: vec![("architecture", architecture.clone())],
                    });
                    self.shared.mutate(|s| {
                        s.phase = Phase::Failed;
                        s.availability = Some(availability);
                        s.package_path = None;
                        s.staged_directory = None;
                        s.install_plan = None;
                        s.checked_at = Some(Utc::now());
                        s.error_message = Some(reason);
                        s.last_error = None;
                    });
                    return false;
                }
                let newer = availability.is_newer();
                self.shared.mutate(|s| {
                    s.phase = if newer { Phase::Available } else { Phase::UpToDate };
                    s.availability = Some(availability);
                    s.package_path = None;
                    s.staged_directory = None;
                    s.install_plan = None;
                    s.checked_at = Some(Utc::now());
                    s.error_message = None;
                    s.last_error = None;
                });
                newer && self.automatic()
            }
            Err(ServiceError::Cancelled) => {
                if phase == Phase::Checking {
                    self.shared.mutate(|s| s.phase = Phase::Idle);
                }
                false
            }
            Err(ServiceError::Refused(error)) => {
                if matches!(phase, Phase::Ready | Phase::Checking) {
                    self.fail_check(self.words(&error));
                }
                false
            }
        }
    }

    /// C# `DownloadAsync`: the package for this machine's architecture downloaded, verified and unpacked; a manifest with no asset for
    /// it is refused. While staging is busy nothing under `updates` is touched: the update stays Available, with no error, and waits.
    pub async fn download(&self) {
        let manifest = {
            let inner = self.shared.lock();
            if inner.closing || inner.checking || inner.state.phase != Phase::Available {
                return;
            }
            match &inner.state.availability {
                Some(availability) => availability.manifest.clone(),
                None => return,
            }
        };
        let architecture = self.options.architecture.clone();
        let Some(asset) = manifest.asset(&architecture).cloned() else {
            self.fail(
                self.words(&UpdateError::Key {
                    key: "appUpdate.error.noPackageForArchitecture",
                    args: vec![("architecture", architecture)],
                }),
            );
            return;
        };
        if self.staging_must_wait() {
            self.shared.lock().staging_deferred = true;
            return;
        }
        self.shared.lock().staging_deferred = false;
        self.shared.mutate(|s| {
            s.phase = Phase::Downloading;
            s.download_fraction = 0.0;
            s.error_message = None;
        });
        let (_, token) = self.shared.lock().start_work();
        let progress: Arc<dyn Fn(f64) + Send + Sync> = {
            let shared = self.shared.clone();
            Arc::new(move |fraction| shared.mutate(|s| s.download_fraction = fraction))
        };
        let downloaded = self.service.download(&asset, &manifest.version, progress, &token).await;
        match downloaded {
            Ok(package) => self.stage(package, &asset, &architecture).await,
            Err(ServiceError::Cancelled) => self.shared.mutate(|s| {
                s.phase = Phase::Available;
                s.download_fraction = 0.0;
                s.package_path = None;
            }),
            Err(ServiceError::Refused(error)) => self.fail(self.words(&error)),
        }
        self.shared.lock().work = None;
    }

    /// The rest of C#'s `DownloadAsync` once the package is on disk: asked again whether staging must wait, then unpacked (off this
    /// thread) and the install plan made.
    async fn stage(&self, package: PathBuf, asset: &super::manifest::Asset, architecture: &str) {
        if self.staging_must_wait() {
            self.shared.lock().staging_deferred = true;
            self.shared.mutate(|s| {
                s.phase = Phase::Available;
                s.download_fraction = 0.0;
                s.package_path = None;
            });
            return;
        }
        self.shared.mutate(|s| {
            s.phase = Phase::Staging;
            s.package_path = Some(package.clone());
        });
        let destination = package.parent().map(|folder| folder.join(STAGED_FOLDER)).unwrap_or_else(|| PathBuf::from(STAGED_FOLDER));
        let staged = {
            let (package, architecture) = (package.clone(), architecture.to_string());
            tokio::task::spawn_blocking(move || super::stage::stage(&package, &destination, &architecture))
                .await
                .unwrap_or_else(|join| Err(UpdateError::Plain(join.to_string())))
        };
        match staged {
            Ok(staged) => {
                let plan = self.options.install_directory.as_deref().map(|install| {
                    InstallPlan::create(
                        &staged,
                        install,
                        &package,
                        &asset.sha256,
                        i64::try_from(asset.size).unwrap_or(i64::MAX),
                        self.options.process_id,
                        None,
                        Utc::now(),
                    )
                });
                self.shared.mutate(|s| {
                    s.phase = Phase::Ready;
                    s.staged_directory = Some(staged);
                    s.install_plan = plan;
                    s.last_error = None;
                });
            }
            Err(error) => self.fail(self.words(&error)),
        }
    }

    /// C# `CancelDownload`.
    pub fn cancel_download(&self) {
        self.service.cancel_download();
        if let Some((_, work)) = &self.shared.lock().work {
            work.cancel();
        }
    }

    /// C# `InstallAsync(relaunch)`: hands the swap to the detached helper and reports the phase; the caller quits the app (the helper
    /// waits for it). `relaunch` false is the install on quit (`--relaunch 0`).
    pub fn install(&self, relaunch: bool) {
        let staged = {
            let inner = self.shared.lock();
            if inner.closing || inner.state.phase != Phase::Ready {
                return;
            }
            inner.state.install_plan.clone()
        };
        let (Some(staged), Some(launch)) = (staged, self.options.launch_helper.clone()) else {
            self.fail(self.words(&UpdateError::key("appUpdate.error.noInstallPlan")));
            return;
        };
        let plan = InstallPlan { relaunch, ..staged };
        self.shared.mutate(|s| {
            s.phase = Phase::Installing;
            s.error_message = None;
        });
        if let Err(error) = launch(&plan) {
            self.fail(error);
        }
    }

    /// C# `InstallOnQuitAsync(poweringOff)`: called as the app quits, before [`Coordinator::shutdown`]; a staged update is installed
    /// once the app has gone, without starting it again. Skipped with the switch off, while an install is under way, while Windows
    /// signs out or shuts down, in a build that cannot verify updates, and in a run without an install plan. True when the helper was
    /// started.
    pub fn install_on_quit(&self, powering_off: bool) -> bool {
        let ready = {
            let inner = self.shared.lock();
            if inner.closing || inner.state.install_plan.is_none() || self.options.launch_helper.is_none() {
                return false;
            }
            inner.state.phase == Phase::Ready
        };
        if !super::auto::installs_on_quit(ready, self.automatic(), self.has_public_key(), powering_off) {
            return false;
        }
        self.install(false);
        self.shared.lock().state.phase == Phase::Installing
    }

    /// C# `ShutdownAsync`: nothing starts after it; the work under way is cancelled.
    pub fn shutdown(&self) {
        let work = {
            let mut inner = self.shared.lock();
            inner.closing = true;
            inner.work.as_ref().map(|(_, work)| work.clone())
        };
        if let Some(work) = work {
            work.cancel();
        }
        self.service.cancel_download();
    }

    /// C# `ShowForSmoke(state)`: a fixture phase on the real box; no network, no disk.
    pub fn show_for_smoke(&self, state: UpdateState) {
        self.shared.mutate(|s| *s = state);
    }

    /// C# `PressAsync(userUrl)`: the one button, routed by the phase.
    pub async fn press(&self, user_url: Option<&str>) {
        let phase = self.shared.lock().state.phase;
        match phase {
            Phase::Available => self.download().await,
            Phase::Downloading => self.cancel_download(),
            Phase::Ready => self.install(true),
            Phase::Checking | Phase::Staging | Phase::Installing => {}
            Phase::Idle | Phase::UpToDate | Phase::Failed => self.check(user_url).await,
        }
    }
}
