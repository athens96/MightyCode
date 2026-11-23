//! MC-33: a Mighty Code server installed, updated, paired with and removed on an SSH machine — over a stand-in `ssh` (a `/bin/sh`
//! script that runs the remote command as the far side's shell would, in a home of its own whose name needs quoting), never a real
//! host. The far side's tools are this Mac's own (`ditto`, `defaults`, `shasum`, `xattr`), but `codesign`, which would refuse the
//! unsigned stand-in app, is a script that records how it was asked. The "app" is a folder whose executable is a script answering
//! `--cli server status|stop|start|ssh-pair` as the real CLI does, and the far side's `/Applications/MightyCode.app` is pointed at a
//! folder of the check's own: no real app ever runs, not even its CLI.
#![cfg(target_os = "macos")]

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mighty_core::ssh::failure::Failure;
use mighty_core::ssh::server::{AppPlace, HOME_APP, INSTALL_FOLDER_PREFIX, UnpackRefusal, requirement};
use mighty_core::ssh::{Mux, Target};
use mighty_run::ssh::SshLaunch;
use mighty_run::ssh_server::{InstallPlan, Progress, RemoteServer, ServerError, Step};
use sha2::{Digest, Sha256};

const CODE: &str = "c0de-c0de-c0de-c0de-c0de-c0de-c0de-c0de-c0de";

fn script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

struct Machine {
    root: PathBuf,
    home: PathBuf,
    ssh: PathBuf,
}

