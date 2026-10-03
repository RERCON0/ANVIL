//! Search query -> pattern for alacritty's `RegexSearch`.

/// Plain queries are escaped; case-insensitive search uses the `(?i)` flag.
pub fn search_pattern(query: &str, regex: bool, case_sensitive: bool) -> String {
    let body = if regex { query.to_owned() } else { escape_regex(query) };
    if case_sensitive {
        body
    } else {
        format!("(?i){body}")
    }
}

pub fn escape_regex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_queries_are_escaped() {
        assert_eq!(search_pattern("a.b(c)", false, true), r"a\.b\(c\)");
        assert_eq!(search_pattern("C:\\x", false, true), r"C:\\x");
        assert_eq!(search_pattern("err", false, false), "(?i)err");
    }

    #[test]
    fn regex_queries_pass_through() {
        assert_eq!(search_pattern("e+r", true, true), "e+r");
        assert_eq!(search_pattern("e+r", true, false), "(?i)e+r");
    }

    #[test]
    fn escaped_pattern_compiles_and_matches_literally() {
        use alacritty_terminal::term::search::RegexSearch;
        assert!(RegexSearch::new(&search_pattern("[a-z]+ (x) ^$", false, false)).is_ok());
    }
}
