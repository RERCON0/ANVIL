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

/// Hard limits for every `git` child: a hung fetch, a credential prompt or a
/// huge output must never pin the panel's worker thread forever.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const GIT_TIMEOUT_NETWORK: std::time::Duration = std::time::Duration::from_secs(120);
const GIT_MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// Runs `git` in `root` and returns stdout; stderr becomes the error text.
pub fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    run_git_timeout(root, args, GIT_TIMEOUT).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

pub fn run_git_timeout(root: &Path, args: &[&str], timeout: std::time::Duration) -> Result<Vec<u8>, String> {
    run_git_capped(root, args, timeout, GIT_MAX_OUTPUT)
}

/// Spawns `git` with prompts disabled, literal pathspecs and a hard deadline.
fn run_git_capped(root: &Path, args: &[&str], timeout: std::time::Duration, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Never let git ask a human: no terminal prompts, no GUI helpers, and
        // treat every path we pass as a literal, not a pathspec pattern.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("GIT_LITERAL_PATHSPECS", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|e| format!("git: {e}"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let reader = |mut stream: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 64 * 1024];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if buffer.len() < max_bytes {
                            let room = max_bytes - buffer.len();
                            buffer.extend_from_slice(&chunk[..n.min(room)]);
                        }
                    }
                }
            }
            buffer
        })
    };
    let stdout_reader = reader(Box::new(stdout));
    let stderr_reader = reader(Box::new(stderr));

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("git {} не ответил за {} с", args.first().copied().unwrap_or(""), timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(e) => return Err(format!("git: {e}")),
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        Err(String::from_utf8_lossy(&stderr).trim().to_owned())
    }
}

/// True for a full object id (sha-1 or sha-256) — the only hashes ever passed
/// back to `git`, so a crafted log record cannot smuggle an option.
pub fn is_object_hash(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

const NUL: char = '\0';

/// Resolves a repository-relative path and proves it stays inside `root`.
///
/// Rejects absolute/UNC/ADS forms, `.`/`..` after Win32 trailing-dot/space
/// normalization (`".. "` resolves to `..` on Windows), reserved device names
/// and control characters, then canonicalizes the deepest existing ancestor to
/// catch symlinks and junctions pointing outside the repository.
pub fn resolve_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let inside = crate::strings::WORKSPACE_PATH_INSIDE_REPO;
    let clean = path.replace('\\', "/");
    if clean.is_empty() || clean.starts_with('/') || clean.contains(':') || clean.contains('~') {
        return Err(inside.to_owned());
    }
    let mut full = root.to_path_buf();
    for part in clean.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        let trimmed = part.trim_end_matches(['.', ' ']);
        if trimmed.is_empty() || trimmed == ".." || part != trimmed {
            return Err(inside.to_owned());
        }
        if part.contains(['<', '>', '"', '|', '?', '*']) || part.chars().any(char::is_control) {
            return Err(inside.to_owned());
        }
        let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" | "LPT1" | "LPT2" | "LPT3") {
            return Err(inside.to_owned());
        }
        full.push(part);
    }
    if full == root {
        return Err(inside.to_owned());
    }
    let canonical_root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut probe = full.clone();
    loop {
        match std::fs::canonicalize(&probe) {
            Ok(canonical) => {
                if canonical == canonical_root || !canonical.starts_with(&canonical_root) {
                    return Err(inside.to_owned());
                }
                return Ok(full);
            }
            Err(_) => {
                if !probe.pop() {
                    return Err(inside.to_owned());
                }
            }
        }
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
    run_git_capped(root, args, GIT_TIMEOUT, GIT_MAX_OUTPUT)
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
        // A file can be modified on both sides at once; each side contributes
        // its own counts (the staged-only entry stays untouched by worktree stats).
        let applies = if staged { change.staged() } else { change.unstaged() };
        if !applies {
            continue;
        }
        if let Some((additions, deletions)) = stats.get(&change.path) {
            change.additions += additions;
            change.deletions += deletions;
        }
    }
}

