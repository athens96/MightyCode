//! The host's loop, restart offer, phone and quit over a live coordinator, its service handed mighty-platform's `FakeUpdateTransport`
//! (no socket, never the real address) and the fixture manifest C# signed (`update-manifests/valid.json`, 9.9.9 for x64 and arm64), on
//! tokio's paused clock with a `LocalSet`, as the app's driver thread runs it.

use std::sync::atomic::AtomicUsize;

use mighty_platform::fake::{FakeProcessHost, FakeRoute, FakeUpdateTransport};
use mighty_updates::app_update::coordinator::UpdateState;
use mighty_updates::app_update::service::{PACKAGE_NAME, STAGED_FOLDER, UPDATES_FOLDER};
use mighty_updates::app_update::smoke::fixture_states;

use super::*;

const MANIFEST: &str = "https://example.invalid/latest.json";
const VALID: &[u8] = include_bytes!("../../../../contracts/fixtures/update-manifests/valid.json");
/// The key `valid.json` is signed with (`update-manifests/cases.json` "fixture").
const FIXTURE_KEY: &str = "dVxMuSVsp83ErP3Gz+7ahJAX5bn5UU6ZGRvWfgsNQnY=";

fn architecture() -> &'static str {
    build::architecture(std::env::consts::ARCH)
}

/// The manifest's package for this machine (not served: a download fails).
fn package_url() -> String {
    format!("https://updates.example/9.9.9/MightyCode-windows-{}.zip", architecture())
}

#[derive(Default)]
struct TestSeams {
    settings: Mutex<Settings>,
    facts: Mutex<IdleFacts>,
    saved: Mutex<Vec<DateTime<Utc>>>,
    changes: AtomicUsize,
}

impl Seams for TestSeams {
    fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }
    fn idle_facts(&self, _now: DateTime<Utc>) -> IdleFacts {
        lock(&self.facts).clone()
    }
    fn save_checked_at(&self, at: DateTime<Utc>) {
        lock(&self.saved).push(at);
        lock(&self.settings).last_checked_at = Some(dotnet::round_trip(at));
    }
    fn changed(&self) {
        self.changes.fetch_add(1, Ordering::SeqCst);
    }
}

