//! Terminal output as plain lines and a pane's transcript as lines (MC-9's `pane read` and `pane wait-output`).

use std::collections::VecDeque;

/// `text` without terminal escape sequences (CSI `ESC [ … final`, OSC `ESC ] … BEL|ESC \`, DCS/APC/PM/SOS strings and two-byte
/// escapes), carriage returns and other control characters but tab and newline.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    // Parameters and intermediates, then one final byte in @..~.
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    // A string ended by BEL or ST (ESC \).
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // A character set's designation (`ESC ( B`): one intermediate, then its final byte.
                Some(' '..='/') => {
                    chars.next();
                }
                Some(_) | None => {}
            },
            '\u{9b}' => {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The last lines of a terminal's output, at most `max_lines` lines of at most `max_line_chars` characters.
#[derive(Debug, Clone)]
pub struct OutputRing {
    lines: VecDeque<String>,
    partial: String,
    partial_chars: usize,
    max_lines: usize,
    max_line_chars: usize,
}

impl OutputRing {
    pub fn new(max_lines: usize, max_line_chars: usize) -> OutputRing {
        OutputRing {
            lines: VecDeque::new(),
            partial: String::new(),
            partial_chars: 0,
            max_lines: max_lines.max(1),
            max_line_chars: max_line_chars.max(1),
        }
    }

    /// Takes plain text (already without escapes) and gives the lines it completed.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        let mut done = Vec::new();
        for c in text.chars() {
            if c == '\n' {
                let line = std::mem::take(&mut self.partial);
                self.partial_chars = 0;
                done.push(line.clone());
                self.lines.push_back(line);
                while self.lines.len() > self.max_lines {
                    self.lines.pop_front();
                }
            } else if self.partial_chars < self.max_line_chars {
                self.partial.push(c);
                self.partial_chars += 1;
            }
        }
        done
    }

    /// The text of the line not yet ended.
    pub fn partial(&self) -> &str {
        &self.partial
    }

    /// The last `n` lines, the unfinished one last when it holds anything.
    pub fn tail(&self, n: usize) -> Vec<String> {
        let mut all: Vec<String> = self.lines.iter().cloned().collect();
        if !self.partial.is_empty() {
            all.push(self.partial.clone());
        }
        let skip = all.len().saturating_sub(n);
        all.into_iter().skip(skip).collect()
    }
}

/// A transcript as lines: each entry `kind: text`, its text's lines under it indented, each entry's text passed through `redact`;
/// the last `n` lines, and whether earlier ones were left out.
pub fn transcript_lines<'a>(
    entries: impl IntoIterator<Item = (&'a str, &'a str)>,
    n: usize,
    redact: &dyn Fn(&str) -> String,
) -> (Vec<String>, bool) {
    let mut lines: Vec<String> = Vec::new();
    for (kind, text) in entries {
        let text = redact(text.trim_end());
        let mut parts = text.lines();
        let first = parts.next().unwrap_or("");
        lines.push(format!("{kind}: {first}"));
        lines.extend(parts.map(|line| format!("  {line}")));
    }
    let skip = lines.len().saturating_sub(n);
    (lines.into_iter().skip(skip).collect(), skip > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_control_characters_are_dropped() {
        assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[0m done\r\n"), "ok done\n");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}after"), "after");
        assert_eq!(strip_ansi("\u{1b}]8;;https://x\u{1b}\\link\u{1b}]8;;\u{1b}\\"), "link");
        assert_eq!(strip_ansi("a\u{1b}(Bb\tc\u{8}"), "ab\tc");
        assert_eq!(strip_ansi("한글 \u{1b}[K줄"), "한글 줄");
    }

    #[test]
    fn the_output_ring_keeps_the_last_lines() {
        let mut ring = OutputRing::new(3, 5);
        assert_eq!(ring.push("one\ntwo\nthr"), vec!["one".to_string(), "two".to_string()]);
        assert_eq!(ring.partial(), "thr");
        assert_eq!(ring.push("ee\nfour-long-line\nfive\n"), vec!["three".to_string(), "four-".to_string(), "five".to_string()]);
        assert_eq!(ring.tail(10), vec!["three", "four-", "five"]);
        ring.push("six");
        assert_eq!(ring.tail(2), vec!["five", "six"]);
    }

    #[test]
    fn a_transcript_reads_as_lines_redacted_and_cut_from_the_top() {
        let entries = [("user", "fix it"), ("assistant", "Done.\nAPI_KEY=sk-123\nbye"), ("tool", "ran tests\n")];
        let redact = |text: &str| text.lines().map(|l| if l.contains("API_KEY") { "[redacted]" } else { l }).collect::<Vec<_>>().join("\n");
        let (lines, cut) = transcript_lines(entries, 10, &redact);
        assert_eq!(lines, vec!["user: fix it", "assistant: Done.", "  [redacted]", "  bye", "tool: ran tests"]);
        assert!(!cut);
        let (lines, cut) = transcript_lines(entries, 2, &redact);
        assert_eq!(lines, vec!["  bye", "tool: ran tests"]);
        assert!(cut);
    }
}
