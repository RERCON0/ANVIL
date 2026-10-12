//! Rewriting the message of a local commit without touching the working tree.
//!
//! The commit and everything above it are copied as raw objects: the target
//! gets the new message, each commit above it is the same object with its
//! parent lines pointing at the copies. The new tip has the tree of the old
//! one, so the index and the working files stay valid; only the branch moves.
use super::{base_git_command, is_object_hash, is_unpushed, preflight, run_git, GIT_MAX_OUTPUT, GIT_TIMEOUT};
use crate::process::run_bounded;
use std::collections::HashMap;
use std::path::Path;

/// Replaces the message of `hash` and replays the commits above it on the
/// result. Returns the id of the rewritten commit.
///
/// Only a commit that is not pushed yet qualifies, judged by git at this very
/// moment rather than by what a panel showed earlier. The commit hooks
/// (`pre-commit`, `commit-msg`, `post-commit`, `post-rewrite`) do not run: only
/// a message changes. Moving the branch is a ref update, and git runs the
/// repository's `reference-transaction` hook for any ref update; that hook is
/// among the programs the repository trust covers.
pub fn reword(root: &Path, hash: &str, message: &str) -> Result<String, String> {
    if !is_object_hash(hash) {
        return Err(crate::strings::WORKSPACE_NO_SUCH_FILE().to_owned());
    }
    let message = clean_message(message).ok_or_else(|| crate::strings::WORKSPACE_REWORD_EMPTY().to_owned())?;
    if operation_in_progress(root)? {
        return Err(crate::strings::WORKSPACE_REWORD_BUSY().to_owned());
    }
    let head = run_git(root, &["rev-parse", "--verify", "--end-of-options", "HEAD"])?.trim().to_owned();
    if !is_unpushed(root, hash)? {
        return Err(crate::strings::WORKSPACE_REWORD_PUSHED().to_owned());
    }
    let range = format!("{hash}..{head}");
    let above =
        run_git(root, &["rev-list", "--topo-order", "--reverse", "--ancestry-path", "--end-of-options", &range])?;
    let mut order = vec![hash.to_owned()];
    for line in above.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if !is_object_hash(line) {
            return Err(crate::strings::WORKSPACE_REWORD_UNEXPECTED().to_owned());
        }
        order.push(line.to_owned());
    }
    // The repository is checked once for the whole replay: one `cat-file` and
    // one `hash-object` per commit are plumbing that runs no hook, filter or
    // external program, and a check per call costs more than the call. The ref
    // update below is checked again, as it is the one step that can run a hook.
    let checked = preflight(root)?;
    // Read everything before writing anything: a refusal leaves no objects behind.
    let raws = read_commits(root, &checked.executable, &order)?;
    if let Some(index) = raws.iter().position(|raw| is_signed(raw)) {
        return Err(crate::strings::workspace_reword_signed(&order[index][..7]));
    }
    let mut copies: HashMap<String, String> = HashMap::new();
    for (commit, raw) in order.iter().zip(&raws) {
        let replacement = (commit == hash).then_some(message.as_bytes());
        let copy = write_commit(root, &checked.executable, rewrite(raw, replacement, &copies))?;
        copies.insert(commit.clone(), copy);
    }
    let new_head = copies.get(&head).ok_or_else(|| crate::strings::WORKSPACE_REWORD_UNEXPECTED().to_owned())?;
    // The old tip must still be what HEAD points at, or git refuses the move.
    run_git(root, &["update-ref", "-m", "reword: message edited", "HEAD", new_head, &head])?;
    Ok(copies[hash].clone())
}

/// A rebase, merge, cherry-pick or revert owns HEAD until it finishes.
fn operation_in_progress(root: &Path) -> Result<bool, String> {
    let markers = ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"];
    let mut args = vec!["rev-parse"];
    for marker in markers {
        args.extend(["--git-path", marker]);
    }
    let paths = run_git(root, &args)?;
    Ok(paths.lines().map(str::trim).filter(|path| !path.is_empty()).any(|path| root.join(path).exists()))
}

/// Whether the commit carries a signature in its headers (never in its message).
fn is_signed(raw: &[u8]) -> bool {
    let (headers, _) = split_commit(raw);
    headers.split(|byte| *byte == b'\n').any(|line| line.starts_with(b"gpgsig"))
}

