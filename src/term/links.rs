//! Which links Ctrl+click may open.

/// Pattern for alacritty's `RegexSearch` over visible lines while Ctrl is held.
pub const URL_PATTERN: &str = r#"(?:https?://|ftp://|mailto:)[^\s<>"'`]+"#;

/// Only these schemes are ever opened; file paths and anything else are not.
pub fn is_openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["http://", "https://", "ftp://", "mailto:"].iter().any(|s| lower.starts_with(s) && lower.len() > s.len())
}

/// Drops trailing punctuation that is almost never part of a URL, keeping a
/// closing bracket when the URL itself opened one (Wikipedia-style links).
pub fn trim_url(raw: &str) -> &str {
    let mut end = raw.len();
    loop {
        let s = &raw[..end];
        let Some(last) = s.chars().last() else { break };
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => s.matches('(').count() < s.matches(')').count(),
            ']' => s.matches('[').count() < s.matches(']').count(),
            '}' => s.matches('{').count() < s.matches('}').count(),
            _ => false,
        };
        if !strip {
            break;
        }
        end -= last.len_utf8();
    }
    &raw[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes() {
        assert!(is_openable("https://example.com"));
        assert!(is_openable("HTTP://example.com"));
        assert!(is_openable("mailto:me@example.com"));
        assert!(!is_openable("file:///C:/Windows"));
        assert!(!is_openable("javascript:alert(1)"));
        assert!(!is_openable("https://"));
    }

    #[test]
    fn trimming() {
        assert_eq!(trim_url("https://x.io/a."), "https://x.io/a");
        assert_eq!(trim_url("https://x.io/a),"), "https://x.io/a");
        assert_eq!(trim_url("https://en.wikipedia.org/wiki/Rust_(language)"), "https://en.wikipedia.org/wiki/Rust_(language)");
        assert_eq!(trim_url("(https://x.io/a)"), "(https://x.io/a)");
        assert_eq!(trim_url("https://x.io/\"'"), "https://x.io/");
    }
}
