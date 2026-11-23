//! MC-34: the network allow list: which `host:port` a sandboxed run may reach through the proxy.
//!
//! An entry is `host` (port 443), `host:port`, `*.host` (any name under it, not the name itself; port 443) or `*.host:port`. IP
//! literals are entries of their own (`140.82.112.3:443`, `[::1]:8080`): an allowed name never lets its address through, so a CONNECT
//! to a listed name's IP is asked about like any other. Names are compared lowercased without a final dot.

use std::net::IpAddr;

/// Where a run connects: a host name or an IP literal (without brackets) and a port.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Target {
    pub host: String,
    pub port: u16,
}

impl Target {
    /// `host:port`, `[v6]:port`; `default_port` when no port is given.
    pub fn parse(text: &str, default_port: u16) -> Option<Target> {
        let text = text.trim();
        if text.is_empty() || text.len() > 300 || text.chars().any(|c| c.is_whitespace() || c.is_control() || c == '/' || c == '@') {
            return None;
        }
        let (host, port) = if let Some(rest) = text.strip_prefix('[') {
            let (inside, after) = rest.split_once(']')?;
            let port = match after.strip_prefix(':') {
                Some(p) => p.parse().ok()?,
                None if after.is_empty() => default_port,
                None => return None,
            };
            inside.parse::<std::net::Ipv6Addr>().ok()?;
            (inside.to_string(), port)
        } else {
            match text.rsplit_once(':') {
                Some((host, port)) if !host.contains(':') => (host.to_string(), port.parse().ok()?),
                Some(_) => return None,
                None => (text.to_string(), default_port),
            }
        };
        let host = normalize_host(&host)?;
        (port != 0).then_some(Target { host, port })
    }

    /// The target's IP when its host is a literal.
    pub fn ip(&self) -> Option<IpAddr> {
        self.host.parse().ok()
    }

    /// Whether it names this computer (a loopback address or `localhost`).
    pub fn loopback(&self) -> bool {
        self.host == "localhost" || self.host.ends_with(".localhost") || self.ip().is_some_and(|ip| ip.is_loopback())
    }

    /// `host:port` as the card shows it.
    pub fn label(&self) -> String {
        if self.host.contains(':') { format!("[{}]:{}", self.host, self.port) } else { format!("{}:{}", self.host, self.port) }
    }
}

/// A host name lowercased without its final dot; None for one that is not a name or an IP.
pub fn normalize_host(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 {
        return None;
    }
    if host.parse::<IpAddr>().is_ok() {
        return Some(host);
    }
    let label_ok = |l: &str| !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    host.split('.').all(label_ok).then_some(host)
}

/// One allow-list entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRule {
    /// The name (without `*.`) or IP.
    pub host: String,
    /// `*.host`: names under it.
    pub wildcard: bool,
    pub port: u16,
}

impl HostRule {
    /// An entry as typed; None for one that is not an entry.
    pub fn parse(text: &str) -> Option<HostRule> {
        let text = text.trim();
        let (wildcard, rest) = match text.strip_prefix("*.") {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let target = Target::parse(rest, 443)?;
        if wildcard && (target.ip().is_some() || !target.host.contains('.')) {
            return None;
        }
        Some(HostRule { host: target.host, wildcard, port: target.port })
    }

    pub fn matches(&self, target: &Target) -> bool {
        if target.port != self.port {
            return false;
        }
        if self.wildcard {
            target.ip().is_none() && target.host.strip_suffix(&self.host).is_some_and(|head| head.ends_with('.') && head.len() > 1)
        } else {
            target.host == self.host
        }
    }

    /// The entry as it is saved and shown (`github.com`, `example.com:8080`, `*.npmjs.org`).
    pub fn text(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let host = if self.wildcard { format!("*.{host}") } else { host };
        if self.port == 443 && !self.host.contains(':') { host } else { format!("{host}:{}", self.port) }
    }
}

/// The hosts each agent's own service needs (its API, its sign-in refresh, its model list). Not proven by a paid run; the card asks
/// for anything else.
pub fn provider_hosts(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &["api.anthropic.com", "claude.ai", "console.anthropic.com", "platform.claude.com", "statsig.anthropic.com"],
        "codex" => &["api.openai.com", "chatgpt.com", "auth.openai.com", "ab.chatgpt.com"],
        "gemini" => &[
            "cloudcode-pa.googleapis.com",
            "daily-cloudcode-pa.googleapis.com",
            "oauth2.googleapis.com",
            "accounts.google.com",
            "generativelanguage.googleapis.com",
            "antigravity.google.com",
        ],
        "opencode" => &["models.dev", "opencode.ai", "api.anthropic.com", "api.openai.com", "openrouter.ai"],
        "pi" => &["api.anthropic.com", "api.openai.com", "chatgpt.com", "generativelanguage.googleapis.com", "openrouter.ai"],
        "hermes" => &["openrouter.ai", "api.openai.com", "api.anthropic.com", "inference-api.nousresearch.com", "portal.nousresearch.com"],
        _ => &[],
    }
}

/// Package registries and code hosts a reviewer may let through on its own (its safe category for connections).
pub const SAFE_HOSTS: [&str; 14] = [
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "index.crates.io",
    "static.crates.io",
    "proxy.golang.org",
    "sum.golang.org",
    "rubygems.org",
    "repo.maven.apache.org",
    "github.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
];

