//! Git integration for the workspace panel. Shells out to `git` and parses its
//! plumbing output; the parsers are pure and unit-tested.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One changed file, as `git status --porcelain=v2` reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub original_path: Option<String>,
    /// Index (staged) status letter, '.' when unmodified.
    pub index: char,
    /// Worktree status letter.
    pub worktree: char,
    pub untracked: bool,
    /// Unmerged (conflict) entry.
    pub unmerged: bool,
    pub additions: u32,
    pub deletions: u32,
}

impl Change {
    pub fn staged(&self) -> bool {
        !self.untracked && !self.unmerged && self.index != '.'
    }

    pub fn unstaged(&self) -> bool {
        (self.untracked || self.unmerged) || self.worktree != '.'
    }

    /// Single-letter badge, as in VS Code's SCM view.
    pub fn letter(&self) -> char {
        if self.unmerged {
            return 'U';
        }
        if self.untracked {
            return '?';
        }
        for c in [self.index, self.worktree] {
            match c {
                'M' | 'A' | 'D' | 'R' | 'C' | 'T' => return c,
                _ => {}
            }
        }
        '?'
    }

    pub fn directory(&self) -> &str {
        match self.path.rfind('/') {
            Some(index) => &self.path[..index],
            None => "",
        }
    }

    pub fn file_name(&self) -> &str {
        match self.path.rfind('/') {
            Some(index) => &self.path[index + 1..],
            None => &self.path,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<Change>,
    pub additions: u32,
    pub deletions: u32,
}

impl Status {
    pub fn is_repo(&self) -> bool {
        !self.branch.is_empty() || !self.changes.is_empty()
    }
}

/// Runs `git` in `root` and returns stdout; stderr becomes the error text.
pub fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output().map_err(|e| format!("git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// The repository root containing `cwd`, if any.
pub fn find_root(cwd: &Path) -> Option<PathBuf> {
    if !cwd.is_dir() {
        return None;
    }
    run_git(cwd, &["rev-parse", "--show-toplevel"]).ok().map(|text| PathBuf::from(text.trim()))
}

/// Reads branch, ahead/behind and every change (staged, unstaged, untracked).
pub fn status(root: &Path) -> Result<Status, String> {
    let raw = run_git_bytes(root, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])?;
    let mut status = parse_status(&raw);
    if let Ok(numstat) = run_git(root, &["diff", "--numstat", "-z", "--no-renames"]) {
        apply_numstat(&mut status.changes, &parse_numstat(numstat.as_bytes()), false);
    }
    if let Ok(numstat) = run_git(root, &["diff", "--cached", "--numstat", "-z", "--no-renames"]) {
        apply_numstat(&mut status.changes, &parse_numstat(numstat.as_bytes()), true);
    }
    status.additions = status.changes.iter().map(|c| c.additions).sum();
    status.deletions = status.changes.iter().map(|c| c.deletions).sum();
    Ok(status)
}

fn run_git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|e| format!("git: {e}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Parses `status --porcelain=v2 --branch -z`.
pub fn parse_status(bytes: &[u8]) -> Status {
    let mut status = Status::default();
    let mut records = bytes.split(|b| *b == 0);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(record);
        match record[0] {
            b'#' => {
                if let Some(rest) = text.strip_prefix("# branch.head ") {
                    status.branch = rest.to_owned();
                } else if let Some(rest) = text.strip_prefix("# branch.upstream ") {
                    status.upstream = Some(rest.to_owned());
                } else if let Some(rest) = text.strip_prefix("# branch.ab ") {
                    let mut parts = rest.split_whitespace();
                    status.ahead = parts.next().and_then(|v| v.trim_start_matches('+').parse().ok()).unwrap_or(0);
                    status.behind = parts.next().and_then(|v| v.trim_start_matches('-').parse().ok()).unwrap_or(0);
                }
            }
            b'1' => {
                let fields: Vec<&str> = text.splitn(9, ' ').collect();
                if let Some(change) = plain_change(&fields) {
                    status.changes.push(change);
                }
            }
            b'2' => {
                let fields: Vec<&str> = text.splitn(10, ' ').collect();
                let original = records.next().map(|value| String::from_utf8_lossy(value).into_owned());
                if let Some(mut change) = plain_change(&fields) {
                    change.original_path = original.filter(|value| !value.is_empty());
                    status.changes.push(change);
                }
            }
            b'u' => {
                let fields: Vec<&str> = text.splitn(11, ' ').collect();
                let xy = fields.get(1).copied().unwrap_or("..");
                let path = fields.get(10).copied().unwrap_or("").to_owned();
                let mut letters = xy.chars();
                status.changes.push(Change {
                    path,
                    original_path: None,
                    index: letters.next().unwrap_or('.'),
                    worktree: letters.next().unwrap_or('.'),
                    untracked: false,
                    unmerged: true,
                    additions: 0,
                    deletions: 0,
                });
            }
            b'?' => {
                let path = text.split_once(' ').map(|(_, rest)| rest).unwrap_or("").to_owned();
                if !path.is_empty() {
                    status.changes.push(Change {
                        path,
                        original_path: None,
                        index: '?',
                        worktree: '?',
                        untracked: true,
                        unmerged: false,
                        additions: 0,
                        deletions: 0,
                    });
                }
            }
            _ => {}
        }
    }
    status.changes.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

fn plain_change(fields: &[&str]) -> Option<Change> {
    let xy = fields.get(1)?;
    let path = fields.last()?.to_string();
    if path.is_empty() {
        return None;
    }
    let mut letters = xy.chars();
    Some(Change {
        path,
        original_path: None,
        index: letters.next().unwrap_or('.'),
        worktree: letters.next().unwrap_or('.'),
        untracked: false,
        unmerged: false,
        additions: 0,
        deletions: 0,
    })
}

/// Parses `diff --numstat -z` into path -> (additions, deletions).
pub fn parse_numstat(bytes: &[u8]) -> HashMap<String, (u32, u32)> {
    let mut out = HashMap::new();
    for record in bytes.split(|b| *b == 0) {
        if record.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(record);
        let mut parts = text.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let count = |value: &str| value.parse::<u32>().unwrap_or(0);
        out.insert(path.to_owned(), (count(add), count(del)));
    }
    out
}

fn apply_numstat(changes: &mut [Change], stats: &HashMap<String, (u32, u32)>, staged: bool) {
    for change in changes.iter_mut() {
        if staged != change.staged() {
            continue;
        }
        if let Some((additions, deletions)) = stats.get(&change.path) {
            change.additions = *additions;
            change.deletions = *deletions;
        }
    }
}

/// Unified diff of one file (worktree and index sides, empty parts dropped).
pub fn diff(root: &Path, path: &str, staged: bool) -> String {
    let mut args = vec!["diff", "--no-ext-diff", "--no-color", "--unified=3"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(path);
    run_git(root, &args).unwrap_or_default()
}

/// Stages or unstages the given paths.
pub fn stage(root: &Path, paths: &[String], staged: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args: Vec<&str> = if staged { vec!["add", "--"] } else { vec!["restore", "--staged", "--"] };
    args.extend(paths.iter().map(String::as_str));
    run_git(root, &args).map(|_| ())
}

/// Commits staged changes; returns the short hash.
pub fn commit(root: &Path, message: &str) -> Result<String, String> {
    run_git(root, &["commit", "-m", message])?;
    run_git(root, &["rev-parse", "--short", "HEAD"]).map(|hash| hash.trim().to_owned())
}

/// The last `count` commit subjects (AI prompt context).
pub fn recent_subjects(root: &Path, count: usize) -> Vec<String> {
    run_git(root, &["log", &format!("--max-count={count}"), "--pretty=format:%s"])
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Command + argv for the AI commit message, per CLI.
pub fn ai_command(spec: &str, prompt: &str) -> Option<(String, Vec<String>)> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    let formatted: Option<Vec<&str>> = match trimmed {
        "claude" => Some(vec!["-p", prompt]),
        "opencode" => Some(vec!["run", prompt]),
        "codex" => Some(vec!["exec", prompt]),
        "gemini" => Some(vec!["-p", prompt]),
        "aider" => Some(vec!["--message", prompt, "--no-auto-commits", "--yes-always"]),
        _ => None,
    };
    if let Some(args) = formatted {
        return Some((trimmed.to_owned(), args.into_iter().map(str::to_owned).collect()));
    }
    let mut parts = trimmed.split_whitespace();
    let command = parts.next()?.to_owned();
    let mut args: Vec<String> = parts.map(str::to_owned).collect();
    args.push(prompt.to_owned());
    Some((command, args))
}

/// Asks the given CLI (or the auto-detected one) for a one-line commit message.
pub fn ai_commit_message(root: &Path, command: Option<&str>, prompt: &str) -> Result<String, String> {
    let spec = command.unwrap_or("");
    let (program, args) = ai_command(spec, prompt).ok_or_else(|| "не выбрана AI-команда".to_owned())?;
    let mut invocation = Command::new(&program);
    invocation
        .args(&args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        invocation.creation_flags(0x0800_0000);
    }
    let output = invocation.output().map_err(|e| format!("{program}: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let message = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("```") && !line.starts_with('#'))
        .unwrap_or("")
        .trim_matches('"')
        .to_owned();
    if message.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(if stderr.trim().is_empty() { "CLI вернул пустое сообщение".to_owned() } else { stderr.trim().to_owned() });
    }
    Ok(message.chars().take(200).collect())
}

/// The prompt sent to the AI CLI, built from the diff and recent subjects.
pub fn ai_prompt(status_text: &str, diff_text: &str, recent: &[String]) -> String {
    let log = recent.join("\n");
    [
        "Write a Git commit message for the following changes.".to_owned(),
        "Respond with the commit message only: a single line of at most 72 characters, no quotes, no code fences, no explanations.".to_owned(),
        if log.is_empty() { String::new() } else { format!("Recent commit subjects for style reference (do not repeat them):\n{log}") },
        if status_text.is_empty() { String::new() } else { format!("Git status:\n{status_text}") },
        if diff_text.is_empty() { "The diff is empty.".to_owned() } else { format!("Diff:\n{diff_text}") },
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_bytes(records: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for record in records {
            out.extend_from_slice(record.as_bytes());
            out.push(0);
        }
        out
    }

    #[test]
    fn parses_branch_and_ahead_behind() {
        let raw = status_bytes(&["# branch.oid abc", "# branch.head main", "# branch.upstream origin/main", "# branch.ab +2 -1"]);
        let status = parse_status(&raw);
        assert_eq!(status.branch, "main");
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 1));
    }

    #[test]
    fn parses_plain_rename_untracked_and_conflict() {
        let raw = status_bytes(&[
            "1 M. N... 100644 100644 100644 aaa bbb src/main.rs",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new name.rs",
            "old name.rs",
            "? notes/todo.txt",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.txt",
        ]);
        let status = parse_status(&raw);
        assert_eq!(status.changes.len(), 4);
        let find = |path: &str| status.changes.iter().find(|c| c.path == path).expect(path);
        let main = find("src/main.rs");
        assert_eq!((main.index, main.worktree), ('M', '.'));
        assert!(main.staged() && !main.unstaged());
        let renamed = find("new name.rs");
        assert_eq!(renamed.original_path.as_deref(), Some("old name.rs"));
        assert_eq!(renamed.letter(), 'R');
        let untracked = find("notes/todo.txt");
        assert!(untracked.untracked && untracked.unstaged() && !untracked.staged());
        assert_eq!(untracked.letter(), '?');
        let conflict = find("conflict.txt");
        assert!(conflict.unmerged);
        assert_eq!(conflict.letter(), 'U');
    }

    #[test]
    fn paths_with_spaces_survive() {
        let raw = status_bytes(&["1 .M N... 100644 100644 100644 aaa bbb a file with spaces.txt"]);
        let status = parse_status(&raw);
        assert_eq!(status.changes[0].path, "a file with spaces.txt");
        assert_eq!(status.changes[0].directory(), "");
        assert_eq!(status.changes[0].file_name(), "a file with spaces.txt");
        assert_eq!(status.changes[0].directory(), "a file with spaces.txt".rsplit_once('/').map(|(d, _)| d).unwrap_or(""));
    }

    #[test]
    fn numstat_and_summing() {
        let stats = parse_numstat(b"12\t3\tsrc/main.rs\0-\t-\timage.png\0");
        assert_eq!(stats.get("src/main.rs"), Some(&(12, 3)));
        assert_eq!(stats.get("image.png"), Some(&(0, 0)));
        let mut changes = vec![
            Change { path: "src/main.rs".into(), index: '.', worktree: 'M', ..Default::default() },
            Change { path: "src/main.rs".into(), index: 'M', worktree: '.', ..Default::default() },
        ];
        apply_numstat(&mut changes, &stats, false);
        assert_eq!((changes[0].additions, changes[0].deletions), (12, 3));
        assert_eq!((changes[1].additions, changes[1].deletions), (0, 0), "staged entry untouched");
    }

    #[test]
    fn ai_command_formats() {
        assert_eq!(
            ai_command("claude", "prompt"),
            Some(("claude".to_owned(), vec!["-p".to_owned(), "prompt".to_owned()]))
        );
        assert_eq!(
            ai_command("opencode", "p"),
            Some(("opencode".to_owned(), vec!["run".to_owned(), "p".to_owned()]))
        );
        let (program, args) = ai_command("my-cli --fast", "p").unwrap();
        assert_eq!((program.as_str(), args), ("my-cli", vec!["--fast".to_owned(), "p".to_owned()]));
        assert_eq!(ai_command("   ", "p"), None);
    }

    #[test]
    fn ai_prompt_contents() {
        let prompt = ai_prompt(" M a.rs", "diff --git", &["feat: past".to_owned()]);
        assert!(prompt.contains("at most 72 characters"));
        assert!(prompt.contains("feat: past"));
        assert!(prompt.contains("Diff:\ndiff --git"));
        let empty = ai_prompt("", "", &[]);
        assert!(empty.contains("The diff is empty."));
    }
}
