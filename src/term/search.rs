//! Search query -> pattern for alacritty's `RegexSearch`.

use alacritty_terminal::term::search::RegexSearch;

/// The compiled search for the last pattern. `RegexSearch::new` builds four
/// lazy DFAs: too much to redo on every frame while the search bar is open.
#[derive(Default)]
pub struct RegexCache {
    entry: Option<(String, Option<RegexSearch>)>,
    query: Option<(String, bool, bool)>,
    generation: u64,
}

impl RegexCache {
    /// The search for `pattern`, compiled once; None when it does not compile.
    pub fn get(&mut self, pattern: &str) -> Option<&mut RegexSearch> {
        self.query = None;
        if self.entry.as_ref().is_none_or(|(key, _)| key != pattern) {
            self.entry = Some((pattern.to_owned(), RegexSearch::new(pattern).ok()));
        }
        self.entry.as_mut().and_then(|(_, regex)| regex.as_mut())
    }

    /// Pattern allocation and compilation happen on edits, not cursor-blink frames.
    pub fn for_query(&mut self, query: &str, regex: bool, case_sensitive: bool) -> (u64, Option<&mut RegexSearch>) {
        if self.query.as_ref().is_none_or(|(old, re, case)| old != query || *re != regex || *case != case_sensitive) {
            let pattern = search_pattern(query, regex, case_sensitive);
            self.entry = Some((pattern.clone(), RegexSearch::new(&pattern).ok()));
            self.query = Some((query.to_owned(), regex, case_sensitive));
            self.generation = self.generation.wrapping_add(1);
        }
        (self.generation, self.entry.as_mut().and_then(|(_, regex)| regex.as_mut()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MatchKey {
    pub query: u64,
    pub output: u64,
    pub offset: usize,
    pub lines: usize,
    pub columns: usize,
    pub history: usize,
}

#[derive(Default)]
pub(super) struct VisibleMatches {
    key: Option<MatchKey>,
    matches: Vec<(usize, usize, usize)>,
}

impl VisibleMatches {
    pub fn get(
        &mut self,
        key: MatchKey,
        scan: impl FnOnce() -> Vec<(usize, usize, usize)>,
    ) -> &[(usize, usize, usize)] {
        if self.key != Some(key) {
            self.matches = scan();
            self.key = Some(key);
        }
        &self.matches
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
    fn unchanged_query_reuses_pattern_but_flags_and_invalid_patterns_change_generation() {
        let mut cache = RegexCache::default();
        let (first, compiled) = cache.for_query("a.b", false, false);
        assert!(compiled.is_some());
        assert_eq!(cache.for_query("a.b", false, false).0, first);
        let (changed, compiled) = cache.for_query("a.b", false, true);
        assert!(changed != first && compiled.is_some());
        let (invalid, compiled) = cache.for_query("(", true, true);
        assert!(invalid != changed && compiled.is_none());
        assert_eq!(cache.for_query("(", true, true).0, invalid);
        assert!(cache.for_query("(", false, true).1.is_some());
    }

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
