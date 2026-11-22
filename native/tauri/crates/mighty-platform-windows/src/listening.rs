//! MC-26: the machine's processes and listening TCP sockets for a pane's ports ([`mighty_platform::listening`]): each process's parent
//! from `CreateToolhelp32Snapshot`, the listening sockets (IPv4 and IPv6) and their owners from `GetExtendedTcpTable`
//! (`TCP_TABLE_OWNER_PID_LISTENER`). Blocking but quick: call it off the async runtime.

use std::collections::HashMap;

use mighty_platform::listening::ProcessNet;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, INVALID_HANDLE_VALUE, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

/// Every process's parent and every listening TCP socket; None when the process list cannot be read.
pub fn snapshot() -> Option<ProcessNet> {
    let parents = parents()?;
    let mut listeners = table(AF_INET as u32, |row: &MIB_TCPROW_OWNER_PID| (row.dwOwningPid, port(row.dwLocalPort)));
    listeners.extend(table(AF_INET6 as u32, |row: &MIB_TCP6ROW_OWNER_PID| (row.dwOwningPid, port(row.dwLocalPort))));
    listeners.retain(|&(_, port)| port > 0);
    Some(ProcessNet { parents, listeners })
}

/// The table's port: the low 16 bits, in network byte order.
fn port(raw: u32) -> u16 {
    u16::from_be((raw & 0xffff) as u16)
}

struct Snapshot(HANDLE);

impl Drop for Snapshot {
    fn drop(&mut self) {
        // SAFETY: a handle CreateToolhelp32Snapshot gave, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn parents() -> Option<HashMap<u32, u32>> {
    // SAFETY: a snapshot of every process (the id is ignored for TH32CS_SNAPPROCESS).
    let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let snapshot = Snapshot(handle);
    let mut parents = HashMap::new();
    // SAFETY: a zeroed plain-data struct whose size is set before use.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: the snapshot and an entry with its size set.
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while more {
        parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    Some(parents)
}

/// One family's listening table, each row read by `read`: `{ DWORD dwNumEntries; ROW table[]; }`, asked for again while the
/// buffer is too small (a socket opened meanwhile).
fn table<Row>(family: u32, read: impl Fn(&Row) -> (u32, u16)) -> Vec<(u32, u16)> {
    let mut size = 0u32;
    for _ in 0..4 {
        // u64 words: the rows' alignment.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
        // SAFETY: a buffer of `size` bytes at least (rounded up to whole words).
        let result = unsafe { GetExtendedTcpTable(buffer.as_mut_ptr().cast(), &mut size, 0, family, TCP_TABLE_OWNER_PID_LISTENER, 0) };
        if result == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if result != NO_ERROR {
            return Vec::new();
        }
        let bytes = buffer.len() * 8;
        let base = buffer.as_ptr().cast::<u8>();
        // SAFETY: the table's count leads the buffer.
        let count = unsafe { base.cast::<u32>().read_unaligned() } as usize;
        // The rows follow the count, at their own alignment.
        let offset = size_of::<u32>().next_multiple_of(align_of::<Row>());
        let count = count.min(bytes.saturating_sub(offset) / size_of::<Row>());
        return (0..count)
            .map(|index| {
                // SAFETY: `index` is inside the rows the buffer holds (bounded above).
                let row = unsafe { base.add(offset + index * size_of::<Row>()).cast::<Row>().read_unaligned() };
                read(&row)
            })
            .collect();
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listener this test opens is found under this process.
    #[test]
    fn a_listening_socket_of_this_process_is_found() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let net = snapshot().expect("the process list");
        assert!(net.parents.contains_key(&std::process::id()));
        assert!(net.ports([std::process::id()]).contains(&port));
    }
}