/// A raw commit object as its headers and its message.
fn split_commit(raw: &[u8]) -> (&[u8], &[u8]) {
    match raw.windows(2).position(|pair| pair == b"\n\n") {
        Some(end) => (&raw[..end], &raw[end + 2..]),
        None => (raw.strip_suffix(b"\n").unwrap_or(raw), &raw[raw.len()..]),
    }
}

/// The text `git commit -F` would store: no trailing spaces, no leading or
/// trailing blank lines, one blank line between paragraphs, a final newline.
fn clean_message(message: &str) -> Option<String> {
    let mut cleaned = String::new();
    let mut paragraph_break = false;
    for line in message.lines() {
        // C's `isspace`, which is what git strips: `trim_end` also takes NBSP and the like.
        let line = line.trim_end_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
        if line.is_empty() {
            paragraph_break = !cleaned.is_empty();
            continue;
        }
        if paragraph_break {
            cleaned.push('\n');
            paragraph_break = false;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }
    (!cleaned.is_empty()).then_some(cleaned)
}

/// A raw commit object with its parent lines mapped through `copies` and,
/// when `message` is given, that message in place of the original. Every other
/// header and every byte of an unchanged message stay as they were. The new
/// message is UTF-8, which is what a commit without an `encoding` header holds,
/// so that header belongs to the message it replaces.
fn rewrite(raw: &[u8], message: Option<&[u8]>, copies: &HashMap<String, String>) -> Vec<u8> {
    let (headers, original) = split_commit(raw);
    let mut lines: Vec<Vec<u8>> = Vec::new();
    for line in headers.split(|byte| *byte == b'\n') {
        if let Some(parent) = line.strip_prefix(b"parent ") {
            let mapped = std::str::from_utf8(parent).ok().and_then(|id| copies.get(id));
            match mapped {
                Some(copy) => lines.push(format!("parent {copy}").into_bytes()),
                None => lines.push(line.to_vec()),
            }
        } else if message.is_some() && line.starts_with(b"encoding ") {
            continue;
        } else {
            lines.push(line.to_vec());
        }
    }
    let mut out = lines.join(&b'\n');
    out.extend_from_slice(b"\n\n");
    out.extend_from_slice(message.unwrap_or(original));
    out
}

/// One git plumbing call with the panel's usual safety settings and deadline.
fn plumbing(root: &Path, executable: &Path, args: &[&str], input: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut command = base_git_command(root, executable);
    command.args(args);
    let label = format!("git {}", args.first().copied().unwrap_or(""));
    let (success, stdout, stderr) = run_bounded(command, &label, GIT_TIMEOUT, GIT_MAX_OUTPUT, Some(input))?;
    if success {
        Ok(stdout)
    } else {
        Err(String::from_utf8_lossy(&stderr).trim().to_owned())
    }
}

/// The raw objects of `ids`, in that order, from a single `git cat-file --batch`.
fn read_commits(root: &Path, executable: &Path, ids: &[String]) -> Result<Vec<Vec<u8>>, String> {
    let unexpected = || crate::strings::WORKSPACE_REWORD_UNEXPECTED().to_owned();
    let input: Vec<u8> = ids.iter().flat_map(|id| format!("{id}\n").into_bytes()).collect();
    let output = plumbing(root, executable, &["cat-file", "--batch"], input)?;
    let mut rest = output.as_slice();
    let mut commits = Vec::with_capacity(ids.len());
    for id in ids {
        // `<id> <type> <size>`, then exactly `size` bytes and a newline.
        let end = rest.iter().position(|byte| *byte == b'\n').ok_or_else(unexpected)?;
        let header = std::str::from_utf8(&rest[..end]).map_err(|_| unexpected())?;
        let mut fields = header.split(' ');
        let (found, kind, size) = (fields.next(), fields.next(), fields.next());
        if found != Some(id.as_str()) || kind != Some("commit") {
            return Err(unexpected());
        }
        let size: usize = size.and_then(|size| size.parse().ok()).ok_or_else(unexpected)?;
        let body = rest.get(end + 1..end + 1 + size).ok_or_else(unexpected)?;
        commits.push(body.to_vec());
        rest = rest.get(end + 2 + size..).unwrap_or_default();
    }
    Ok(commits)
}

/// Stores a commit object and returns its id.
fn write_commit(root: &Path, executable: &Path, object: Vec<u8>) -> Result<String, String> {
    let stdout = plumbing(root, executable, &["hash-object", "-t", "commit", "-w", "--stdin"], object)?;
    let id = String::from_utf8_lossy(&stdout).trim().to_owned();
    if is_object_hash(&id) {
        Ok(id)
    } else {
        Err(crate::strings::WORKSPACE_REWORD_UNEXPECTED().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: &str = "0000000000000000000000000000000000000001";
    const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const NEW: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const OTHER: &str = "cccccccccccccccccccccccccccccccccccccccc";

    fn commit(parents: &[&str], extra: &str, message: &[u8]) -> Vec<u8> {
        let mut text = format!("tree {TREE}\n");
        for parent in parents {
            text.push_str(&format!("parent {parent}\n"));
        }
        text.push_str("author A <a@x> 1 +0000\ncommitter C <c@x> 2 +0000\n");
        text.push_str(extra);
        let mut raw = text.into_bytes();
        raw.push(b'\n');
        raw.extend_from_slice(message);
        raw
    }

    fn text(raw: &[u8]) -> String {
        String::from_utf8_lossy(raw).into_owned()
    }

    #[test]
    fn the_new_message_is_utf8_so_a_legacy_encoding_header_goes_with_the_old_message() {
        let raw = commit(&[], "encoding ISO-8859-1\n", b"old\n");
        let edited = rewrite(&raw, Some(b"new\n"), &HashMap::new());
        assert!(!text(&edited).contains("encoding"), "{}", text(&edited));
        assert!(text(&edited).ends_with("\n\nnew\n"));
        let replayed = rewrite(&raw, None, &HashMap::new());
        assert_eq!(replayed, raw, "a commit that only moves keeps its encoding header and every byte");
    }

    #[test]
    fn parents_follow_the_copies_and_unknown_parents_stay() {
        let raw = commit(&[OLD, OTHER], "", b"merge\n");
        let copies = HashMap::from([(OLD.to_owned(), NEW.to_owned())]);
        let moved = text(&rewrite(&raw, None, &copies));
        assert!(moved.contains(&format!("parent {NEW}\nparent {OTHER}\n")), "{moved}");
        assert!(!moved.contains(OLD));
    }

    #[test]
    fn a_replayed_message_keeps_crlf_and_bytes_that_are_not_utf8() {
        let message: &[u8] = b"subject\r\n\r\n\xcf\xf0\xe8\xe2\xe5\xf2\r\n";
        let raw = commit(&[OLD], "", message);
        let copies = HashMap::from([(OLD.to_owned(), NEW.to_owned())]);
        let out = rewrite(&raw, None, &copies);
        assert!(out.ends_with(message));
    }

    #[test]
    fn a_signature_is_only_looked_for_in_the_headers() {
        let signed = commit(&[], "gpgsig -----BEGIN PGP SIGNATURE-----\n \n -----END PGP SIGNATURE-----\n", b"m\n");
        assert!(is_signed(&signed));
        let sha256 = commit(&[], "gpgsig-sha256 -----BEGIN PGP SIGNATURE-----\n", b"m\n");
        assert!(is_signed(&sha256));
        let mentions = commit(&[], "", b"gpgsig is how a signature header starts\n");
        assert!(!is_signed(&mentions), "a message that mentions it is not signed");
    }

    /// `git commit -F` strips what C's `isspace` calls space: no NBSP, no
    /// ideographic space, which a message may carry on purpose.
    #[test]
    fn only_ascii_whitespace_is_stripped_from_the_line_ends() {
        assert_eq!(clean_message("a\u{a0}\nb\u{3000}"), Some("a\u{a0}\nb\u{3000}\n".to_owned()));
        assert_eq!(clean_message("a \t\x0b\x0c\r\nb"), Some("a\nb\n".to_owned()));
        assert_eq!(clean_message("\u{a0}"), Some("\u{a0}\n".to_owned()), "a lone NBSP is text, as git sees it");
    }

    #[test]
    fn messages_are_cleaned_like_git_commit_does() {
        assert_eq!(clean_message("a"), Some("a\n".to_owned()));
        assert_eq!(clean_message("\n\n  a  \r\n\r\n\r\n\r\nb\t\n\n"), Some("  a\n\nb\n".to_owned()));
        assert_eq!(clean_message("# kept\nbody"), Some("# kept\nbody\n".to_owned()));
        assert_eq!(clean_message(" \n\t\n"), None);
        assert_eq!(clean_message(""), None);
    }
}
