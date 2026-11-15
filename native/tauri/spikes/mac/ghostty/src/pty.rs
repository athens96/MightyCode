//! The PTY Rust owns (plan D2): `forkpty` with the window size, a shell that prints the spike's markers, a reader thread, writes and
//! `TIOCSWINSZ` resizes. The product's `MacPtyHost` (M-A) is the full port of Swift's `PTYAgentTerminalPane`; this is the least the
//! spike needs to show the bytes go round.

use std::ffi::CString;
use std::os::fd::RawFd;

pub struct Pty {
    pub master: RawFd,
    pub pid: libc::pid_t,
}

/// The shell's script: a block of pure red cells (the pixel check's colour), the parse marker, the terminal's name and grid, then an
/// interactive shell at a known prompt.
const SCRIPT: &str = "printf '\\033[48;2;255;0;0m%s\\033[0m\\n' '                                        ' \
    '                                        ' '                                        '; \
    echo \"spike-marker-$((6*7))\"; tty; stty size; PS1='spike$ '; export PS1; exec /bin/sh -i";

pub fn open(columns: u16, rows: u16) -> Result<Pty, String> {
    // Everything the child needs is made before the fork: after it only async-signal-safe calls run (execve).
    let program = CString::new("/bin/sh").unwrap();
    let args = [program.clone(), CString::new("-c").unwrap(), CString::new(SCRIPT).unwrap()];
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let env = [
        "TERM=xterm-256color".to_string(),
        "COLORTERM=truecolor".into(),
        "LANG=en_US.UTF-8".into(),
        format!("HOME={home}"),
        "PATH=/usr/bin:/bin:/usr/sbin:/sbin".into(),
    ]
    .map(|e| CString::new(e).unwrap());
    let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    let mut envp: Vec<*const libc::c_char> = env.iter().map(|e| e.as_ptr()).collect();
    envp.push(std::ptr::null());
    let mut size = libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
    let mut master: RawFd = -1;
    // SAFETY: forkpty with a valid winsize; the child calls only execve and _exit.
    let pid = unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) };
    if pid < 0 {
        return Err(format!("forkpty failed: {}", std::io::Error::last_os_error()));
    }
    if pid == 0 {
        // SAFETY: the child after fork: argv and envp were built before it.
        unsafe {
            libc::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
            libc::_exit(127);
        }
    }
    Ok(Pty { master, pid })
}

/// Writes all of `bytes` to the PTY (the user's input), retrying short writes.
pub fn write_all(master: RawFd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: a valid descriptor and buffer.
        let written = unsafe { libc::write(master, bytes.as_ptr().cast(), bytes.len()) };
        if written <= 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        bytes = &bytes[written as usize..];
    }
}

pub fn resize(master: RawFd, columns: u16, rows: u16, width: u32, height: u32) -> bool {
    let size = libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: width as u16, ws_ypixel: height as u16 };
    // SAFETY: TIOCSWINSZ with a valid winsize on the master.
    unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &size) == 0 }
}

/// Reads the PTY until it closes, handing each chunk to `output` (on this thread).
pub fn spawn_reader(master: RawFd, output: impl Fn(&[u8]) + Send + 'static) {
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            // SAFETY: a valid descriptor and buffer.
            let read = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if read <= 0 {
                return;
            }
            output(&buffer[..read as usize]);
        }
    });
}
