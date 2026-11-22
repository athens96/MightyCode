//! The TCP ports a pane's processes listen on (MC-26): each OS reads its process list (who is whose parent) and its listening sockets
//! (which process owns which port) into a [`ProcessNet`] — the Mac from `proc_pidinfo` and `lsof`, Windows from
//! `CreateToolhelp32Snapshot` and `GetExtendedTcpTable` — and the shared rules here walk a pane's processes (its shell, agent and
//! run actions, and everything under them) to the ports they hold.

use std::collections::{BTreeSet, HashMap, HashSet};

/// One reading of the machine: every process's parent, and every listening TCP socket's owner and port.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessNet {
    pub parents: HashMap<u32, u32>,
    pub listeners: Vec<(u32, u16)>,
}

impl ProcessNet {
    /// `roots` and every process under them.
    pub fn tree(&self, roots: impl IntoIterator<Item = u32>) -> HashSet<u32> {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (&child, &parent) in &self.parents {
            // A process that names itself (Windows' System Idle Process) is no one's child.
            if child != parent {
                children.entry(parent).or_default().push(child);
            }
        }
        let mut seen = HashSet::new();
        let mut stack: Vec<u32> = roots.into_iter().filter(|&pid| pid > 0).collect();
        while let Some(pid) = stack.pop() {
            if seen.insert(pid) {
                stack.extend(children.get(&pid).into_iter().flatten().copied());
            }
        }
        seen
    }

    /// The ports `roots` and their descendants listen on, ascending, each once.
    pub fn ports(&self, roots: impl IntoIterator<Item = u32>) -> Vec<u16> {
        let tree = self.tree(roots);
        if tree.is_empty() {
            return Vec::new();
        }
        let ports: BTreeSet<u16> = self.listeners.iter().filter(|(pid, _)| tree.contains(pid)).map(|&(_, port)| port).collect();
        ports.into_iter().collect()
    }
}

/// `lsof -nP -iTCP -sTCP:LISTEN -Fpn`'s answer as (process, port) pairs: a `p<pid>` line opens a process, each `n<address>:<port>`
/// line under it is a socket (`*:3000`, `127.0.0.1:5173`, `[::1]:8080`).
pub fn parse_lsof(output: &str) -> Vec<(u32, u16)> {
    let mut found = Vec::new();
    let mut process = None;
    for line in output.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            process = pid.trim().parse::<u32>().ok();
        } else if let (Some(name), Some(pid)) = (line.strip_prefix('n'), process)
            && let Some((_, port)) = name.rsplit_once(':')
            && let Ok(port) = port.trim().parse::<u16>()
            && port > 0
        {
            found.push((pid, port));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panes_ports_are_its_processes_and_their_descendants() {
        let net = ProcessNet {
            parents: HashMap::from([(10, 1), (11, 10), (12, 11), (20, 1), (21, 20), (0, 0)]),
            listeners: vec![(12, 5173), (11, 3000), (21, 8080), (12, 5173), (30, 9000)],
        };
        assert_eq!(net.tree([10]), HashSet::from([10, 11, 12]));
        assert_eq!(net.ports([10]), [3000, 5173], "the shell's grandchild's dev server, each port once");
        assert_eq!(net.ports([20, 12]), [5173, 8080]);
        assert_eq!(net.ports([0]), Vec::<u16>::new(), "no process 0");
        assert_eq!(net.ports([99]), Vec::<u16>::new(), "a process gone");
        assert!(net.ports([30]).contains(&9000), "a root alone");
    }

    #[test]
    fn lsof_lines_are_read() {
        let output = "p412\nf23\nn*:3000\nf24\nn127.0.0.1:5173\np9\nf5\nn[::1]:8080\nnbad\nn*:0\np\nn*:1\n";
        assert_eq!(parse_lsof(output), [(412, 3000), (412, 5173), (9, 8080)]);
    }
}
