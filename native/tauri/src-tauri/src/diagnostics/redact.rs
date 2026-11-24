//! MC-54: what a diagnostics bundle and a copied turn audit never carry. Every text that goes into either passes [`Redactor::redact`],
//! which takes out, in this order:
//!
//! 1. `homePath`: this user's home folder (as written, and with the other slash on Windows) becomes `~`;
//! 2. `userPath`: any other `/Users/<name>`, `/home/<name>` or `C:\Users\<name>` becomes `~` (the user name is left out);
//! 3. `authorizationHeader`: the value of an `Authorization`, `Proxy-Authorization`, `X-Api-Key`, `Cookie` or `Set-Cookie` header or
//!    field;
//! 4. `urlCredentials`: the `user:password@` of a `scheme://user:password@host` address;
//! 5. `secretTokens`: Core's secret guard (`mighty_core::shared_memory::sanitize_secrets`): PEM private keys, `*_TOKEN=`-style
//!    assignments, `Bearer` values and the provider key prefixes the commit sheet's scan knows (AWS, GitHub, GitLab, Slack, Anthropic,
//!    OpenAI, Google, Stripe);
//! 6. `secretFields`: the value of a field, assignment or flag whose name ends in password, secret, token, api key, access key, private
//!    key, client secret, pairing key or code, session key or signature (`"apiKey": "…"`, `password=…`, `?token=…`, `--token …`);
//! 7. `longHex`: a run of 48 or more hex digits (the control API's and the daemon's tokens, key hashes); a 40-digit commit stays;
//! 8. `email`: an e-mail address (an account's sign-in name);
//! 9. `commitScan`: a last pass with the Git workbench's own pre-commit patterns (`git_workbench::scan_text_secrets`): a line that
//!    still matches one is replaced whole by `[redacted: kind]`.
//!
//! Each rule counts what it took out, so the bundle's manifest can say how many values each file lost.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::{Captures, Regex};

pub const REDACTED: &str = "[REDACTED]";

/// The rules in the order they run, by the names the manifest uses.
pub const RULES: [&str; 9] =
    ["homePath", "userPath", "authorizationHeader", "urlCredentials", "secretTokens", "secretFields", "longHex", "email", "commitScan"];

static USER_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(^|[\s"'=(\[,<>])((?:[A-Za-z]:)?[\\/](?:[Uu]sers|home)[\\/][^\\/\s"':;,<>|]+)"#).expect("valid user path regex")
});
static AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b((?:proxy-)?authorization|x-api-key|set-cookie|cookie)("?\s*[:=]\s*"?)([^\r\n",}]+)"#)
        .expect("valid authorization regex")
});
static URL_CREDENTIALS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)[^/@\s]+@").expect("valid url credentials regex"));
static SECRET_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?i)(["']?)\b([A-Za-z0-9_.-]*?(?:password|passwd|passphrase|secret|token|api[_-]?key|apikey|access[_-]?key|private[_-]?key"#,
        r#"|pairing[_-]?key|pairingkey|pairing[_-]?code|pairingcode|client[_-]?secret|session[_-]?key|signature))(["']?)(\s*[:=]\s*)"#,
        r#"("(?:[^"\\\r\n]|\\.)*"|'[^'\r\n]*'|[^\s,&;}\]"']+)"#
    ))
    .expect("valid secret field regex")
});
static SECRET_FLAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(--[a-z0-9-]*?(?:password|passwd|secret|token|api-key|apikey|access-key|private-key|pairing-key))(\s+|=)([^\s-][^\s]*)",
    )
    .expect("valid secret flag regex")
});
static LONG_HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9a-fA-F]{48,}\b").expect("valid long hex regex"));
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}\b").expect("valid email regex"));

/// How many values each rule took out of one text (or a whole bundle), by rule name; a rule that took nothing is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally(pub BTreeMap<&'static str, usize>);

impl Tally {
    fn add(&mut self, rule: &'static str, count: usize) {
        if count > 0 {
            *self.0.entry(rule).or_default() += count;
        }
    }

    pub fn merge(&mut self, other: &Tally) {
        for (rule, count) in &other.0 {
            self.add(rule, *count);
        }
    }

    pub fn total(&self) -> usize {
        self.0.values().sum()
    }

    #[cfg(test)]
    pub fn get(&self, rule: &str) -> usize {
        self.0.get(rule).copied().unwrap_or(0)
    }
}

/// The redactor of one bundle: this user's home folder, which becomes `~`.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    homes: Vec<String>,
}

impl Redactor {
    /// `home` as written and, on a Windows path, with forward slashes too (a log may write either); a home of a single character or
    /// none (a root) replaces nothing.
    pub fn new(home: Option<&str>) -> Redactor {
        let mut homes = Vec::new();
        if let Some(home) = home.map(|h| h.trim_end_matches(['/', '\\'])).filter(|h| h.chars().count() > 1) {
            homes.push(home.to_string());
            if home.contains('\\') {
                homes.push(home.replace('\\', "/"));
            }
        }
        Redactor { homes }
    }

