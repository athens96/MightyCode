//! What the Mac spikes share. A spike's answer is a JSON report (`report.json` in its `--out` folder) with the pictures it judged
//! by, and its exit code: 0 when every check of its go/no-go held.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

#[cfg(target_os = "macos")]
pub mod capture;

/// `--out <folder>` (default `$TMPDIR/<name>`), created.
pub fn out_dir(name: &str) -> PathBuf {
    let mut args = std::env::args().skip(1);
    let mut out = None;
    while let Some(arg) = args.next() {
        if arg == "--out" {
            out = args.next().map(PathBuf::from);
        }
    }
    let out = out.unwrap_or_else(|| std::env::temp_dir().join(name));
    let _ = std::fs::create_dir_all(&out);
    out
}

/// The value after `flag`, if given.
pub fn arg(flag: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
    }
    None
}

/// Writes `report` as `<out>/report.json` and prints it.
pub fn write_report(out: &Path, report: &Value) {
    let text = serde_json::to_string_pretty(report).unwrap_or_default();
    let _ = std::fs::write(out.join("report.json"), &text);
    println!("{text}");
}

/// A request the loopback server answered: its path with the query, and its `Cookie` header.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub path: String,
    pub cookie: Option<String>,
}

/// An answer: status, extra header lines, body.
pub struct Answer {
    pub status: u16,
    pub headers: Vec<String>,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn html(body: &str) -> Answer {
        Answer { status: 200, headers: vec!["Content-Type: text/html; charset=utf-8".into()], body: body.as_bytes().to_vec() }
    }
}

/// Serves on 127.0.0.1 (a free port) until the process ends: every request is recorded, then `answer(path)` answers it. Returns the
/// base address (`http://127.0.0.1:<port>`) and the record.
pub fn serve(answer: impl Fn(&str) -> Answer + Send + Sync + 'static) -> std::io::Result<(String, Arc<Mutex<Vec<Seen>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let base = format!("http://{}", listener.local_addr()?);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    let answer = Arc::new(answer);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let record = record.clone();
            let answer = answer.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buffer = vec![0u8; 16 * 1024];
                let mut read = 0;
                while read < buffer.len() {
                    match stream.read(&mut buffer[read..]) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            read += n;
                            if buffer[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let text = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let mut lines = text.split("\r\n");
                let path = lines.next().and_then(|line| line.split(' ').nth(1)).unwrap_or("/").to_string();
                let mut headers = BTreeMap::new();
                for line in lines {
                    if let Some((name, value)) = line.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }
                record.lock().unwrap().push(Seen { path: path.clone(), cookie: headers.get("cookie").cloned() });
                let reply = answer(&path);
                let mut head = format!("HTTP/1.1 {} OK\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n", reply.status, reply.body.len());
                for header in &reply.headers {
                    head.push_str(header);
                    head.push_str("\r\n");
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&reply.body);
            });
        }
    });
    Ok((base, seen))
}
