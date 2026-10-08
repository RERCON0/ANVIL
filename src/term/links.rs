//! Which links Ctrl+click may open.

/// Only these schemes are ever opened; file paths and anything else are not.
/// This allowlist is the whole policy: `is_openable` gates both entry paths
/// (OSC 8 targets and the Ctrl-held scan of visible lines).
pub fn is_openable(url: &str) -> bool {
    if url.len() > 32 * 1024 || url.chars().any(char::is_control) {
        return false;
    }
    ["http://", "https://", "ftp://", "mailto:"].iter().any(|scheme| {
        url.len() > scheme.len() && url.get(..scheme.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
    })
}

/// Drops trailing punctuation that is almost never part of a URL, keeping a
/// closing bracket when the URL itself opened one (Wikipedia-style links).
pub fn trim_url(raw: &str) -> &str {
    // Count delimiters once. Recounting and chars().last() for every trailing
    // byte made long punctuation runs quadratic on the UI thread.
    let mut brackets = [0_usize; 6];
    for byte in raw.bytes() {
        if let Some(index) = b"()[]{}".iter().position(|candidate| *candidate == byte) {
            brackets[index] += 1;
        }
    }
    let mut end = raw.len();
    for (index, last) in raw.char_indices().rev() {
        let closing = match last {
            ')' => Some(1),
            ']' => Some(3),
            '}' => Some(5),
            _ => None,
        };
        let strip = if let Some(closing) = closing {
            let strip = brackets[closing] > brackets[closing - 1];
            if strip {
                brackets[closing] -= 1;
            }
            strip
        } else {
            matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"')
        };
        if !strip {
            break;
        }
        end = index;
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
    fn hostile_links_are_bounded_and_trimmed_in_one_pass() {
        assert!(!is_openable("https://example.com/\ncommand"));
        assert!(!is_openable(&format!("https://example.com/{}", "a".repeat(32 * 1024))));
        let tail = format!("https://example.com/путь{}", ")]}.!?".repeat(20_000));
        assert_eq!(trim_url(&tail), "https://example.com/путь");
        assert_eq!(trim_url("https://example.com/(x))]"), "https://example.com/(x)");
    }

    #[test]
    fn trimming() {
        assert_eq!(trim_url("https://x.io/a."), "https://x.io/a");
        assert_eq!(trim_url("https://x.io/a),"), "https://x.io/a");
        assert_eq!(
            trim_url("https://en.wikipedia.org/wiki/Rust_(language)"),
            "https://en.wikipedia.org/wiki/Rust_(language)"
        );
        assert_eq!(trim_url("(https://x.io/a)"), "(https://x.io/a)");
        assert_eq!(trim_url("https://x.io/\"'"), "https://x.io/");
    }
}
