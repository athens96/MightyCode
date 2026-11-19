//! The data folder's `remote-connections.json` in the Mac's shape (C# `RemoteConnectionFile`, `RemoteSavedConnection`,
//! R\RemoteConnectionFile.cs; M/Remote/RemoteClientService.swift `SavedFile`, version 2; plan §1.6): `{"version":2,"connections":[{id,
//! name,address}]}`, at most 256 KiB, never a key (that is in the vault). A file of another version (the remote workspaces removed on
//! 2026-09-28 wrote version 1, with keys), a damaged or oversize one reads as no connections; an entry with a bad or repeated id, or an
//! address that does not parse or repeats an earlier entry's, is skipped alone. A connection paired through the relay is saved under its
//! `relay:` address (`paired::relay_address`); an older app skips it alone. Written atomically, only where the app may write the
//! profile (`--allow-state-write`; else the connections live in memory for the launch). Owner: W8-D.

use std::io;
use std::path::Path;

use mighty_core::json::JsonOut;
use mighty_core::wire;
use serde_json::Value;

use crate::remote::text;
use crate::remote::wire::protocol;

pub const FILE_NAME: &str = "remote-connections.json";
pub const MAXIMUM_FILE_BYTES: u64 = 256 * 1024;
pub const MAXIMUM_NAME: usize = 120;
pub const MAXIMUM_ADDRESS: usize = 300;

/// C# `RemoteSavedConnection(Id, Name, Address)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedConnection {
    pub id: String,
    pub name: String,
    pub address: String,
}

impl SavedConnection {
    pub fn new(id: &str, name: &str, address: &str) -> SavedConnection {
        SavedConnection { id: id.into(), name: name.into(), address: address.into() }
    }
}

/// `Read(directory)`: the saved connections; none when the file cannot be read now.
pub fn read(directory: &Path) -> Vec<SavedConnection> {
    try_read(directory).unwrap_or_default()
}

/// `TryRead(directory, out connections, out failure)`: Err when the file is there but cannot be read now (held open by another program,
/// as an antivirus scan does, or refused), so the caller reads again later and never writes over connections it has not read. No file,
/// or a damaged, oversize or other-version one, reads as none.
pub fn try_read(directory: &Path) -> Result<Vec<SavedConnection>, io::Error> {
    let path = directory.join(FILE_NAME);
    let size = match std::fs::metadata(&path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    if size > MAXIMUM_FILE_BYTES {
        return Ok(vec![]);
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    if bytes.len() as u64 > MAXIMUM_FILE_BYTES {
        return Ok(vec![]);
    }
    // Read as Wire.Json reads the record: member names in any case, a null or missing list as no file.
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { return Ok(vec![]) };
    let Some(file) = value.as_object() else { return Ok(vec![]) };
    let member = |name: &str| file.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v);
    if member("version").and_then(Value::as_i64) != Some(i64::from(protocol::VERSION))
        || member("version").is_some_and(|v| !v.is_i64() && !v.is_u64())
    {
        return Ok(vec![]);
    }
    let Some(Value::Array(saved)) = member("connections") else { return Ok(vec![]) };
    let mut entries = vec![];
    // As the Mac reads the file: its first 16 entries, then each is checked; the write caps what it keeps instead.
    for item in saved.iter().take(protocol::MAXIMUM_CONNECTIONS) {
        let Some(o) = item.as_object() else {
            if item.is_null() {
                entries.push(None);
                continue;
            }
            return Ok(vec![]);
        };
        let text_member = |name: &str| match o.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(()),
        };
        let (Ok(id), Ok(name), Ok(address)) = (text_member("id"), text_member("name"), text_member("address")) else { return Ok(vec![]) };
        entries.push(Some((id, name, address)));
    }
    Ok(usable(entries.into_iter().flatten()))
}

/// `WriteAsync(directory, connections)`: what `read` would keep, in order, so the file never holds an entry the next launch drops.
pub fn write(directory: &Path, connections: &[SavedConnection]) -> io::Result<()> {
    let kept = usable(connections.iter().map(|c| (Some(c.id.clone()), Some(c.name.clone()), Some(c.address.clone()))));
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_int("version", i64::from(protocol::VERSION));
    out.name("connections");
    out.begin_array();
    for connection in &kept {
        out.begin_object();
        out.prop_str("id", Some(&connection.id));
        out.prop_str("name", Some(&connection.name));
        out.prop_str("address", Some(&connection.address));
        out.end_object();
    }
    out.end_array();
    out.end_object();
    mighty_core::store::atomic_write(&directory.join(FILE_NAME), out.finish().as_bytes(), None, mighty_core::store::portable_replace)
}

/// `Usable(connections)`: as the Mac's `loadSaved`: an entry is kept when its id is good and new and its address parses to one no earlier
/// entry has; the address in its shown form. At most 16 kept, in order.
fn usable(connections: impl Iterator<Item = (Option<String>, Option<String>, Option<String>)>) -> Vec<SavedConnection> {
    let mut kept: Vec<SavedConnection> = vec![];
    for (id, name, address) in connections {
        if kept.len() == protocol::MAXIMUM_CONNECTIONS {
            break;
        }
        let Some(id) = id.filter(|id| wire::identifier(id) && !kept.iter().any(|k| &k.id == id)) else { continue };
        let Some(address) = address.filter(|a| !a.is_empty() && a.encode_utf16().count() <= MAXIMUM_ADDRESS) else { continue };
        let Some(display) = crate::remote::paired::display_address(&address) else { continue };
        if kept.iter().any(|k| k.address == display) {
            continue;
        }
        kept.push(SavedConnection { id, name: name_of(name.as_deref()), address: display });
    }
    kept
}

/// `Name(value)`: a connection's name as it is kept: control characters out, then at most 120 characters as the Mac counts them. Connect
/// names a connection by this too, so it reads the same after a restart.
pub fn name_of(value: Option<&str>) -> String {
    text::cut(&wire::clean(value.unwrap_or(""), usize::MAX), MAXIMUM_NAME)
}