impl Machine {
    fn new(name: &str) -> Machine {
        let root = common::temp(name);
        let home = root.join("it's home");
        std::fs::create_dir_all(&home).unwrap();
        let fake = root.join("fake-bin");
        std::fs::create_dir_all(&fake).unwrap();
        script(
            &fake.join("codesign"),
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{root}/codesign.args'\n\
                 if [ -f '{root}/bad-signature' ]; then echo 'code object is not signed at all' >&2; exit 1; fi\nexit 0\n",
                root = root.display()
            ),
        );
        let ssh = root.join("ssh");
        // Every call's arguments recorded (one per line, a blank line between calls); the far side's tools are the system's, but the
        // step's own PATH line is pointed at the stand-in `codesign` first.
        script(
            &ssh,
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{root}/ssh.args'\nprintf '\\n' >> '{root}/ssh.args'\n\
                 while [ \"$#\" -gt 0 ]; do if [ \"$1\" = \"--\" ]; then shift; break; fi; shift; done\n\
                 shift\n\
                 if [ -f '{root}/lost' ]; then echo 'ssh: connect to host devbox port 22: Connection refused' >&2; exit 255; fi\n\
                 {decode}\n\
                 command=$(decode \"$1\" | sed -e 's|PATH=/usr/bin:/bin:/usr/sbin:/sbin|PATH={fake}:/usr/bin:/bin:/usr/sbin:/sbin|' \
                 -e 's|HOME/Applications/|HOME/@HOMEAPPS@/|g' -e 's|/Applications/MightyCode.app|{system}|g')\n\
                 case \"$command\" in *'/Applications/MightyCode'*|*MightyCode-local*) echo 'the installed app is never run' >&2; exit 99;; esac\n\
                 command=$(printf '%s' \"$command\" | sed -e 's|@HOMEAPPS@|Applications|g')\n\
                 exec /usr/bin/env -i HOME='{home}' PATH=/usr/bin:/bin SHELL=/bin/zsh /bin/sh -c \"$command\"\n",
                decode = common::DECODE_REMOTE,
                root = root.display(),
                fake = fake.display(),
                system = root.join("system").join("MightyCode.app").display(),
                home = home.display().to_string().replace('\'', "'\\''"),
            ),
        );
        Machine { root, home, ssh }
    }

    fn server(&self) -> RemoteServer {
        let launch = SshLaunch {
            program: self.ssh.clone(),
            target: Target::parse("devbox").unwrap(),
            mux: Mux::Shared("/tmp/mc-ssh-server-test".into()),
            dir: "~".into(),
            local: self.root.clone(),
        };
        RemoteServer::new(common::processes(), launch)
    }

    /// A stand-in app of `version` zipped as the release zips it (`ditto -c -k --keepParent`): its package and SHA-256.
    fn package(&self, version: &str) -> (PathBuf, String) {
        let build = self.root.join(format!("build-{version}"));
        let app = build.join("MightyCode.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
                 \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\
                 <key>CFBundleIdentifier</key><string>dev.mightycode.native</string>\
                 <key>CFBundleShortVersionString</key><string>{version}</string>\
                 <key>CFBundleExecutable</key><string>MightyCode</string></dict></plist>\n"
            ),
        )
        .unwrap();
        let state = self.root.join("server");
        let calls = self.root.join("calls");
        script(
            &app.join("Contents/MacOS/MightyCode"),
            &format!(
                "#!/bin/sh\n[ \"$1\" = --cli ] || exit 9\n\
                 case \"$2 $3\" in\n\
                 'server status') if [ -f '{state}' ]; then printf '{{\\n  \"running\": true,\\n  \"mode\": \"headless\",\\n  \"version\": \"%s\"\\n}}\\n' \"$(cat '{state}')\"; exit 0; fi; printf '{{\"running\": false}}\\n'; exit 1;;\n\
                 'server stop') echo stop-{version} >> '{calls}'; rm -f '{state}'; printf '{{\"stopped\": true}}\\n';;\n\
                 'server start') echo start-{version} >> '{calls}'; if [ -f '{root}/no-gui' ]; then printf '{{\"ok\": false, \"error\": {{\"code\": \"timeout\", \"message\": \"the app (pid 9) started but its control API did not answer\"}}}}\\n'; exit 2; fi; printf '{version}' > '{state}'; printf '{{\\n  \"running\": true,\\n  \"mode\": \"headless\",\\n  \"version\": \"{version}\",\\n  \"pid\": 77\\n}}\\n';;\n\
                 'server ssh-pair') [ -f '{state}' ] || {{ printf '{{\"ok\": false, \"error\": {{\"message\": \"Mighty Code is not running\"}}}}\\n'; exit 1; }}; if [ -f '{root}/pairing' ]; then cat '{root}/pairing'; exit 0; fi; printf '{{\\n  \"port\": 50123,\\n  \"code\": \"{CODE}\",\\n  \"version\": \"{version}\"\\n}}\\n';;\n\
                 *) exit 9;;\n\
                 esac\n",
                state = state.display(),
                calls = calls.display(),
                root = self.root.display(),
            ),
        );
        let zip = build.join("MightyCode-macos.zip");
        #[allow(clippy::disallowed_methods, reason = "the check zips its stand-in app as the release does")]
        let zipped = std::process::Command::new("/usr/bin/ditto").args(["-c", "-k", "--keepParent"]).arg(&app).arg(&zip).status().unwrap();
        assert!(zipped.success());
        let digest = Sha256::digest(std::fs::read(&zip).unwrap()).iter().map(|b| format!("{b:02x}")).collect();
        (zip, digest)
    }

    fn plan(&self, version: &str, stop_first: bool) -> InstallPlan {
        let (package, sha256) = self.package(version);
        InstallPlan {
            package,
            sha256,
            version: version.into(),
            requirement: requirement(Some("TEAM1")),
            id: "0123456789abcdef0123456789abcdef".into(),
            stop_first,
        }
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap_or_default()
    }

    fn installed_version(&self) -> String {
        let plist = std::fs::read_to_string(self.home.join(HOME_APP).join("Contents/Info.plist")).unwrap_or_default();
        plist.split("<key>CFBundleShortVersionString</key><string>").nth(1).and_then(|r| r.split('<').next()).unwrap_or_default().into()
    }

    fn install_folders(&self) -> usize {
        std::fs::read_dir(self.home.join("Applications"))
            .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(INSTALL_FOLDER_PREFIX)).count())
            .unwrap_or(0)
    }
}

fn recorder() -> (Arc<Mutex<Vec<Progress>>>, impl Fn(Progress) + Send + Sync) {
    let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
    let into = seen.clone();
    (seen, move |p: Progress| into.lock().unwrap().push(p))
}

