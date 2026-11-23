//! MC-3: the exe's daemon mode, `MightyCode --daemon [--headless] [--profile <absolute folder>]`: no window, no webview, no state.
//! It takes the profile's lock (a second daemon of the profile exits at once, 0), writes its child groups down for the next launch should
//! it crash (the Mac), binds its own socket or pipe, writes its endpoint and serves (mighty-run `daemon::server`) until it is told to
//! stop, a signal comes, or (unless `--headless`) it has had nothing running and nobody attached for ten minutes. As it ends it ends
//! every process it owns.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use mighty_core::daemon::protocol::PROTOCOL;
use mighty_run::daemon::server::{Daemon, Options};

use super::files::{self, DaemonLog, Endpoint};

/// The exe's argument that starts the daemon.
pub const ARGUMENT: &str = "--daemon";
pub const HEADLESS: &str = "--headless";

/// The daemon mode's arguments (after `--daemon`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Arguments {
    pub headless: bool,
    pub profile: Option<PathBuf>,
}

pub fn parse(args: &[String]) -> Result<Arguments, String> {
    let mut parsed = Arguments::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            HEADLESS => parsed.headless = true,
            "--profile" if parsed.profile.is_none() => {
                let path = PathBuf::from(args.next().ok_or("--profile needs a path.")?);
                if !path.is_absolute() {
                    return Err("--profile needs an absolute path.".into());
                }
                parsed.profile = Some(path);
            }
            other => return Err(format!("Unsupported argument: {other}")),
        }
    }
    Ok(parsed)
}

/// The exe mode: serves until the daemon stops; the process's exit code.
pub fn run(args: &[String]) -> i32 {
    let arguments = match parse(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "mighty daemon: {error}");
            return 1;
        }
    };
    let data_dir = match crate::platform::data_dir(arguments.profile.clone()) {
        Ok(dir) => dir,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "mighty daemon: {error}");
            return 1;
        }
    };
    // The folder (0700) and the token first: the lock and the log live in it.
    let token = match files::token_or_new(&data_dir) {
        Ok(token) => token,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "mighty daemon: the daemon folder could not be written: {error}");
            return 1;
        }
    };
    let _lock = match crate::platform::daemon_lock(&files::folder(&data_dir).join(files::LOCK_FILE)) {
        Ok(lock) => lock,
        Err(_) => {
            // Another daemon serves this profile: nothing to do.
            let _ = writeln!(std::io::stderr(), "mighty daemon: already running for this profile");
            return 0;
        }
    };
    let log = Arc::new(DaemonLog::open(&data_dir));
    log.line(&format!(
        "daemon {} starting (protocol {PROTOCOL}{})",
        env!("CARGO_PKG_VERSION"),
        if arguments.headless { ", headless" } else { "" }
    ));
    // What an earlier daemon of the profile left running when it crashed is ended, and this daemon's children written down (the Mac).
    crate::platform::record_daemon_groups(&files::folder(&data_dir).join(files::GROUPS_FOLDER));
    let (processes, ptys) = crate::platform::daemon_ports();
    let mut options = Options::new(env!("CARGO_PKG_VERSION"), &token);
    if arguments.headless {
        options.idle = None;
    }
    let writer = log.clone();
    let daemon = Daemon::new(processes, ptys, options, Arc::new(move |line: &str| writer.line(line)));
    tauri::async_runtime::block_on(async move {
        let listener = crate::platform::daemon_listener(&data_dir);
        let address = match listener.bind() {
            Ok(address) => address,
            Err(error) => {
                log.line(&format!("daemon: not listening: {error}"));
                return 1;
            }
        };
        let endpoint = Endpoint {
            transport: if cfg!(windows) { "pipe" } else { "unix" }.into(),
            address: address.clone(),
            pid: std::process::id(),
            app: env!("CARGO_PKG_VERSION").into(),
            protocol: PROTOCOL,
        };
        if let Err(error) = files::write_endpoint(&data_dir, &endpoint) {
            log.line(&format!("daemon: the endpoint could not be written: {error}"));
            listener.close();
            return 1;
        }
        log.line("daemon: listening");
        let stopper = daemon.clone();
        tokio::spawn(async move {
            stopped_by_signal().await;
            stopper.stop();
        });
        daemon.serve(listener).await;
        files::remove_endpoint(&data_dir, &address);
        0
    })
}

/// Ends when the OS asks the process to end (Ctrl+C; on the Mac SIGTERM and SIGHUP too), so the daemon ends its children first.
async fn stopped_by_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut terminate), Ok(mut hang_up)) = (signal(SignalKind::terminate()), signal(SignalKind::hangup())) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = terminate.recv() => {}
            _ = hang_up.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daemon_mode_reads_its_arguments() {
        let read = |args: &[&str]| parse(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        assert_eq!(read(&[]).unwrap(), Arguments::default());
        let folder = if cfg!(windows) { r"C:\p" } else { "/tmp/p" };
        let both = read(&["--headless", "--profile", folder]).unwrap();
        assert_eq!(both, Arguments { headless: true, profile: Some(PathBuf::from(folder)) });
        assert!(read(&["--profile", "relative"]).is_err());
        assert!(read(&["--profile"]).is_err());
        assert!(read(&["--smoke-test"]).is_err());
    }
}