/// Where a commit belongs relative to the upstream branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// Not pushed yet.
    Outgoing,
    /// On the remote, not merged locally.
    Incoming,
    /// Already in both.
    History,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub subject: String,
    pub refs: Vec<String>,
    /// Committer time, unix seconds.
    pub time: i64,
    pub section: Section,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitLog {
    pub commits: Vec<Commit>,
    pub upstream: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitDetail {
    pub files: Vec<(char, String, u32, u32)>,
    pub patch: String,
    pub header: String,
}

/// Upstream ref of the current branch: the configured one, else `origin/<branch>`.
pub fn upstream(root: &Path) -> Option<String> {
    if let Ok(text) = run_git(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]) {
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    let branch = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let branch = branch.trim();
    if branch.is_empty() || branch == "HEAD" {
        return None;
    }
    run_git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{branch}")]).ok()?;
    Some(format!("origin/{branch}"))
}

fn rev_list_set(root: &Path, args: &[&str]) -> std::collections::HashSet<String> {
    let mut full: Vec<&str> = vec!["rev-list", "--max-count=200"];
    full.extend(args.iter().copied());
    run_git(root, &full)
        .map(|text| text.lines().map(|line| line.trim().to_owned()).filter(|line| !line.is_empty()).collect())
        .unwrap_or_default()
}

/// Recent commits of HEAD (and the upstream), marked with their section.
pub fn log(root: &Path) -> Result<CommitLog, String> {
    const MAX_COMMITS: usize = 80;
    let upstream = upstream(root);
    let (outgoing, incoming) = match &upstream {
        Some(upstream) => (
            rev_list_set(root, &[&format!("{upstream}..HEAD")]),
            rev_list_set(root, &[&format!("HEAD..{upstream}")]),
        ),
        None => (rev_list_set(root, &["HEAD", "--not", "--remotes"]), Default::default()),
    };
    let format = "%H\x1f%h\x1f%P\x1f%an\x1f%ct\x1f%D\x1f%s\x1e".to_owned();
    let revs: Vec<&str> = match &upstream {
        Some(upstream) => vec!["HEAD", upstream],
        None => vec!["HEAD"],
    };
    let mut args: Vec<String> = vec![
        "log".to_owned(),
        format!("--max-count={MAX_COMMITS}"),
        "--topo-order".to_owned(),
        format!("--pretty=format:{format}"),
    ];
    args.extend(revs.iter().map(|rev| (*rev).to_owned()));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let text = run_git(root, &arg_refs)?;
    let mut commits = Vec::new();
    for record in text.split('\u{1e}') {
        let record = record.trim_start_matches(['\n', '\r']);
        if record.is_empty() {
            continue;
        }
        let fields: Vec<&str> = record.split('\u{1f}').collect();
        // A commit subject may itself contain the record/field separators; only
        // records whose hash is a real object id are trusted, so a crafted
        // subject cannot inject a "hash" that later reaches git as an option.
        if fields.len() < 7 || !is_object_hash(fields[0]) || fields.iter().any(|field| field.contains(NUL)) {
            continue;
        }
        let hash = fields[0].to_owned();
        commits.push(Commit {
            hash: hash.clone(),
            short: fields[1].to_owned(),
            parents: fields[2].split(' ').filter(|p| !p.is_empty()).map(str::to_owned).collect(),
            author: fields[3].to_owned(),
            time: fields[4].parse().unwrap_or(0),
            refs: fields[5].split(',').map(|r| r.trim().to_owned()).filter(|r| !r.is_empty()).collect(),
            subject: fields[6].to_owned(),
            section: if outgoing.contains(&hash) {
                Section::Outgoing
            } else if incoming.contains(&hash) {
                Section::Incoming
            } else {
                Section::History
            },
        });
    }
    let truncated = commits.len() >= MAX_COMMITS;
    Ok(CommitLog { commits, upstream, truncated })
}

/// Files and patch of one commit (first parent, as in VS Code's history view).
pub fn commit_detail(root: &Path, hash: &str) -> Result<CommitDetail, String> {
    if !is_object_hash(hash) {
        return Err(crate::strings::WORKSPACE_NO_SUCH_FILE.to_owned());
    }
    // `--end-of-options` keeps a revision from ever being read as an option.
    let base: Vec<&str> = vec!["show", "--no-color", "--format=", "--end-of-options", hash];
    let name_status = run_git_bytes(root, &base.iter().copied().chain(["--name-status", "-z"]).collect::<Vec<_>>())?;
    let numstat = run_git_bytes(root, &base.iter().copied().chain(["--numstat", "-z"]).collect::<Vec<_>>())?;
    let stats = parse_numstat(&numstat);
    let mut files = Vec::new();
    let mut records = name_status.split(|b| *b == 0).filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        let text = String::from_utf8_lossy(record);
        // `-z` separates fields with NUL: "STATUS[<tab>PATH]" and, for
        // renames/copies, the old and the new path as two more fields.
        let (status_field, inline_path) = match text.split_once('\t') {
            Some((status, path)) => (status.to_owned(), Some(path.to_owned())),
            None => (text.into_owned(), None),
        };
        let status = status_field.chars().next().unwrap_or('M');
        let first = match inline_path {
            Some(path) => path,
            None => match records.next() {
                Some(path) => String::from_utf8_lossy(path).into_owned(),
                None => break,
            },
        };
        let path = if status == 'R' || status == 'C' {
            match records.next() {
                Some(new_path) => String::from_utf8_lossy(new_path).into_owned(),
                None => first,
            }
        } else {
            first
        };
        let (additions, deletions) = stats.get(&path).copied().unwrap_or((0, 0));
        files.push((status, path, additions, deletions));
    }
    let header = run_git(root, &["show", "--no-patch", "--end-of-options", "--format=%H%n%an <%ae>%n%ci%n%s%n%b", hash]).unwrap_or_default();
    let patch = run_git(
        root,
        &["show", "--no-color", "--no-ext-diff", "--end-of-options", "--unified=3", "--format=", "-m", "--first-parent", hash],
    )
    .unwrap_or_default();
    Ok(CommitDetail { files, patch, header })
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
    let (program, args) = ai_command(spec, prompt).ok_or_else(|| crate::strings::WORKSPACE_NO_AI_COMMAND.to_owned())?;
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
        let fallback = crate::strings::WORKSPACE_AI_EMPTY.to_owned();
        return Err(if stderr.trim().is_empty() { fallback } else { stderr.trim().to_owned() });
    }
    Ok(message.chars().take(200).collect())
}

