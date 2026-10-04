//! Search query -> pattern for alacritty's `RegexSearch`.

use alacritty_terminal::term::search::RegexSearch;

/// The compiled search for the last pattern. `RegexSearch::new` builds four
/// lazy DFAs: too much to redo on every frame while the search bar is open.
#[derive(Default)]
pub struct RegexCache {
    entry: Option<(String, Option<RegexSearch>)>,
}

impl RegexCache {
    /// The search for `pattern`, compiled once; None when it does not compile.
    pub fn get(&mut self, pattern: &str) -> Option<&mut RegexSearch> {
        if self.entry.as_ref().is_none_or(|(key, _)| key != pattern) {
            self.entry = Some((pattern.to_owned(), RegexSearch::new(pattern).ok()));
        }
        self.entry.as_mut().and_then(|(_, regex)| regex.as_mut())
    }
}

/// Plain queries are escaped; case-insensitive search uses the `(?i)` flag.
pub fn search_pattern(query: &str, regex: bool, case_sensitive: bool) -> String {
    let body = if regex { query.to_owned() } else { escape_regex(query) };
    if case_sensitive {
        // alacritty's RegexSearch turns on smart-case, which would still match
        // uppercase text for a lowercase query; an explicit flag overrides it.
        format!("(?-i){body}")
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
    fn a_pattern_is_compiled_once() {
        let mut cache = RegexCache::default();
        let first = cache.get("(?i)error").map(|regex| regex as *const RegexSearch).expect("compiles");
        let again = cache.get("(?i)error").map(|regex| regex as *const RegexSearch).expect("compiles");
        assert!(std::ptr::eq(first, again), "the same pattern reuses the compiled search");
        assert!(cache.get("(").is_none(), "an invalid pattern");
        assert!(cache.get("(?i)warn").is_some(), "a new pattern compiles again");
    }

    #[test]
    fn plain_queries_are_escaped() {
        assert_eq!(search_pattern("a.b(c)", false, true), r"(?-i)a\.b\(c\)");
        assert_eq!(search_pattern("C:\\x", false, true), r"(?-i)C:\\x");
        assert_eq!(search_pattern("err", false, false), "(?i)err");
    }

    #[test]
    fn regex_queries_pass_through() {
        assert_eq!(search_pattern("e+r", true, true), "(?-i)e+r");
        assert_eq!(search_pattern("e+r", true, false), "(?i)e+r");
    }

    struct Size {
        cols: usize,
        lines: usize,
    }

    impl alacritty_terminal::grid::Dimensions for Size {
        fn total_lines(&self) -> usize {
            self.lines
        }
        fn screen_lines(&self) -> usize {
            self.lines
        }
        fn columns(&self) -> usize {
            self.cols
        }
    }

    /// alacritty's RegexSearch enables smart-case (uppercase in the pattern
    /// turns case sensitivity on); the explicit flag must override it.
    #[test]
    fn case_sensitive_pattern_beats_smart_case() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::index::{Column, Line, Point};
        use alacritty_terminal::term::search::RegexSearch;
        use alacritty_terminal::term::{Config, Term};
        use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

        let mut term = Term::new(Config::default(), &Size { cols: 20, lines: 2 }, VoidListener);
        let mut processor: Processor<StdSyncHandler> = Processor::new();
        processor.advance(&mut term, b"ERR err");
        let line_end = Point::new(Line(0), Column(19));

        let mut sensitive = RegexSearch::new(&search_pattern("err", false, true)).unwrap();
        let found = term.regex_search_right(&mut sensitive, Point::new(Line(0), Column(0)), line_end).expect("match");
        assert_eq!(found.start().column.0, 4, "case-sensitive skips ERR");

        let mut insensitive = RegexSearch::new(&search_pattern("err", false, false)).unwrap();
        let found = term.regex_search_right(&mut insensitive, Point::new(Line(0), Column(0)), line_end).expect("match");
        assert_eq!(found.start().column.0, 0, "case-insensitive matches ERR");
    }

    #[test]
    fn escaped_pattern_compiles_and_matches_literally() {
        use alacritty_terminal::term::search::RegexSearch;
        assert!(RegexSearch::new(&search_pattern("[a-z]+ (x) ^$", false, false)).is_ok());
    }
}
