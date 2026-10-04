//! Clipboard text -> bytes for the PTY.

/// Line breaks become CR (what Enter sends). With bracketed paste on, every ESC
/// is stripped so the text cannot close the bracket early (`ESC[201~`) or
/// smuggle other sequences, then the text is wrapped in `ESC[200~`/`ESC[201~`.
pub fn prepare_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return normalized.into_bytes();
    }
    let body: String = normalized.chars().filter(|&c| c != '\x1b').collect();
    let mut out = Vec::with_capacity(body.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(body.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

/// How the pane's shell reads a quoted path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathQuoting {
    /// bash and friends: `'...'`, a `'` inside becomes `'\''`.
    Unix,
    /// `'...'`, quote characters (PowerShell also treats the typographic ones
    /// as quotes) doubled.
    PowerShell,
    /// `"..."`. Windows paths cannot contain `"`, and inside quotes the
    /// interactive cmd prompt takes `^`, `!` and `%%` literally, so nothing is
    /// escaped (Helm's `^^`/`%%` doubling corrupted such paths).
    Cmd,
}

/// A dropped file or folder as text for the shell: control characters
/// removed, quoted for `quoting`, followed by a space so several drops (or
/// the next argument) stay separate — Helm's path drop.
pub fn quote_path(path: &str, quoting: PathQuoting) -> String {
    let path: String = path.chars().filter(|c| !c.is_control()).collect();
    let quoted = match quoting {
        PathQuoting::Unix => format!("'{}'", path.replace('\'', r"'\''")),
        PathQuoting::PowerShell => {
            let mut out = String::with_capacity(path.len() + 2);
            out.push('\'');
            for c in path.chars() {
                if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                    out.push(c);
                }
                out.push(c);
            }
            out.push('\'');
            out
        }
        PathQuoting::Cmd => format!("\"{path}\""),
    };
    quoted + " "
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_are_quoted_for_the_shell() {
        let dir = r"C:\Users\rerco\Desktop\24";
        assert_eq!(quote_path(dir, PathQuoting::PowerShell), r"'C:\Users\rerco\Desktop\24' ");
        assert_eq!(quote_path(dir, PathQuoting::Cmd), r#""C:\Users\rerco\Desktop\24" "#);
        assert_eq!(quote_path(dir, PathQuoting::Unix), r"'C:\Users\rerco\Desktop\24' ");
        assert_eq!(quote_path(r"C:\it's", PathQuoting::Unix), r"'C:\it'\''s' ");
        assert_eq!(quote_path("C:\\it\u{2019}s", PathQuoting::PowerShell), "'C:\\it\u{2019}\u{2019}s' ");
        assert_eq!(quote_path(r"C:\100%^!", PathQuoting::Cmd), r#""C:\100%^!" "#, "cmd takes these literally");
        assert_eq!(quote_path("C:\\a\r\nb\x1b[201~", PathQuoting::PowerShell), "'C:\\ab[201~' ", "no control characters");
    }

    #[test]
    fn newlines_become_carriage_returns() {
        assert_eq!(prepare_paste("a\r\nb\nc", false), b"a\rb\rc");
    }

    #[test]
    fn bracketed_wraps_and_strips_escapes() {
        assert_eq!(prepare_paste("ls\n", true), b"\x1b[200~ls\r\x1b[201~");
        assert_eq!(
            prepare_paste("x\x1b[201~rm -rf /\n", true),
            b"\x1b[200~x[201~rm -rf /\r\x1b[201~"
        );
    }

    #[test]
    fn unicode_survives() {
        assert_eq!(prepare_paste("привет", true), "\x1b[200~привет\x1b[201~".as_bytes());
    }
}