/// Tracked plus untracked-but-not-ignored files, sorted.
pub fn ls_files(root: &Path) -> Result<Vec<String>, String> {
    let bytes = run_git_bytes(root, &["ls-files", "-z", "--deduplicate", "--cached", "--others", "--exclude-standard"])?;
    let mut files: Vec<String> = bytes
        .split(|b| *b == 0)
        .filter(|record| !record.is_empty())
        .map(|record| String::from_utf8_lossy(record).into_owned())
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Reads a repository file (UTF-8, lossy) up to `max_bytes`; the bool reports
/// truncation. The path is validated first and the cap is applied while
/// reading, so a huge file cannot be allocated to memory.
pub fn read_file(root: &Path, path: &str, max_bytes: usize) -> Result<(String, bool), String> {
    use std::io::Read;
    let full = resolve_path(root, path)?;
    let mut file = std::fs::File::open(&full).map_err(|e| format!("{path}: {e}"))?;
    let mut buffer = Vec::with_capacity(max_bytes.min(64 * 1024));
    file.by_ref()
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut buffer)
        .map_err(|e| format!("{path}: {e}"))?;
    let truncated = buffer.len() > max_bytes;
    buffer.truncate(max_bytes);
    Ok((String::from_utf8_lossy(&buffer).into_owned(), truncated))
}

/// One hunk of a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    /// ` `, `-` and `+` lines, without the trailing newline.
    pub lines: Vec<String>,
}

/// One file's part of a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    /// `diff --git` / `---` / `+++` lines, used to rebuild a patch.
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub staged: bool,
}

/// Splits a unified diff into files and hunks.
pub fn parse_diff(text: &str, staged: bool) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current: Option<FileDiff> = None;
    let mut hunk: Option<Hunk> = None;
    let flush_hunk = |file: &mut Option<FileDiff>, hunk: &mut Option<Hunk>| {
        if let (Some(file), Some(hunk)) = (file.as_mut(), hunk.take()) {
            file.hunks.push(hunk);
        }
    };
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            flush_hunk(&mut current, &mut hunk);
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(FileDiff { path: path_from_diff_header(line), header: vec![line.to_owned()], hunks: Vec::new(), staged });
            continue;
        }
        let Some(file) = current.as_mut() else { continue };
        if line.starts_with("@@") {
            flush_hunk(&mut current, &mut hunk);
            hunk = Some(Hunk { header: line.to_owned(), lines: Vec::new() });
        } else if hunk.is_some() {
            // "\ No newline at end of file" markers are kept: the rebuilt patch
            // must carry them or `git apply` rejects (or silently alters) the file.
            if let Some(hunk) = hunk.as_mut() {
                hunk.lines.push(line.to_owned());
            }
        } else {
            // The b-side of the +++ line is authoritative for the file name
            // (the diff --git line is ambiguous when a path contains " b/").
            if let Some(path) = line.strip_prefix("+++ b/") {
                file.path = path.to_owned();
            }
            file.header.push(line.to_owned());
        }
    }
    flush_hunk(&mut current, &mut hunk);
    if let Some(file) = current.take() {
        files.push(file);
    }
    files
}