/// Whether `target` is in the reviewer's safe category: a listed registry on 443.
pub fn safe_host(target: &Target) -> bool {
    target.port == 443 && target.ip().is_none() && SAFE_HOSTS.contains(&target.host.as_str())
}

/// The allow list of one run: the agent's hosts and the person's entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllowList {
    pub rules: Vec<HostRule>,
}

impl AllowList {
    pub fn new(provider: &str, entries: &[String]) -> AllowList {
        let mut list = AllowList::default();
        for entry in provider_hosts(provider).iter().map(|h| h.to_string()).chain(entries.iter().cloned()) {
            list.add(&entry);
        }
        list
    }

    /// Adds an entry (nothing for one that is not an entry or is there already). Whether it is one.
    pub fn add(&mut self, entry: &str) -> bool {
        let Some(rule) = HostRule::parse(entry) else { return false };
        if !self.rules.contains(&rule) {
            self.rules.push(rule);
        }
        true
    }

    pub fn allows(&self, target: &Target) -> bool {
        self.rules.iter().any(|r| r.matches(target))
    }

    /// The loopback ports listed (a run reaches them directly, not through the proxy).
    pub fn loopback_ports(&self) -> Vec<u16> {
        self.rules.iter().filter(|r| !r.wildcard && Target { host: r.host.clone(), port: r.port }.loopback()).map(|r| r.port).collect()
    }
}

/// Whether an address a listed *name* resolved to may be reached: never this computer, a link-local address (the cloud metadata
/// service) or an unspecified or multicast one. (An IP literal listed on its own is the person's choice and is not asked this.)
pub fn reachable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !(v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast()),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return reachable(&IpAddr::V4(v4));
            }
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> Target {
        Target::parse(text, 443).unwrap()
    }

    #[test]
    fn targets_parse_names_ports_and_literals() {
        assert_eq!(t("GitHub.com."), Target { host: "github.com".into(), port: 443 });
        assert_eq!(t("example.com:8080").port, 8080);
        assert_eq!(t("[::1]:3000"), Target { host: "::1".into(), port: 3000 });
        assert_eq!(t("[::1]:3000").label(), "[::1]:3000");
        for bad in ["", "a b", "user@host", "host:0", "host:99999", "::1:80", "exa/mple.com", "[::1", "bad_label!.com"] {
            assert!(Target::parse(bad, 443).is_none(), "{bad}");
        }
        assert!(t("127.0.0.1:80").loopback() && t("localhost:3000").loopback() && !t("github.com").loopback());
    }

    #[test]
    fn rules_match_exactly_their_name_and_port() {
        let list = AllowList::new("", &["github.com".into(), "*.npmjs.org".into(), "example.com:8080".into(), "nonsense entry".into()]);
        assert!(list.allows(&t("github.com:443")));
        assert!(!list.allows(&t("github.com:22")), "another port is another target");
        assert!(!list.allows(&t("gist.github.com")), "a name does not cover the names under it");
        assert!(!list.allows(&t("evilgithub.com")));
        assert!(list.allows(&t("registry.npmjs.org")) && !list.allows(&t("npmjs.org")), "a wildcard covers only names under it");
        assert!(!list.allows(&t("x.evilnpmjs.org")));
        assert!(list.allows(&t("example.com:8080")) && !list.allows(&t("example.com")));
        assert!(!list.allows(&t("140.82.112.3")), "an allowed name's address is not allowed");
        assert_eq!(list.rules.len(), 3);
        assert_eq!(list.rules.iter().map(HostRule::text).collect::<Vec<_>>(), ["github.com", "*.npmjs.org", "example.com:8080"]);
        assert!(
            HostRule::parse("*.1.2.3.4").is_none()
                && HostRule::parse("*.com").is_none()
                && HostRule::parse("*.example.com").is_some()
                && HostRule::parse("*.localhost").is_none()
        );
    }

    #[test]
    fn each_agent_reaches_its_own_service() {
        assert!(AllowList::new("claude", &[]).allows(&t("api.anthropic.com")));
        assert!(!AllowList::new("claude", &[]).allows(&t("api.openai.com")));
        assert!(AllowList::new("codex", &[]).allows(&t("api.openai.com")));
        for provider in ["claude", "codex", "gemini", "opencode", "pi", "hermes"] {
            assert!(!provider_hosts(provider).is_empty(), "{provider}");
            assert!(provider_hosts(provider).iter().all(|h| HostRule::parse(h).is_some()));
        }
        let list = AllowList::new("claude", &["localhost:3000".into(), "127.0.0.1:5432".into()]);
        assert_eq!(list.loopback_ports(), [3000, 5432]);
    }

    #[test]
    fn a_listed_name_never_reaches_this_computer_or_the_metadata_service() {
        for ip in ["127.0.0.1", "::1", "169.254.169.254", "0.0.0.0", "fe80::1", "::ffff:127.0.0.1", "224.0.0.1"] {
            assert!(!reachable(&ip.parse().unwrap()), "{ip}");
        }
        for ip in ["140.82.112.3", "10.0.0.5", "2606:4700::1111"] {
            assert!(reachable(&ip.parse().unwrap()), "{ip}");
        }
        assert!(safe_host(&t("registry.npmjs.org")) && !safe_host(&t("registry.npmjs.org:80")) && !safe_host(&t("evil.example")));
    }
}
