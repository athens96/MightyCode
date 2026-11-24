//! What the user may name as an SSH machine: a `Host` alias (from `~/.ssh/config`, or a plain host name ssh resolves itself) or the
//! plain `user@host[:port]` form. Nothing that starts with `-` (ssh would read it as an option), no space, quote, `%` or shell
//! character, no `ProxyCommand` or any other option: ssh's own config decides how to reach the host.

use std::fmt;

/// The longest name kept (a DNS name's limit).
pub const MAXIMUM_LENGTH: usize = 253;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// A `Host` alias or a host name with nothing else: everything else comes from ssh_config.
    Alias(String),
    /// `user@host[:port]` (`[v6]:port` for an IPv6 address): the user and port are passed as ssh's own arguments.
    Direct { user: Option<String>, host: String, port: Option<u16> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetError {
    Empty,
    /// Starts with `-`: ssh would read it as an option.
    Dash,
    /// A character ssh or a shell would treat specially.
    Character,
    TooLong,
    Port,
    User,
    Host,
}

impl TargetError {
    /// The locale key that says what is wrong.
    pub fn key(self) -> &'static str {
        match self {
            TargetError::Empty => "ssh.target.error.empty",
            TargetError::Dash => "ssh.target.error.dash",
            TargetError::Port => "ssh.target.error.port",
            _ => "ssh.target.error.invalid",
        }
    }
}

/// A host name or alias: letters, digits, `.`, `_` and `-`, not starting with `-` or `.`.
fn valid_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAXIMUM_LENGTH
        && !text.starts_with(['-', '.'])
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A user name: letters, digits, `.`, `_` and `-`, not starting with `-`.
fn valid_user(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && !text.starts_with('-')
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// An IPv6 address's characters (hex digits, `:`, `.` for a mapped v4, `%` is refused).
fn valid_v6(text: &str) -> bool {
    text.contains(':') && text.len() <= 45 && text.bytes().all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'))
}

fn port(text: &str) -> Result<u16, TargetError> {
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(TargetError::Port);
    }
    match text.parse::<u32>() {
        Ok(n) if (1..=65535).contains(&n) => Ok(n as u16),
        _ => Err(TargetError::Port),
    }
}

