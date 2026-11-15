//! Update addresses (C# `AppUpdateManifest.Allowed`, `AppUpdateService.RequireHttps` and the redirect's `new Uri(current, Location)`).
//!
//! C# asks .NET's `Uri`: absolute, scheme `https` (any case), a non-empty host. This port reads the address itself and is stricter
//! where .NET is lenient (it repairs `https:\\host`, `https:host`, white space around or inside): such an address is refused here.
//! Every address the release manifest, the build stamp and a typed address carry in practice is `https://host/...`, accepted by both;
//! the cases are in `update-versions.json`'s `allowed` list, C#'s verdict beside each.

/// The scheme of an absolute reference (`^[A-Za-z][A-Za-z0-9+.-]*:`), lower-cased, and what follows the colon.
fn scheme(address: &str) -> Option<(String, &str)> {
    let colon = address.find(':')?;
    let name = &address[..colon];
    let mut chars = name.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')))
        .then(|| (name.to_ascii_lowercase(), &address[colon + 1..]))
}

/// The authority of `//authority/rest` and the rest (path, query, fragment).
fn authority(after_scheme: &str) -> Option<(&str, &str)> {
    let rest = after_scheme.strip_prefix("//")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some((&rest[..end], &rest[end..]))
}

/// The host of an authority (`user@host:port`), None when the port is not a number up to 65535 or a bracket does not close.
fn host(authority: &str) -> Option<&str> {
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, after)| after);
    let (host, port) = if host_port.starts_with('[') {
        let close = host_port.find(']')?;
        let after = &host_port[close + 1..];
        (&host_port[..=close], if after.is_empty() { None } else { Some(after.strip_prefix(':')?) })
    } else {
        match host_port.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        }
    };
    if let Some(port) = port
        && !port.is_empty()
        && !(port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u32>().is_ok_and(|p| p <= 65535))
    {
        return None;
    }
    let forbidden = |c: char| matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\' | '/');
    (!host.chars().any(forbidden)).then_some(host)
}

/// C# `AppUpdateManifest.Allowed`: an absolute https address with a host.
pub fn allowed(address: &str) -> bool {
    if address.is_empty() || address.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\') {
        return false;
    }
    let Some((scheme, rest)) = scheme(address) else { return false };
    scheme == "https" && authority(rest).and_then(|(authority, _)| host(authority)).is_some_and(|host| !host.is_empty())
}

/// RFC 3986's `remove_dot_segments`.
fn remove_dots(path: &str) -> String {
    let mut output: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').collect();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *segment {
            "." => {
                if last {
                    output.push("");
                }
            }
            ".." => {
                if output.len() > 1 {
                    output.pop();
                }
                if last {
                    output.push("");
                }
            }
            other => output.push(other),
        }
    }
    output.join("/")
}

/// `new Uri(current, location)`: a redirect's `Location` read against the address that answered it (RFC 3986 §5.2), so a
/// relative one stays on the same https host.
pub fn resolve(current: &str, location: &str) -> String {
    if scheme(location).is_some() {
        return location.to_string();
    }
    let Some((base_scheme, after)) = scheme(current) else { return location.to_string() };
    let Some((base_authority, base_rest)) = authority(after) else { return location.to_string() };
    if location.starts_with("//") {
        return format!("{base_scheme}:{location}");
    }
    let base_rest = base_rest.split('#').next().unwrap_or_default();
    let base_path = base_rest.split('?').next().unwrap_or_default();
    let prefix = format!("{base_scheme}://{base_authority}");
    if location.is_empty() {
        return format!("{prefix}{base_rest}");
    }
    if location.starts_with('#') {
        return format!("{prefix}{base_rest}{location}");
    }
    if location.starts_with('?') {
        return format!("{prefix}{base_path}{location}");
    }
    let (path, tail) = location.find(['?', '#']).map_or((location, ""), |at| location.split_at(at));
    let merged = if path.starts_with('/') {
        path.to_string()
    } else {
        let directory = base_path.rfind('/').map_or("/", |at| &base_path[..=at]);
        format!("{}{path}", if directory.is_empty() { "/" } else { directory })
    };
    format!("{prefix}{}{tail}", remove_dots(&merged))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_with_a_host_is_allowed() {
        assert!(allowed("https://updates.example/latest.json"));
        assert!(allowed("HTTPS://updates.example:443/x?y#z"));
        assert!(allowed("https://user@[::1]:8443/"));
        assert!(!allowed("http://updates.example/latest.json"));
        assert!(!allowed("file:///tmp/latest.json"));
        assert!(!allowed("https:///latest.json"));
        assert!(!allowed("https://host:99999/"));
        assert!(!allowed(" https://host/"));
        assert!(!allowed("updates.example/latest.json"));
    }

    #[test]
    fn a_redirect_is_read_against_the_address_that_sent_it() {
        let base = "https://updates.example/a/b/latest.json?x=1";
        assert_eq!(resolve(base, "https://mirror.example/l.json"), "https://mirror.example/l.json");
        assert_eq!(resolve(base, "http://updates.example/l.json"), "http://updates.example/l.json");
        assert_eq!(resolve(base, "//cdn.example/l.json"), "https://cdn.example/l.json");
        assert_eq!(resolve(base, "/root.json"), "https://updates.example/root.json");
        assert_eq!(resolve(base, "next.json"), "https://updates.example/a/b/next.json");
        assert_eq!(resolve(base, "../up.json?v=2"), "https://updates.example/a/up.json?v=2");
        assert_eq!(resolve(base, "?y=2"), "https://updates.example/a/b/latest.json?y=2");
    }
}
