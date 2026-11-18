//! Remote-workspace text counted as the Mac counts it (C# `RemoteText`, R\RemoteText.cs; plan §1.6): in Characters (Swift's
//! `String.count` and `prefix`), which are extended grapheme clusters, the same text elements C#'s `StringInfo` reads. A Rust string
//! holds no lone surrogate, so C#'s skipping of one has nothing to do here. Both walk the text only as far as the limit, so a 2 MiB
//! host answer costs no more than its first characters. Owner: W8-D.

use unicode_segmentation::UnicodeSegmentation;

/// The default limit of [`cut`] (C#'s `max = 300`).
pub const DEFAULT_CUT: usize = 300;

/// `RemoteText.Cut(value, max)`: the first `max` characters (the Mac's `String(value.prefix(max))`), never a part of one; "" for 0.
pub fn cut(value: &str, max: usize) -> String {
    if max == 0 || value.is_empty() {
        return String::new();
    }
    match value.grapheme_indices(true).nth(max) {
        Some((end, _)) => value[..end].to_string(),
        None => value.to_string(),
    }
}

/// `RemoteText.Count(value, limit)`: how many characters the Mac counts (`value.count`), counting no further than `limit`.
pub fn count(value: &str, limit: usize) -> usize {
    if limit == 0 {
        return 0;
    }
    value.graphemes(true).take(limit).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_and_count_walk_whole_characters() {
        let family = "👨‍👩‍👧";
        let text = format!("a{family}b");
        assert_eq!(count(&text, usize::MAX), 3);
        assert_eq!(cut(&text, 2), format!("a{family}"));
        assert_eq!(cut(&text, 0), "");
        assert_eq!(cut("", 5), "");
        assert_eq!(count(&text, 2), 2);
        assert_eq!(cut("e\u{301}x", 1), "e\u{301}");
    }
}