struct Rig {
    core: Arc<Core>,
    seams: Arc<TestSeams>,
    transport: Arc<FakeUpdateTransport>,
    launches: Arc<Mutex<Vec<InstallPlan>>>,
    busy: Arc<AtomicBool>,
    folder: PathBuf,
    commands: mpsc::UnboundedSender<Command>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

static RIGS: AtomicUsize = AtomicUsize::new(0);

/// A core over a live coordinator (version 0.4.59), its driver running on the current `LocalSet`. `allowed`: the build may change the
/// machine; `key`: it carries the fixture key.
fn rig(allowed: bool, key: bool) -> Rig {
    let folder =
        std::env::temp_dir().join(format!("mighty-app-update-host-{}-{}", std::process::id(), RIGS.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(folder.join("install")).unwrap();
    let transport = Arc::new(FakeUpdateTransport::default());
    transport.route(MANIFEST, FakeRoute::ok(VALID.to_vec()));
    let service = Arc::new(Service::new(transport.clone(), &folder, key.then(|| build::public_key(Some(FIXTURE_KEY)).unwrap())));
    let seams = Arc::new(TestSeams::default());
    let launches = Arc::new(Mutex::new(Vec::new()));
    let busy = Arc::new(AtomicBool::new(false));
    let mut options = Options::new("0.4.59");
    options.install_directory = Some(folder.join("install"));
    let reads = seams.clone();
    options.automatic = Arc::new(move || lock(&reads.settings).automatic);
    let running = busy.clone();
    options.running_from = Arc::new(move |_| Ok(running.load(Ordering::SeqCst)));
    let recorded = launches.clone();
    options.launch_helper = Some(Arc::new(move |plan: &InstallPlan| {
        lock(&recorded).push(plan.clone());
        Ok(())
    }));
    let coordinator = Arc::new(Coordinator::new(service, options));
    let start = Instant::now();
    let base = Utc::now();
    let clock: Clock = Arc::new(move || base + chrono::Duration::from_std(Instant::now() - start).unwrap_or_default());
    let core = Core::new(coordinator.clone(), seams.clone(), clock, allowed, false);
    let (commands, received) = mpsc::unbounded_channel::<Command>();
    wire(&coordinator, &commands);
    tokio::task::spawn_local(drive(core.clone(), received));
    Rig { core, seams, transport, launches, busy, folder, commands }
}

impl Rig {
    fn set(&self, change: impl FnOnce(&mut Settings)) {
        change(&mut lock(&self.seams.settings));
    }
    fn send(&self, work: impl FnOnce(&Arc<Core>) + Send + 'static) {
        let _ = self.commands.send(Box::new(work));
    }
    fn asks(&self, url: &str) -> usize {
        self.transport.asked().iter().filter(|asked| *asked == url).count()
    }
    /// Automatic, the address typed, checked just now (no check due).
    fn automatic_checked_now(&self) {
        let now = (self.core.clock)();
        self.set(|s| {
            s.automatic = true;
            s.url = Some(MANIFEST.into());
            s.last_checked_at = Some(dotnet::round_trip(now));
        });
    }
    /// A ready update (the smoke's fixture) with an install plan.
    fn ready(&self) -> UpdateState {
        let mut ready = fixture_states(&Locale::new("en")).into_iter().find(|s| s.phase == Phase::Ready).unwrap();
        let staged = self.folder.join(UPDATES_FOLDER).join("9.9.9").join(STAGED_FOLDER);
        ready.install_plan = Some(InstallPlan::create(
            &staged,
            &self.folder.join("install"),
            &self.folder.join(PACKAGE_NAME),
            &"a".repeat(64),
            1024,
            4242,
            None,
            Utc::now(),
        ));
        ready
    }
    fn counting_down(&self) -> bool {
        matches!(self.core.restart(), RestartState::CountingDown(_))
    }
}

/// The driver and the work it spawned run until they wait.
async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

/// `duration` passes on the paused clock, the driver ticking as it goes.
async fn pass(duration: Duration) {
    tokio::time::advance(duration).await;
    settle().await;
}

async fn local<F: std::future::Future<Output = ()>>(test: F) {
    tokio::task::LocalSet::new().run_until(test).await;
}

/// C# `BeginAutomaticAppUpdateCheck`, `CheckAppUpdateIfDue`, `AppUpdateTick`: a check at once when one is due, the check time saved
/// once per check, nothing more for the 6 h after the attempt however often the 10 min loop ticks, then the next; the switch off stops
/// the loop.
#[tokio::test(start_paused = true)]
async fn the_loop_checks_when_due_then_waits_six_hours_and_stops_with_the_switch() {
    local(async {
        let rig = rig(true, true);
        rig.set(|s| {
            s.automatic = true;
            s.url = Some(MANIFEST.into());
        });
        rig.send(|core| core.begin());
        settle().await;
        assert_eq!(rig.asks(MANIFEST), 1, "never checked: due at once");
        assert_eq!(lock(&rig.seams.saved).len(), 1, "the check time saved once");
        assert_eq!(rig.asks(&package_url()), 1, "a newer version is downloaded with the switch on");
        assert_eq!(rig.core.coordinator.state().phase, Phase::Failed, "the fixture's package is not served");
        assert!(rig.seams.changes.load(Ordering::SeqCst) > 0, "the box and the phone follow each change");
        for _ in 0..6 {
            pass(auto::TICK).await;
        }
        assert_eq!(rig.asks(MANIFEST), 1, "the attempt holds the next check for 6 h");
        pass(auto::CHECK_INTERVAL).await;
        assert_eq!(rig.asks(MANIFEST), 2, "due again 6 h after the attempt");
        assert_eq!(lock(&rig.seams.saved).len(), 2);
        rig.set(|s| s.automatic = false);
        rig.send(|core| core.automatic_changed(false));
        settle().await;
        assert!(rig.core.next_tick().is_none(), "the switch off stops the loop");
        pass(auto::CHECK_INTERVAL * 2).await;
        assert_eq!(rig.asks(MANIFEST), 2);
    })
    .await;
}

/// The loop runs only where the build may change the machine and verifies manifests it knows the address of (plan §4): a read-only
/// or development launch, a build without the key or no address never checks, and the box's press does nothing where the build may not
/// change the machine; with `appUpdateAutoCheck` off nothing checks or installs on its own (the rehearsal relies on it).
#[tokio::test(start_paused = true)]
async fn nothing_checks_where_the_build_may_not_or_cannot() {
    local(async {
        for (allowed, key, url, automatic) in
            [(false, true, true, true), (true, false, true, true), (true, true, false, true), (true, true, true, false)]
        {
            let rig = rig(allowed, key);
            rig.set(|s| {
                s.automatic = automatic;
                s.url = url.then(|| MANIFEST.into());
            });
            rig.send(|core| core.begin());
            settle().await;
            pass(auto::CHECK_INTERVAL).await;
            assert_eq!(rig.asks(MANIFEST), 0, "allowed {allowed}, key {key}, address {url}, automatic {automatic}");
            if !automatic {
                // `appUpdateAutoCheck` off (C#): no loop, no check, no install on quit; the phone still reads the state.
                assert!(rig.core.next_tick().is_none() && rig.core.phone_read().is_some());
                rig.core.coordinator.show_for_smoke(rig.ready());
                assert!(!rig.core.quit(false) && lock(&rig.launches).is_empty());
                continue;
            }
            assert!(!rig.core.can_run_automatically());
            assert!(rig.core.phone_read().is_none(), "the phone's 409 stays");
            if !allowed {
                rig.core.clone().press().await;
                assert_eq!(rig.asks(MANIFEST), 0, "the press waits for a build that may change the machine");
            }
        }
    })
    .await;
}

/// C# `RestartOffer` on the loop: idle and ready, the banner counts down 60 s at a 1 s pace; input alone does not cancel it; at its end
/// the helper starts with `--relaunch 1`.
#[tokio::test(start_paused = true)]
async fn an_idle_ready_app_counts_down_and_restarts_with_input_alone_not_cancelling() {
    local(async {
        let rig = rig(true, true);
        rig.automatic_checked_now();
        rig.send(|core| core.begin());
        settle().await;
        rig.core.coordinator.show_for_smoke(rig.ready());
        pass(auto::TICK).await;
        assert!(rig.counting_down(), "idle and ready: the countdown starts");
        let locale = Locale::new("en");
        let message = locale.get_with("appUpdate.countdown.message", &[("version", "9.9.9"), ("seconds", "60")]);
        assert_eq!(rig.core.frame(&locale).countdown.as_deref(), Some(message.as_str()));
        for _ in 0..30 {
            pass(auto::COUNTDOWN_TICK).await;
        }
        lock(&rig.seams.facts).last_user_input = Some((rig.core.clock)());
        pass(auto::COUNTDOWN_TICK).await;
        assert!(rig.counting_down(), "input alone does not cancel it");
        assert!(lock(&rig.launches).is_empty());
        for _ in 0..30 {
            pass(auto::COUNTDOWN_TICK).await;
        }
        let launches = lock(&rig.launches).clone();
        assert_eq!(launches.len(), 1, "the countdown ran out: the helper started");
        assert!(launches[0].relaunch);
        assert_eq!(rig.core.restart(), RestartState::Idle);
        assert_eq!(rig.core.coordinator.state().phase, Phase::Installing);
    })
    .await;
}

/// A countdown found more than 5 s past its end (the computer slept through it) is cancelled, never acted on; the next idle tick offers
/// a fresh one. "Later" holds the banner for 4 h. A draft cancels a running countdown at once.
#[tokio::test(start_paused = true)]
async fn a_slept_through_countdown_is_cancelled_later_snoozes_and_a_draft_cancels() {
    local(async {
        let rig = rig(true, true);
        rig.automatic_checked_now();
        rig.send(|core| core.begin());
        settle().await;
        rig.core.coordinator.show_for_smoke(rig.ready());
        pass(auto::TICK).await;
        assert!(rig.counting_down());
        pass(auto::COUNTDOWN + Duration::from_secs(10)).await;
        assert_eq!(rig.core.restart(), RestartState::Idle, "slept past the grace: cancelled");
        assert!(lock(&rig.launches).is_empty(), "never acted on");
        pass(auto::TICK).await;
        assert!(rig.counting_down(), "the next idle tick offers a fresh countdown");

        rig.send(|core| core.postpone());
        settle().await;
        assert!(matches!(rig.core.restart(), RestartState::Snoozed(_)));
        assert!(rig.core.frame(&Locale::new("en")).countdown.is_none(), "the banner goes");
        pass(Duration::from_secs(3 * 60 * 60)).await;
        assert!(matches!(rig.core.restart(), RestartState::Snoozed(_)), "no banner within the 4 h");
        pass(Duration::from_secs(70 * 60)).await;
        assert!(rig.counting_down(), "after the snooze the offer comes back");

        lock(&rig.seams.facts).drafts = 1;
        pass(auto::COUNTDOWN_TICK).await;
        assert_eq!(rig.core.restart(), RestartState::Idle, "a draft cancels it quietly");
        pass(auto::COUNTDOWN * 2).await;
        assert!(lock(&rig.launches).is_empty());
    })
    .await;
}

/// C# `CheckAppUpdateOnTick`: an update an install on quit kept is looked at from the first tick on, before any due check; it waits
/// tick after tick while staging is busy, then is checked once.
#[tokio::test(start_paused = true)]
async fn a_kept_update_is_checked_first_and_waits_while_staging_is_busy() {
    local(async {
        let rig = rig(true, true);
        let kept = rig.folder.join(UPDATES_FOLDER).join("9.9.9");
        std::fs::create_dir_all(kept.join(STAGED_FOLDER)).unwrap();
        std::fs::write(kept.join(PACKAGE_NAME), b"zip").unwrap();
        std::fs::write(kept.join(STAGED_FOLDER).join("MightyCode.exe"), b"MZ").unwrap();
        rig.automatic_checked_now();
        rig.busy.store(true, Ordering::SeqCst);
        rig.send(|core| core.begin());
        settle().await;
        assert_eq!(rig.asks(MANIFEST), 0, "checked just now: no due check, and the kept update waits for the first tick");
        pass(auto::TICK).await;
        pass(auto::TICK).await;
        assert_eq!(rig.asks(MANIFEST), 0, "staging busy: the kept update waits");
        rig.busy.store(false, Ordering::SeqCst);
        pass(auto::TICK).await;
        assert_eq!(rig.asks(MANIFEST), 1, "then it is checked against the signed manifest");
        pass(auto::TICK).await;
        assert_eq!(rig.asks(MANIFEST), 1, "once");
    })
    .await;
}

/// C# `RetryDeferredStagingAsync` on the loop: a download that found staging busy leaves the update Available with no error, and a
/// later tick downloads it.
#[tokio::test(start_paused = true)]
async fn a_deferred_download_is_tried_again_on_a_tick() {
    local(async {
        let rig = rig(true, true);
        rig.automatic_checked_now();
        let available = fixture_states(&Locale::new("en")).into_iter().find(|s| s.phase == Phase::Available).unwrap();
        rig.core.coordinator.show_for_smoke(available);
        rig.busy.store(true, Ordering::SeqCst);
        std::fs::create_dir_all(rig.folder.join(UPDATES_FOLDER)).unwrap();
        rig.send(|core| core.automatic_changed(true));
        settle().await;
        assert!(rig.core.coordinator.staging_deferred());
        assert_eq!(rig.core.coordinator.state().phase, Phase::Available);
        let fixture_package = format!("https://example.invalid/MightyCode-windows-{}.zip", architecture());
        assert_eq!(rig.asks(&fixture_package), 0);
        rig.busy.store(false, Ordering::SeqCst);
        pass(auto::TICK).await;
        assert_eq!(rig.asks(&fixture_package), 1, "the tick downloads what waited");
    })
    .await;
}

/// PhoneAppUpdateVerification.cs's four checks, the phone's halves against the live coordinator (W8 ported the router's in
/// mighty-remote): the phone reads the coordinator's state only where the app updates itself; its check is the coordinator's; its
/// install goes on through the download by itself and ends on a failure; a ready update installs at once with `--relaunch 1`.
#[tokio::test(start_paused = true)]
async fn phone_app_update_the_phones_requests_drive_the_live_coordinator() {
    local(async {
        let rig = rig(true, true);
        assert!(rig.core.phone_read().is_none(), "no address: no state");
        rig.set(|s| s.url = Some(MANIFEST.into()));
        let read = rig.core.phone_read().expect("an app that updates itself has a state");
        assert_eq!(phone_update::map(Some(&read), "0.4.59", 0).unwrap().phase, "idle");

        rig.core.clone().phone_act(false).await;
        settle().await;
        let mapped = phone_update::map(rig.core.phone_read().as_ref(), "0.4.59", 2).unwrap();
        assert_eq!((mapped.phase.as_str(), mapped.available.as_deref(), mapped.running), ("available", Some("9.9.9"), 2));
        assert_eq!(rig.asks(&package_url()), 0, "a check only checks (the switch is off)");

        rig.core.clone().phone_act(true).await;
        settle().await;
        assert_eq!(rig.asks(&package_url()), 1, "the phone's install downloads by itself");
        assert_eq!(rig.core.coordinator.state().phase, Phase::Failed);
        assert!(!rig.core.state().phone_requested, "a failure ends the request: it never loops");
        assert_eq!(phone_update::map(rig.core.phone_read().as_ref(), "0.4.59", 0).unwrap().phase, "failed");

        rig.core.coordinator.show_for_smoke(rig.ready());
        rig.core.clone().phone_act(true).await;
        settle().await;
        let launches = lock(&rig.launches).clone();
        assert_eq!(launches.len(), 1, "a ready update installs at once");
        assert!(launches[0].relaunch);
    })
    .await;
}

/// C# `InstallAppUpdateOnQuitAsync`: a ready update installs once the app has gone (`--relaunch 0`), not while Windows ends the session
/// (unless it called that off), not with the switch off; nothing starts after the quit.
#[tokio::test(start_paused = true)]
async fn quitting_installs_a_ready_update_unless_windows_ends_the_session() {
    local(async {
        let powering_off = rig(true, true);
        powering_off.automatic_checked_now();
        powering_off.core.coordinator.show_for_smoke(powering_off.ready());
        powering_off.core.session(SessionEnding::Query);
        assert!(!powering_off.core.quit(false), "Windows ends the session: the update waits");
        assert!(lock(&powering_off.launches).is_empty());

        let called_off = rig(true, true);
        called_off.automatic_checked_now();
        called_off.core.coordinator.show_for_smoke(called_off.ready());
        called_off.core.session(SessionEnding::Query);
        called_off.core.session(SessionEnding::End { ending: false });
        assert!(called_off.core.quit(false), "the end was called off: the quit installs");
        let launches = lock(&called_off.launches).clone();
        assert_eq!(launches.len(), 1);
        assert!(!launches[0].relaunch, "an install on quit does not start the app again");
        assert!(called_off.core.closing());
        called_off.send(|core| core.begin());
        settle().await;
        assert!(called_off.core.next_tick().is_none(), "nothing starts after the quit");

        let off = rig(true, true);
        off.set(|s| s.url = Some(MANIFEST.into()));
        off.core.coordinator.show_for_smoke(off.ready());
        assert!(!off.core.quit(false), "the switch off: quitting installs nothing");
        assert!(lock(&off.launches).is_empty());
    })
    .await;
}

/// `RefreshStatusBar`'s update rows: the staged words with the switch on, the badge otherwise; nothing without a newer version.
#[tokio::test(start_paused = true)]
async fn the_status_bar_says_staged_or_shows_the_badge() {
    local(async {
        let rig = rig(true, true);
        let locale = Locale::new("en");
        rig.automatic_checked_now();
        rig.core.coordinator.show_for_smoke(rig.ready());
        let frame = rig.core.frame(&locale);
        assert_eq!(frame.staged, Some(locale.get_with("window.status.updateStaged", &[("version", "9.9.9")])));
        assert_eq!((frame.update, frame.staged_version.as_deref()), (None, Some("9.9.9")));
        rig.set(|s| s.automatic = false);
        let badge = Some(locale.get_with("window.status.updateBadge", &[("version", "9.9.9")]));
        let frame = rig.core.frame(&locale);
        assert_eq!((frame.staged, frame.update), (None, badge));
        rig.core.coordinator.show_for_smoke(UpdateState::default());
        assert_eq!(rig.core.frame(&locale), AppUpdateFrame::default());
    })
    .await;
}

/// C# `LaunchUpdateHelperAsync`: the staged exe started detached and hidden in the temporary folder with exactly
/// `SystemRoot=C:\Windows` and the plan's arguments; a relaunch then quits the app; a refused start is the failure's words; a helper
/// outside the staged folder is refused.
#[test]
fn the_helper_starts_detached_from_the_stage_with_only_system_root() {
    let processes = FakeProcessHost::default();
    let locale = Locale::new("en");
    let folder = std::env::temp_dir().join("mighty-app-update-launch");
    let mut plan = InstallPlan::create(
        &folder.join("staged"),
        &folder.join("install"),
        &folder.join(PACKAGE_NAME),
        &"a".repeat(64),
        1024,
        4242,
        None,
        Utc::now(),
    );
    let quits = AtomicUsize::new(0);
    let quit = || {
        quits.fetch_add(1, Ordering::SeqCst);
    };
    assert_eq!(launch_helper(&processes, &locale, &plan, &quit), Ok(()));
    let started = processes.detached();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].program, plan.staged_helper_executable());
    assert_eq!(started[0].args, plan.helper_arguments());
    assert_eq!(started[0].cwd, std::env::temp_dir());
    assert!(started[0].hidden);
    let only = BTreeMap::from([("SystemRoot".to_string(), "C:\\Windows".to_string())]);
    assert!(started[0].environment == DetachedEnvironment::Exactly(only));
    assert_eq!(quits.load(Ordering::SeqCst), 1, "a relaunch quits the app");
    plan.relaunch = false;
    assert_eq!(launch_helper(&processes, &locale, &plan, &quit), Ok(()));
    assert_eq!(quits.load(Ordering::SeqCst), 1, "an install on quit is already quitting");
    processes.refuse_detached(true);
    assert_eq!(
        launch_helper(&processes, &locale, &plan, &quit),
        Err("The job does not allow breakaway.".to_string()),
        "the refusal says why"
    );

    let staged = folder.join("staged");
    assert!(inside_stage(&staged.join("MightyCode.exe"), &staged));
    assert!(!inside_stage(&folder.join("other").join("MightyCode.exe"), &staged));
    assert!(!inside_stage(&staged.join("..").join("MightyCode.exe"), &staged));
    assert!(!inside_stage(&folder.join("staged-not").join("MightyCode.exe"), &staged), "a sibling with the same start");
}