    /// The machine's: the home folder of the user running the app.
    pub fn for_this_user() -> Redactor {
        let home = std::env::var("HOME").ok().or_else(|| std::env::var("USERPROFILE").ok());
        Redactor::new(home.as_deref())
    }

    /// `text` with every rule applied, and what each took out.
    pub fn redact(&self, text: &str) -> (String, Tally) {
        let mut tally = Tally::default();
        let mut out = text.to_string();
        for home in &self.homes {
            let count = out.matches(home.as_str()).count();
            if count > 0 {
                out = out.replace(home.as_str(), "~");
                tally.add("homePath", count);
            }
        }
        out = replace(&USER_PATH, &out, &mut tally, "userPath", |c| format!("{}~", &c[1]));
        out = replace(&AUTHORIZATION, &out, &mut tally, "authorizationHeader", |c| format!("{}{}{REDACTED}", &c[1], &c[2]));
        out = replace(&URL_CREDENTIALS, &out, &mut tally, "urlCredentials", |c| format!("{}{REDACTED}@", &c[1]));
        let (sanitized, count) = mighty_core::shared_memory::sanitize_secrets(&out);
        if count > 0 {
            out = sanitized;
            tally.add("secretTokens", count);
        }
        out = replace_some(&SECRET_FIELD, &out, &mut tally, "secretFields", |c| {
            let value = &c[5];
            let bare = value.trim_matches(['"', '\'']);
            if bare.is_empty() || bare.starts_with("[REDACTED") || matches!(bare.to_ascii_lowercase().as_str(), "null" | "true" | "false") {
                return None;
            }
            let quote = if value.starts_with('"') {
                "\""
            } else if value.starts_with('\'') {
                "'"
            } else {
                ""
            };
            Some(format!("{}{}{}{}{quote}{REDACTED}{quote}", &c[1], &c[2], &c[3], &c[4]))
        });
        out = replace_some(&SECRET_FLAG, &out, &mut tally, "secretFields", |c| {
            (!c[3].starts_with("[REDACTED")).then(|| format!("{}{}{REDACTED}", &c[1], &c[2]))
        });
        out = replace(&LONG_HEX, &out, &mut tally, "longHex", |_| REDACTED.to_string());
        out = replace(&EMAIL, &out, &mut tally, "email", |_| "[REDACTED_EMAIL]".to_string());
        // The Git workbench's pre-commit patterns as the last word: whatever line still holds one goes whole.
        if !crate::git_workbench::scan_text_secrets(&out, "", 1).is_empty() {
            let mut lines = Vec::new();
            for line in out.split('\n') {
                if crate::git_workbench::scan_text_secrets(line, "", 1).is_empty() {
                    lines.push(line.to_string());
                } else {
                    tally.add("commitScan", 1);
                    lines.push(crate::git_workbench::redact_secret_text(line));
                }
            }
            out = lines.join("\n");
        }
        (out, tally)
    }
}

fn replace(regex: &Regex, text: &str, tally: &mut Tally, rule: &'static str, with: impl Fn(&Captures) -> String) -> String {
    replace_some(regex, text, tally, rule, |c| Some(with(c)))
}