fn steps(seen: &Arc<Mutex<Vec<Progress>>>) -> Vec<Step> {
    seen.lock().unwrap().iter().filter_map(|p| if let Progress::Step(s) = p { Some(*s) } else { None }).collect()
}

/// Detect, install, detect again, pair, update over the running server, then remove — every step through `ssh -T` in batch mode
/// over the shared connection, each remote command one `/bin/sh -c` word.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_is_installed_paired_updated_and_removed_over_ssh() {
    let machine = Machine::new("ssh-server");
    let server = machine.server();

    let fresh = server.detect().await.unwrap();
    assert!(fresh.is_mac() && fresh.supported_arch(), "{fresh:?}");
    assert_eq!(fresh.app, None);
    assert!(!fresh.running());

    let (seen, progress) = recorder();
    let status = server.install(&machine.plan("0.6.0", false), &progress).await.unwrap();
    assert!(status.running && status.mode == "headless" && status.version == "0.6.0");
    assert_eq!(steps(&seen), [Step::Upload, Step::Unpack, Step::Swap, Step::Start], "no stop: nothing ran");
    assert!(seen.lock().unwrap().iter().any(|p| matches!(p, Progress::Upload(f) if (*f - 1.0).abs() < f64::EPSILON)));
    assert_eq!(machine.installed_version(), "0.6.0");
    assert_eq!(machine.install_folders(), 0, "the install folder is gone");
    let codesign: Vec<String> = machine.read("codesign.args").lines().map(str::to_string).collect();
    assert_eq!(&codesign[..4], ["--verify", "--deep", "--strict", "-R"]);
    assert_eq!(codesign[4], requirement(Some("TEAM1")), "the requirement reached codesign as one word");
    assert!(codesign[5].ends_with(&format!("{INSTALL_FOLDER_PREFIX}0123456789abcdef0123456789abcdef/MightyCode.app")));

    let installed = server.detect().await.unwrap();
    assert_eq!(installed.app, Some(AppPlace::Home));
    assert_eq!(installed.version.as_deref(), Some("0.6.0"));
    assert!(installed.running());

    let pairing = server.pair(AppPlace::Home).await.unwrap();
    assert_eq!(pairing.port, 50123);
    assert_eq!(pairing.code, CODE);

    let (seen, progress) = recorder();
    let status = server.install(&machine.plan("0.6.1", true), &progress).await.unwrap();
    assert_eq!(status.version, "0.6.1");
    assert_eq!(steps(&seen), [Step::Upload, Step::Unpack, Step::Stop, Step::Swap, Step::Start]);
    assert_eq!(
        machine.read("calls").lines().collect::<Vec<_>>(),
        ["start-0.6.0", "stop-0.6.0", "start-0.6.1"],
        "the old one stopped itself"
    );
    assert_eq!(machine.installed_version(), "0.6.1");

    server.remove().await.unwrap();
    assert!(!machine.home.join(HOME_APP).exists());
    assert!(machine.read("calls").ends_with("stop-0.6.1\n"), "the server was stopped before its app went");

    // Every call: batch mode, no terminal, the shared connection, `-- devbox`, one `/bin/sh -c '…'` word, and never the code.
    let calls: Vec<Vec<String>> =
        machine.read("ssh.args").split("\n\n").filter(|c| !c.trim().is_empty()).map(|c| c.lines().map(str::to_string).collect()).collect();
    assert!(calls.len() >= 10, "{}", calls.len());
    for call in &calls {
        assert!(call.windows(2).any(|w| w == ["-o", "BatchMode=yes"]), "{call:?}");
        assert!(call.contains(&"-T".to_string()) && call.windows(2).any(|w| w == ["-o", "ControlMaster=auto"]));
        let at = call.iter().position(|a| a == "--").unwrap();
        assert_eq!(call[at + 1], "devbox");
        assert!(call[at + 2].starts_with("/bin/sh -c '"), "{:?}", call[at + 2]);
    }
    assert!(!machine.read("ssh.args").contains(CODE), "the pairing code is never on a command line");
    common::remove_when_free(&machine.root).await;
}