impl Target {
    /// Reads what the user typed (or an alias from ssh_config).
    pub fn parse(text: &str) -> Result<Target, TargetError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(TargetError::Empty);
        }
        if text.starts_with('-') {
            return Err(TargetError::Dash);
        }
        if text.len() > MAXIMUM_LENGTH + 80 {
            return Err(TargetError::TooLong);
        }
        if text.chars().any(|c| c.is_whitespace() || c.is_control() || !c.is_ascii()) {
            return Err(TargetError::Character);
        }
        if text.bytes().any(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'@' | b':' | b'[' | b']'))) {
            return Err(TargetError::Character);
        }
        let (user, rest) = match text.rsplit_once('@') {
            Some((user, rest)) => {
                if !valid_user(user) {
                    return Err(TargetError::User);
                }
                (Some(user.to_string()), rest)
            }
            None => (None, text),
        };
        let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
            let (address, after) = inner.split_once(']').ok_or(TargetError::Host)?;
            if !valid_v6(address) {
                return Err(TargetError::Host);
            }
            let port = match after {
                "" => None,
                _ => Some(port(after.strip_prefix(':').ok_or(TargetError::Host)?)?),
            };
            (address.to_string(), port)
        } else if rest.contains(['[', ']']) {
            return Err(TargetError::Host);
        } else if rest.matches(':').count() > 1 && valid_v6(rest) {
            (rest.to_string(), None)
        } else if let Some((host, digits)) = rest.split_once(':') {
            (host.to_string(), Some(port(digits)?))
        } else {
            (rest.to_string(), None)
        };
        if !host.contains(':') && !valid_name(&host) {
            return Err(if host.starts_with('-') { TargetError::Dash } else { TargetError::Host });
        }
        Ok(match (user, port) {
            (None, None) if !host.contains(':') => Target::Alias(host),
            (user, port) => Target::Direct { user, host, port },
        })
    }

    /// What ssh is given after `--`: the alias, or `user@host`.
    pub fn destination(&self) -> String {
        match self {
            Target::Alias(alias) => alias.clone(),
            Target::Direct { user: Some(user), host, .. } => format!("{user}@{host}"),
            Target::Direct { user: None, host, .. } => host.clone(),
        }
    }

    /// `-p <port>` when one was given.
    pub fn port_args(&self) -> Vec<String> {
        match self {
            Target::Direct { port: Some(port), .. } => vec!["-p".into(), port.to_string()],
            _ => vec![],
        }
    }

    /// MC-52: the remote authority VS Code's Remote-SSH (and the editors built on it) opens: `ssh-remote+<alias>` or
    /// `ssh-remote+user@host`; a port or an IPv6 address goes as the hex of the JSON the extension reads
    /// (`{"hostName":…,"user":…,"port":…}`), as its own "Connect to Host" names such a host. Only the characters [`Target::parse`]
    /// takes ever reach it, so nothing needs escaping in a URI.
    pub fn vscode_authority(&self) -> String {
        match self {
            Target::Alias(alias) => format!("ssh-remote+{alias}"),
            Target::Direct { user, host, port: None } if !host.contains(':') => match user {
                Some(user) => format!("ssh-remote+{user}@{host}"),
                None => format!("ssh-remote+{host}"),
            },
            Target::Direct { user, host, port } => {
                let mut json = format!(r#"{{"hostName":"{host}""#);
                if let Some(user) = user {
                    json.push_str(&format!(r#","user":"{user}""#));
                }
                if let Some(port) = port {
                    json.push_str(&format!(r#","port":{port}"#));
                }
                json.push('}');
                let hex: String = json.bytes().map(|b| format!("{b:02x}")).collect();
                format!("ssh-remote+{hex}")
            }
        }
    }

    /// The host part alone (the sidebar's short name).
    pub fn host(&self) -> &str {
        match self {
            Target::Alias(alias) => alias,
            Target::Direct { host, .. } => host,
        }
    }
}

/// As the user typed it, normalized: `alias`, `user@host`, `user@host:2222`, `[::1]:2222`.
impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Alias(alias) => f.write_str(alias),
            Target::Direct { user, host, port } => {
                if let Some(user) = user {
                    write!(f, "{user}@")?;
                }
                match (host.contains(':'), port) {
                    (true, Some(port)) => write!(f, "[{host}]:{port}"),
                    (false, Some(port)) => write!(f, "{host}:{port}"),
                    (_, None) => f.write_str(host),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_and_plain_forms_are_read() {
        assert_eq!(Target::parse("devbox").unwrap(), Target::Alias("devbox".into()));
        assert_eq!(Target::parse(" build.example.com ").unwrap(), Target::Alias("build.example.com".into()));
        assert_eq!(
            Target::parse("me@build.example.com:2222").unwrap(),
            Target::Direct { user: Some("me".into()), host: "build.example.com".into(), port: Some(2222) }
        );
        assert_eq!(Target::parse("me@box").unwrap(), Target::Direct { user: Some("me".into()), host: "box".into(), port: None });
        assert_eq!(Target::parse("box:22").unwrap(), Target::Direct { user: None, host: "box".into(), port: Some(22) });
        assert_eq!(Target::parse("[fd00::1]:2200").unwrap(), Target::Direct { user: None, host: "fd00::1".into(), port: Some(2200) });
        assert_eq!(Target::parse("ops@fd00::1").unwrap(), Target::Direct { user: Some("ops".into()), host: "fd00::1".into(), port: None });
        for text in ["devbox", "me@box", "me@box:2222", "[fd00::1]:2200", "box:22"] {
            assert_eq!(Target::parse(text).unwrap().to_string(), text);
        }
    }

    #[test]
    fn vscode_names_the_machine_as_its_remote_ssh_does() {
        assert_eq!(Target::parse("devbox").unwrap().vscode_authority(), "ssh-remote+devbox");
        assert_eq!(Target::parse("me@box").unwrap().vscode_authority(), "ssh-remote+me@box");
        let hex = Target::parse("me@box:2222").unwrap().vscode_authority();
        let digits = hex.strip_prefix("ssh-remote+").unwrap();
        let bytes: Vec<u8> = (0..digits.len()).step_by(2).map(|i| u8::from_str_radix(&digits[i..i + 2], 16).unwrap()).collect();
        assert_eq!(String::from_utf8(bytes).unwrap(), r#"{"hostName":"box","user":"me","port":2222}"#);
        assert!(Target::parse("[fd00::1]:2200").unwrap().vscode_authority().starts_with("ssh-remote+7b22"));
    }

    #[test]
    fn options_and_shell_characters_are_refused() {
        assert_eq!(Target::parse(""), Err(TargetError::Empty));
        assert_eq!(Target::parse("-oProxyCommand=sh"), Err(TargetError::Dash));
        assert_eq!(Target::parse("-p22"), Err(TargetError::Dash));
        assert_eq!(Target::parse("me@-oProxyCommand"), Err(TargetError::Dash));
        assert_eq!(Target::parse("-me@box"), Err(TargetError::Dash));
        for bad in [
            "box -oProxyCommand=x",
            "box;rm",
            "box|x",
            "$(id)",
            "`id`",
            "box'",
            "box\"",
            "a%h",
            "a/b",
            "box=1",
            "bo\nx",
            "host,other",
            "ssh://box",
            "호스트",
        ] {
            assert!(Target::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(Target::parse("box:0"), Err(TargetError::Port));
        assert_eq!(Target::parse("box:65536"), Err(TargetError::Port));
        assert_eq!(Target::parse("box:x"), Err(TargetError::Port));
        assert!(Target::parse(".hidden").is_err());
        assert!(Target::parse("@box").is_err());
        assert!(Target::parse(&"a".repeat(400)).is_err());
    }

    #[test]
    fn destination_and_port_go_to_ssh_separately() {
        let target = Target::parse("me@box:2222").unwrap();
        assert_eq!(target.destination(), "me@box");
        assert_eq!(target.port_args(), ["-p", "2222"]);
        assert!(Target::parse("devbox").unwrap().port_args().is_empty());
        assert_eq!(TargetError::Dash.key(), "ssh.target.error.dash");
    }
}