/// Every match of `regex` replaced by what `with` makes of it, or left when it makes nothing; each replacement counted under `rule`.
fn replace_some(regex: &Regex, text: &str, tally: &mut Tally, rule: &'static str, with: impl Fn(&Captures) -> Option<String>) -> String {
    let mut count = 0;
    let out = regex.replace_all(text, |c: &Captures| match with(c) {
        Some(replacement) => {
            count += 1;
            replacement
        }
        None => c[0].to_string(),
    });
    tally.add(rule, count);
    out.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac() -> Redactor {
        Redactor::new(Some("/Users/alice"))
    }

    fn clean(text: &str) -> String {
        mac().redact(text).0
    }

    #[test]
    fn the_home_folder_becomes_a_tilde_and_other_user_names_go_too() {
        let (out, tally) = mac().redact("opened /Users/alice/MightyCode/src/main.rs and /Users/bob/notes.txt");
        assert_eq!(out, "opened ~/MightyCode/src/main.rs and ~/notes.txt");
        assert_eq!(tally.get("homePath"), 1);
        assert_eq!(tally.get("userPath"), 1);
        let windows = Redactor::new(Some(r"C:\Users\Alice"));
        assert_eq!(windows.redact(r"log at C:\Users\Alice\AppData\x.log and C:/Users/Alice/y").0, r"log at ~\AppData\x.log and ~/y");
        assert_eq!(windows.redact(r#""path":"D:\Users\carol\work""#).0, r#""path":"~\work""#);
        assert_eq!(clean("cd /home/dave/project"), "cd ~/project");
        // A web address that has /home in it is not a path.
        assert_eq!(clean("https://example.com/home/page"), "https://example.com/home/page");
        assert!(Redactor::new(Some("/")).redact("/a").0 == "/a", "a root home replaces nothing");
    }

    #[test]
    fn authorization_headers_and_url_credentials_are_taken_out() {
        assert_eq!(clean("Authorization: Basic dXNlcjpwYXNz"), "Authorization: [REDACTED]");
        assert_eq!(clean(r#"{"authorization":"token abc","x":1}"#), r#"{"authorization":"[REDACTED]","x":1}"#);
        assert_eq!(clean("Cookie: session=abc; theme=dark"), "Cookie: [REDACTED]");
        assert_eq!(clean("x-api-key=abcdef"), "x-api-key=[REDACTED]");
        assert_eq!(clean("git clone https://bob:hunter2@github.com/a/b.git"), "git clone https://[REDACTED]@github.com/a/b.git");
        assert_eq!(clean("wss://relay.example.com/m1"), "wss://relay.example.com/m1");
    }

    #[test]
    fn provider_keys_and_bearer_tokens_go_through_cores_guard() {
        let key = format!("sk-ant-{}", "a".repeat(30));
        let out = clean(&format!("using {key} now"));
        assert!(!out.contains(&key), "{out}");
        let github = format!("ghp_{}", "B".repeat(36));
        assert!(!clean(&github).contains(&github));
        let aws = "AKIAABCDEFGHIJKLMNOP";
        assert!(!clean(aws).contains(aws));
        let (out, tally) = mac().redact("Bearer eyJhbGciOiJIUzI1NiJ9.payload.signature");
        assert!(!out.contains("eyJhbGci"), "{out}");
        assert!(tally.total() >= 1);
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----";
        assert!(!clean(pem).contains("MIIEow"));
        assert!(clean("export GITHUB_TOKEN=abcdef123456").ends_with("[REDACTED_SECRET]"));
    }

    #[test]
    fn secret_fields_flags_and_query_values_are_taken_out() {
        assert_eq!(clean(r#"{"pairingKey": "q7Zx-9_abc", "port": 43139}"#), r#"{"pairingKey": "[REDACTED]", "port": 43139}"#);
        assert_eq!(clean(r#""password":"p@ss word""#), r#""password":"[REDACTED]""#);
        assert_eq!(clean("db_password=secret1 user=x"), "db_password=[REDACTED_SECRET]", "an assignment line is Core's");
        assert_eq!(clean("login password=secret1 user=x"), "login password=[REDACTED] user=x");
        assert_eq!(clean("GET /m1/pair?token=abc123&x=1"), "GET /m1/pair?token=[REDACTED]&x=1");
        assert_eq!(clean("pairingCode: 482913"), "pairingCode: [REDACTED]");
        assert_eq!(clean("mighty login --token abc.def --verbose"), "mighty login --token [REDACTED] --verbose");
        assert_eq!(clean("--api-key=xyz"), "--api-key=[REDACTED]");
        // Counts and switches that only name a token stay.
        assert_eq!(
            clean(r#"{"inputTokens": 1200, "tokenCount": 3, "token": null}"#),
            r#"{"inputTokens": 1200, "tokenCount": 3, "token": null}"#
        );
        assert_eq!(clean("maxTokens=4096"), "maxTokens=4096");
    }

    #[test]
    fn long_hex_tokens_and_emails_go_but_commit_ids_stay() {
        let token = "0123456789abcdef".repeat(4);
        assert_eq!(clean(&format!("hello {token}")), "hello [REDACTED]");
        let commit = "40331533aaaabbbbccccddddeeeeffff00001111";
        assert_eq!(clean(&format!("at {commit}")), format!("at {commit}"));
        assert_eq!(clean("signed in as alice@example.com"), "signed in as [REDACTED_EMAIL]");
    }

    #[test]
    fn nothing_the_commit_scan_finds_is_left() {
        let samples = [
            format!("aws {}", "AKIAABCDEFGHIJKLMNOP"),
            format!("gh {}", "ghp_".to_string() + &"x".repeat(40)),
            format!("gitlab glpat-{}", "y".repeat(24)),
            format!("slack xoxb-{}", "1".repeat(20)),
            format!("openai sk-proj-{}", "z".repeat(40)),
            format!("google AIza{}", "Q".repeat(35)),
            format!("stripe sk_live_{}", "4".repeat(24)),
            "-----BEGIN PRIVATE KEY-----".to_string(),
        ];
        for sample in samples {
            let out = clean(&sample);
            assert!(crate::git_workbench::scan_text_secrets(&out, "", 1).is_empty(), "{sample} -> {out}");
        }
    }

    #[test]
    fn plain_log_lines_are_left_as_they_are() {
        let line = "2026-11-24T10:00:00.000Z verify: pane 3 passed 2 commands in 1.2s (exit 0)";
        let (out, tally) = mac().redact(line);
        assert_eq!(out, line);
        assert_eq!(tally.total(), 0);
    }
}