/// A refused signature, a package that is not the one sent, a server that does not start, a lost connection: each says so, leaves
/// no install folder and never touches the installed app.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_step_leaves_the_installed_app_alone() {
    let machine = Machine::new("ssh-server-refused");
    let server = machine.server();
    let (_, progress) = recorder();
    server.install(&machine.plan("0.6.0", false), &progress).await.unwrap();

    std::fs::write(machine.root.join("bad-signature"), "").unwrap();
    let refused = server.install(&machine.plan("0.6.1", true), &progress).await.unwrap_err();
    assert_eq!(refused, ServerError::Unpack(UnpackRefusal::Signature));
    assert_eq!(machine.installed_version(), "0.6.0");
    assert_eq!(machine.install_folders(), 0);
    assert!(!machine.read("calls").contains("stop"), "nothing was stopped before the new app was verified");
    std::fs::remove_file(machine.root.join("bad-signature")).unwrap();

    let mut plan = machine.plan("0.6.1", true);
    plan.sha256 = "0".repeat(64);
    assert_eq!(server.install(&plan, &progress).await.unwrap_err(), ServerError::Digest);
    assert_eq!(machine.installed_version(), "0.6.0");
    assert_eq!(machine.install_folders(), 0);

    let mut plan = machine.plan("0.6.1", true);
    plan.version = "0.6.2".into();
    assert_eq!(server.install(&plan, &progress).await.unwrap_err(), ServerError::Unpack(UnpackRefusal::Version));

    std::fs::write(machine.root.join("no-gui"), "").unwrap();
    server.stop().await.unwrap();
    match server.start(AppPlace::Home).await.unwrap_err() {
        ServerError::Start(words) => assert!(words.contains("control API did not answer"), "{words}"),
        other => panic!("{other:?}"),
    }
    match server.pair(AppPlace::Home).await.unwrap_err() {
        ServerError::Pair(words) => assert_eq!(words, "Mighty Code is not running"),
        other => panic!("{other:?}"),
    }

    std::fs::write(machine.root.join("lost"), "").unwrap();
    assert_eq!(server.detect().await.unwrap_err(), ServerError::Ssh(Failure::Unreachable));
    common::remove_when_free(&machine.root).await;
}

/// MC-40: the far side's app restarted on another port while ssh stayed up: the reconnect's pairing over the same ssh gives the new
/// port and a new code, each only on ssh's standard output — never on a command line (the stand-in ssh records every argument).
#[tokio::test(flavor = "multi_thread")]
async fn a_server_restarted_on_another_port_is_paired_again_over_the_same_ssh() {
    let machine = Machine::new("ssh-server-repair");
    let server = machine.server();
    let (_, progress) = recorder();
    server.install(&machine.plan("0.6.0", false), &progress).await.unwrap();
    let before = server.pair(AppPlace::Home).await.unwrap();
    assert_eq!((before.port, before.code.as_str()), (50123, CODE));

    // The app started again: another port, another code (as its control API's `server.sshPair` answers).
    let renewed = "f00d-f00d-f00d-f00d-f00d-f00d-f00d-f00d-f00d";
    std::fs::write(
        machine.root.join("pairing"),
        format!("{{\n  \"port\": 50777,\n  \"code\": \"{renewed}\",\n  \"version\": \"0.6.0\"\n}}\n"),
    )
    .unwrap();
    let after = server.pair(AppPlace::Home).await.unwrap();
    assert_eq!((after.port, after.code.as_str()), (50777, renewed));

    let recorded = machine.read("ssh.args");
    assert!(!recorded.contains(CODE) && !recorded.contains(renewed), "no code is ever on a command line");
    // Never the installed app's CLI: the stand-in ssh points the far side's apps at the check's own folders, and only the stand-in
    // app writes its calls there.
    assert_eq!(machine.read("calls").lines().collect::<Vec<_>>(), ["start-0.6.0"]);
    assert!(machine.home.join(HOME_APP).exists());
    common::remove_when_free(&machine.root).await;
}
