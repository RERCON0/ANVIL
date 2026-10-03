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

#[cfg(test)]
mod tests {
    use super::*;

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
