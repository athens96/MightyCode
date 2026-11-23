//! What went wrong, from ssh's stderr and exit code: the connection test reports each separately, and a pane's banner says which.
//! Only ssh's own well-known sentences are matched; anything else is [`Failure::Other`] with its last line.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The name does not resolve, nothing listens, the network or the connect timed out.
    Unreachable,
    /// The server refused every key (or wanted a password, which batch mode does not type).
    AuthFailed,
    /// The host's key is not in `known_hosts` yet: the person accepts it once in a terminal (`ssh <host>`), never the app.
    HostKeyUnknown,
    /// The host's key differs from the one in `known_hosts`.
    HostKeyChanged,
    /// The local `ssh` program could not be started.
    NoSsh,
    /// The test ran past its limit.
    TimedOut,
    /// Something else: ssh's last line.
    Other(String),
}

impl Failure {
    /// The locale key of the sentence that says it.
    pub fn key(&self) -> &'static str {
        match self {
            Failure::Unreachable => "ssh.failure.unreachable",
            Failure::AuthFailed => "ssh.failure.auth",
            Failure::HostKeyUnknown => "ssh.failure.hostKeyUnknown",
            Failure::HostKeyChanged => "ssh.failure.hostKeyChanged",
            Failure::NoSsh => "ssh.failure.noSsh",
            Failure::TimedOut => "ssh.failure.timedOut",
            Failure::Other(_) => "ssh.failure.other",
        }
    }

    /// A short machine word (`unreachable`, `auth`, …) for the saved state and the page.
    pub fn word(&self) -> &'static str {
        match self {
            Failure::Unreachable => "unreachable",
            Failure::AuthFailed => "auth",
            Failure::HostKeyUnknown => "hostKeyUnknown",
            Failure::HostKeyChanged => "hostKeyChanged",
            Failure::NoSsh => "noSsh",
            Failure::TimedOut => "timedOut",
            Failure::Other(_) => "other",
        }
    }
}

/// ssh's stderr read. Host-key problems first (a changed key's warning also says "verification failed").
pub fn classify(stderr: &str) -> Failure {
    let text = stderr.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
    if has(&["remote host identification has changed", "host key for", "offending"]) && has(&["changed", "offending"]) {
        return Failure::HostKeyChanged;
    }
    if has(&[
        "host key verification failed",
        "no ecdsa host key is known",
        "no ed25519 host key is known",
        "no rsa host key is known",
        "authenticity of host",
    ]) {
        return Failure::HostKeyUnknown;
    }
    if has(&["permission denied", "too many authentication failures", "authentication failed", "no supported authentication methods"]) {
        return Failure::AuthFailed;
    }
    if has(&[
        "could not resolve hostname",
        "name or service not known",
        "nodename nor servname",
        "connection refused",
        "connection timed out",
        "operation timed out",
        "no route to host",
        "network is unreachable",
        "host is down",
        "connection reset",
        "connection closed by",
        "broken pipe",
        "timeout, server",
        "connect to host",
    ]) {
        return Failure::Unreachable;
    }
    let last = stderr.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("");
    Failure::Other(last.chars().filter(|c| !c.is_control()).take(200).collect())
}

/// Whether a terminal pane's ssh ended because the connection went (ssh's 255), not because its shell exited.
pub fn lost_connection(exit_code: i32) -> bool {
    exit_code == super::command::SSH_FAILED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_sentences_are_told_apart() {
        let cases: &[(&str, Failure)] = &[
            ("ssh: Could not resolve hostname devbox: nodename nor servname provided, or not known\r\n", Failure::Unreachable),
            ("ssh: connect to host 10.0.0.9 port 22: Connection refused", Failure::Unreachable),
            ("ssh: connect to host 10.0.0.9 port 22: Operation timed out", Failure::Unreachable),
            ("ssh: connect to host box port 22: No route to host", Failure::Unreachable),
            ("Timeout, server box not responding.", Failure::Unreachable),
            ("me@box: Permission denied (publickey,password).", Failure::AuthFailed),
            ("Received disconnect from 1.2.3.4 port 22:2: Too many authentication failures", Failure::AuthFailed),
            (
                "No ED25519 host key is known for box and you have requested strict checking.\nHost key verification failed.",
                Failure::HostKeyUnknown,
            ),
            ("Host key verification failed.", Failure::HostKeyUnknown),
            (
                "@@@@@@@@@@@\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\nOffending ECDSA key in /Users/me/.ssh/known_hosts:3\nHost key verification failed.",
                Failure::HostKeyChanged,
            ),
            ("kex_exchange_identification: something odd\n", Failure::Other("kex_exchange_identification: something odd".into())),
            ("", Failure::Other(String::new())),
        ];
        for (stderr, expected) in cases {
            assert_eq!(&classify(stderr), expected, "{stderr}");
        }
        assert_eq!(Failure::AuthFailed.key(), "ssh.failure.auth");
        assert!(lost_connection(255) && !lost_connection(0) && !lost_connection(130));
    }
}