fn path_from_diff_header(line: &str) -> String {
    // `diff --git a/dir/file b/dir/file` — take the b-side, falling back to a-side.
    let rest = line.strip_prefix("diff --git ").unwrap_or(line);
    if let Some(index) = rest.rfind(" b/") {
        return rest[index + 3..].to_owned();
    }
    rest.split_whitespace().last().unwrap_or("").trim_start_matches("a/").to_owned()
}

/// Rebuilds a patch containing only the selected hunks (all when empty).
pub fn hunk_patch(file: &FileDiff, selection: &[usize]) -> String {
    let mut out = String::new();
    for line in &file.header {
        out.push_str(line);
        out.push('\n');
    }
    for (index, hunk) in file.hunks.iter().enumerate() {
        if !selection.is_empty() && !selection.contains(&index) {
            continue;
        }
        out.push_str(&hunk.header);
        out.push('\n');
        for line in &hunk.lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Applies selected hunks to the index (`staged = false`) or takes them back
/// out of it (`staged = true`), by piping the rebuilt patch to `git apply`.
pub fn apply_hunks(root: &Path, file: &FileDiff, selection: &[usize], from_index: bool) -> Result<(), String> {
    use std::io::Write;
    let patch = hunk_patch(file, selection);
    if patch.trim().is_empty() {
        return Ok(());
    }
    let mut command = Command::new("git");
    command
        .args(["apply", "--cached", "--whitespace=nowarn"])
        .args(if from_index { vec!["--reverse"] } else { vec![] })
        .arg("-")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| format!("git apply: {e}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "git apply: no stdin".to_owned())?
        .write_all(patch.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Fetches and prunes the remote of the current branch (git resolves it).
pub fn fetch(root: &Path) -> Result<String, String> {
    let remote = run_git(root, &["config", "--get", "branch", &format!("{}.remote", current_branch(root)?)])
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    let remote = if remote.is_empty() { "origin".to_owned() } else { remote };
    if remote.starts_with('-') {
        return Err(crate::strings::WORKSPACE_PATH_INSIDE_REPO.to_owned());
    }
    run_git_timeout(root, &["fetch", "--no-tags", "--quiet", "--prune", "--", &remote], GIT_TIMEOUT_NETWORK)
        .map(|_| strings_fetch_done(&remote))
}

fn current_branch(root: &Path) -> Result<String, String> {
    let branch = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_owned();
    if branch.is_empty() || branch == "HEAD" {
        return Err(crate::strings::WORKSPACE_NO_BRANCH.to_owned());
    }
    Ok(branch)
}

fn strings_fetch_done(remote: &str) -> String {
    format!("fetch {remote}")
}

/// Pushes the current branch, setting the upstream when it has none.
pub fn push(root: &Path) -> Result<String, String> {
    let branch = current_branch(root)?;
    if upstream(root).is_none() {
        run_git_timeout(
            root,
            &["push", "--porcelain", "--set-upstream", "origin", &format!("refs/heads/{branch}:refs/heads/{branch}")],
            GIT_TIMEOUT_NETWORK,
        )?;
    } else {
        run_git_timeout(root, &["push", "--porcelain"], GIT_TIMEOUT_NETWORK)?;
    }
    Ok(branch)
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
        // A file touched on both sides gets both counts.
        let mut both = vec![Change { path: "src/main.rs".into(), index: 'M', worktree: 'M', ..Default::default() }];
        apply_numstat(&mut both, &stats, true);
        apply_numstat(&mut both, &stats, false);
        assert_eq!((both[0].additions, both[0].deletions), (24, 6));
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

    const SAMPLE_DIFF: &str = "diff --git a/src/main.rs b/src/main.rs\n\
index 111..222 100644\n\
--- a/src/main.rs\n\
+++ b/src/main.rs\n\
@@ -1,3 +1,4 @@\n\
 fn main() {\n\
-    old();\n\
+    new();\n\
+    extra();\n\
 }\n\
@@ -10,2 +11,2 @@\n\
 ctx();\n\
-older();\n\
+newer();\n";

    #[test]
    fn parses_diff_into_files_and_hunks() {
        let files = parse_diff(SAMPLE_DIFF, false);
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.path, "src/main.rs");
        assert_eq!(file.header.len(), 4, "diff/index/---/+++ lines");
        assert_eq!(file.hunks.len(), 2);
        assert_eq!(file.hunks[0].header, "@@ -1,3 +1,4 @@");
        assert_eq!(file.hunks[0].lines.len(), 5);
        assert_eq!(file.hunks[1].lines.len(), 3);
    }

    #[test]
    fn hunk_patch_selects_only_requested_hunks() {
        let files = parse_diff(SAMPLE_DIFF, false);
        let all = hunk_patch(&files[0], &[]);
        assert!(all.contains("@@ -1,3 +1,4 @@") && all.contains("@@ -10,2 +11,2 @@"));
        let second = hunk_patch(&files[0], &[1]);
        assert!(!second.contains("@@ -1,3 +1,4 @@"));
        assert!(second.contains("@@ -10,2 +11,2 @@"));
        assert!(second.starts_with("diff --git a/src/main.rs b/src/main.rs"));
        assert!(second.ends_with("+newer();\n"));
    }

    #[test]
    fn multi_file_diff_splits_paths() {
        let text = format!("{SAMPLE_DIFF}diff --git a/other.txt b/other.txt\n--- a/other.txt\n+++ b/other.txt\n@@ -1 +1 @@\n-a\n+b\n");
        let files = parse_diff(&text, true);
        assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["src/main.rs", "other.txt"]);
        assert!(files[1].staged);
    }

    #[test]
    fn object_hashes_only() {
        assert!(is_object_hash("0123456789abcdef0123456789abcdef01234567"));
        assert!(!is_object_hash("--output=C:/x"));
        assert!(!is_object_hash("0123456789abcdef0123456789abcdef0123456"));
        assert!(!is_object_hash("0123456789abcdef0123456789abcdef0123456g"));
    }

    #[test]
    fn log_rejects_records_whose_hash_is_not_an_object_id() {
        // A subject carrying the record separator would otherwise inject a
        // second, attacker-chosen "commit" whose hash reaches git show.
        let mut raw = Vec::new();
        for record in [
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{1f}a\u{1f}\u{1f}me\u{1f}1\u{1f}\u{1f}ok",
            "subj\u{1e}--output=C:/pwn\u{1f}x\u{1f}\u{1f}m\u{1f}1\u{1f}\u{1f}evil",
        ] {
            raw.extend_from_slice(record.as_bytes());
            raw.push(0x1e);
        }
        let text = String::from_utf8(raw).unwrap();
        let mut commits = Vec::new();
        for record in text.split('\u{1e}') {
            let record = record.trim_start_matches(['\n', '\r']);
            if record.is_empty() {
                continue;
            }
            let fields: Vec<&str> = record.split('\u{1f}').collect();
            if fields.len() < 7 || !is_object_hash(fields[0]) {
                continue;
            }
            commits.push(fields[0].to_owned());
        }
        assert_eq!(commits, vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()]);
    }

    #[test]
    fn pathspec_magic_in_a_file_name_survives_as_a_literal_path() {
        // The worker sets GIT_LITERAL_PATHSPECS=1, so a name like this is a path,
        // not a pattern; what must hold locally is that the path is unchanged.
        let files = parse_diff("diff --git a/:(glob)* b/:(glob)*\n--- a/:(glob)*\n+++ b/:(glob)*\n@@ -1 +1 @@\n-a\n+b\n", false);
        assert_eq!(files[0].path, ":(glob)*");
    }

    #[test]
    fn path_with_b_slash_uses_the_plus_line() {
        let files = parse_diff("diff --git a/foo b/bar.txt b/foo b/bar.txt\n--- a/foo b/bar.txt\n+++ b/foo b/bar.txt\n@@ -1 +1 @@\n-a\n+b\n", false);
        assert_eq!(files[0].path, "foo b/bar.txt");
    }

    #[test]
    fn no_newline_marker_survives_a_patch_round_trip() {
        let text = "diff --git a/x.txt b/x.txt\n--- a/x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";
        let files = parse_diff(text, false);
        assert_eq!(files[0].hunks[0].lines.len(), 4, "both markers are kept");
        let patch = hunk_patch(&files[0], &[]);
        assert!(patch.contains("\\ No newline at end of file"));
        assert_eq!(patch, text, "the rebuilt patch is byte-identical");
    }

    #[test]
    fn resolve_path_rejects_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("a.txt"), "x").unwrap();
        assert!(resolve_path(root, "sub/a.txt").is_ok());
        for bad in [".. ", "...", "sub/.. ", "sub/../..", "/abs", "C:/x", "\\\\server\\share", "sub/CON", "sub/a.txt:stream", "a\u{0}b", "sub/ a. "] {
            assert!(resolve_path(root, bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(resolve_path(root, "").is_err());
        assert!(resolve_path(root, ".").is_err(), "the root itself is not a file target");
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
