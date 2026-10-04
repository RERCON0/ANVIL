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
    pub head_oid: Option<String>,
    pub upstream_oid: Option<String>,
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
const GIT_TIMEOUT_COMMIT: std::time::Duration = std::time::Duration::from_secs(600);
const GIT_MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// Runs `git` in `root` and returns stdout; stderr becomes the error text.
pub fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    run_git_timeout(root, args, GIT_TIMEOUT).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

pub fn run_git_timeout(root: &Path, args: &[&str], timeout: std::time::Duration) -> Result<Vec<u8>, String> {
    run_git_capped(root, args, timeout, GIT_MAX_OUTPUT)
}

/// `git` in `root`, configured the same way for every call the panel makes.
/// The panel follows the shell into any folder, so the repository's own
/// config must not be able to run code on a mere poll, and the user's config
/// must not change the output the parsers read.
fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        // Command-line config wins over the repository's: a repo-local
        // fsmonitor hook or signature verifier must not run on a refresh,
        // and quotePath must not C-quote non-ASCII names in diff headers.
        .args(["-c", "core.fsmonitor=false", "-c", "core.quotePath=false", "-c", "log.showSignature=false"])
        .current_dir(root)
        // Never let git ask a human: no terminal prompts, no GUI helpers, and
        // treat every path we pass as a literal, not a pathspec pattern.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("GIT_LITERAL_PATHSPECS", "1")
        // The background poll must not take index.lock to refresh stat data:
        // a `git add` or commit in the shell next to it would then fail.
        .env("GIT_OPTIONAL_LOCKS", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// Spawns `git` with prompts disabled, literal pathspecs and a hard deadline.
fn run_git_capped(root: &Path, args: &[&str], timeout: std::time::Duration, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut command = git_command(root);
    command.args(args);
    let label = format!("git {}", args.first().copied().unwrap_or(""));
    let (success, stdout, stderr) = run_bounded(command, &label, timeout, max_bytes, None)?;
    if success {
        Ok(stdout)
    } else {
        Err(String::from_utf8_lossy(&stderr).trim().to_owned())
    }
}

/// Runs `command` without a console, feeding it `input` (or no stdin),
/// keeping at most `max_bytes` of each output stream and killing it at
/// `timeout`: a hung fetch, a credential prompt or a CLI that never answers
/// must not pin the panel's worker thread. `(success, stdout, stderr)`.
fn run_bounded(
    mut command: Command,
    label: &str,
    timeout: std::time::Duration,
    max_bytes: usize,
    input: Option<Vec<u8>>,
) -> Result<(bool, Vec<u8>, Vec<u8>), String> {
    command
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|e| format!("{label}: {e}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        // Written on its own thread: a child that answers before reading all
        // of its input would otherwise deadlock against the full output pipe.
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = stdin.write_all(&input);
        });
    }
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    // Each stream is drained on its own thread and handed over through a
    // channel, never joined: a grandchild (a hook, ssh) that inherited the
    // pipe can keep it open long after the process itself is gone.
    let reader = |mut stream: Box<dyn std::io::Read + Send>| {
        let (tx, rx) = std::sync::mpsc::channel();
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
            let _ = tx.send(buffer);
        });
        rx
    };
    let stdout_rx = reader(Box::new(stdout));
    let stderr_rx = reader(Box::new(stderr));

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{label} не ответил за {} с", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(e) => return Err(format!("{label}: {e}")),
        }
    };
    // The process has exited; its own output is complete. Wait a little for
    // the pipes to close, but never longer than the deadline allows.
    let collect = |rx: std::sync::mpsc::Receiver<Vec<u8>>| {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        rx.recv_timeout(left.max(std::time::Duration::from_millis(500))).unwrap_or_default()
    };
    let stdout = collect(stdout_rx);
    let stderr = collect(stderr_rx);
    Ok((status.success(), stdout, stderr))
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
                // The nearest existing ancestor may be the root itself (a new
                // file at the top level); the target itself may not be the
                // root (a junction pointing back at it).
                let is_root = canonical == canonical_root && probe == full;
                if is_root || !canonical.starts_with(&canonical_root) {
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

/// Exact effective configuration approved for one repository/session.
/// Keep values opaque: Git configuration may contain credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct RepositoryStamp(std::sync::Arc<Vec<u8>>);

impl std::fmt::Debug for RepositoryStamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RepositoryStamp")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryIdentity {
    pub root: PathBuf,
    pub stamp: RepositoryStamp,
}

/// Root/config discovery is read-only: unlike status/diff/log, these Git
/// commands cannot invoke clean filters, hooks or signature programs.
pub fn repository_identity(cwd: &Path) -> Result<Option<RepositoryIdentity>, String> {
    reject_network_repository(cwd)?;
    let Some(root) = find_root(cwd) else { return Ok(None) };
    let stamp = repository_stamp(&root)?;
    Ok(Some(RepositoryIdentity { root, stamp }))
}

/// Includes all effective scopes and included files, not just .git/config.
/// Compare before accepting approval and before running repository commands.
pub fn repository_stamp(root: &Path) -> Result<RepositoryStamp, String> {
    reject_network_repository(root)?;
    const CONFIG_MAX_OUTPUT: usize = 1024 * 1024;
    let config = run_git_capped(
        root,
        &["config", "--includes", "--show-origin", "--show-scope", "--null", "--list"],
        GIT_TIMEOUT,
        CONFIG_MAX_OUTPUT,
    )?;
    if config.len() >= CONFIG_MAX_OUTPUT || config.last() != Some(&0) {
        return Err("Не удалось полностью прочитать конфигурацию Git; доступ к репозиторию не разрешён.".to_owned());
    }
    Ok(RepositoryStamp(std::sync::Arc::new(config)))
}

fn reject_network_repository(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if matches!(
            path.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
        ) {
            return Err("Git-панель не открывает сетевые репозитории.".to_owned());
        }
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

/// Reads branch, ahead/behind and every change (staged, unstaged, untracked).
pub fn status(root: &Path) -> Result<Status, String> {
    let raw = run_git_bytes(root, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])?;
    let mut status = parse_status(&raw);
    if status.head_oid.is_some() || status.upstream.is_some() {
        // Match log's origin/<branch> fallback without making a branch name
        // an option or interpreting it as a revision expression.
        let reference: std::borrow::Cow<'_, str> = if status.upstream.is_some() {
            "@{upstream}^{commit}".into()
        } else {
            format!("refs/remotes/origin/{}^{{commit}}", status.branch).into()
        };
        status.upstream_oid = run_git(root, &["rev-parse", "--verify", "--quiet", "--end-of-options", &reference])
            .ok()
            .map(|oid| oid.trim().to_owned())
            .filter(|oid| is_object_hash(oid));
    }
    if let Ok(numstat) = run_git(root, &["diff", "--no-ext-diff", "--no-textconv", "--numstat", "-z", "--no-renames"]) {
        apply_numstat(&mut status.changes, &parse_numstat(numstat.as_bytes()), false);
    }
    if let Ok(numstat) = run_git(root, &["diff", "--no-ext-diff", "--no-textconv", "--cached", "--numstat", "-z", "--no-renames"]) {
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
                if let Some(rest) = text.strip_prefix("# branch.oid ") {
                    status.head_oid = is_object_hash(rest).then(|| rest.to_owned());
                } else if let Some(rest) = text.strip_prefix("# branch.head ") {
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

/// Files and message of one commit, relative to its first parent.
pub fn commit_detail(root: &Path, hash: &str) -> Result<CommitDetail, String> {
    if !is_object_hash(hash) {
        return Err(crate::strings::WORKSPACE_NO_SUCH_FILE.to_owned());
    }
    // `--end-of-options` keeps a revision from ever being read as an option,
    // so every option must precede it: git rejects options that follow.
    let name_status = run_git_bytes(root, &["show", "--no-color", "--format=", "--first-parent", "--diff-merges=first-parent", "--name-status", "-z", "--end-of-options", hash])?;
    let numstat = run_git_bytes(root, &["show", "--no-color", "--no-ext-diff", "--no-textconv", "--format=", "--first-parent", "--diff-merges=first-parent", "--numstat", "-z", "--end-of-options", hash])?;
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
    let header = run_git(root, &["show", "--no-patch", "--format=%H%n%an <%ae>%n%ci%n%s%n%b", "--end-of-options", hash]).unwrap_or_default();
    Ok(CommitDetail { files, header })
}

/// Load only the selected commit file; paths remain literal even with glob characters.
pub fn commit_file_diff(root: &Path, hash: &str, path: &str) -> Result<String, String> {
    if !is_object_hash(hash) {
        return Err(crate::strings::WORKSPACE_NO_SUCH_FILE.to_owned());
    }
    run_git(
        root,
        &[
            "show", "--no-color", "--no-ext-diff", "--no-textconv", "--unified=3", "--format=",
            "--src-prefix=a/", "--dst-prefix=b/", "--first-parent", "--diff-merges=first-parent",
            "--end-of-options", hash, "--", path,
        ],
    )
}

/// Unified diff of one file as git printed it, byte for byte: hunk patches are
/// rebuilt from these bytes, so a file in another encoding or with CRLF line
/// ends is staged exactly. No external diff or textconv drivers (a repository
/// could point them at any program), and fixed `a/`/`b/` prefixes whatever
/// `diff.noprefix` or `diff.mnemonicPrefix` say, since `git apply` expects them.
pub fn diff_bytes(root: &Path, path: &str, staged: bool) -> Vec<u8> {
    let mut args =
        vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--unified=3", "--src-prefix=a/", "--dst-prefix=b/"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(path);
    run_git_bytes(root, &args).unwrap_or_default()
}

/// The same diff as text, for display.
pub fn diff(root: &Path, path: &str, staged: bool) -> String {
    String::from_utf8_lossy(&diff_bytes(root, path, staged)).into_owned()
}

/// Stages or unstages the given paths.
/// Stages or unstages the given paths. They go to git on stdin, NUL
/// separated: "stage all" in a large change set does not fit a command line
/// (os error 206 past 32767 characters). Before the first commit there is no
/// HEAD to restore from, so unstaging removes the paths from the index.
pub fn stage(root: &Path, paths: &[String], staged: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let from_stdin = ["--pathspec-from-file=-", "--pathspec-file-nul"];
    let command: &[&str] = if staged {
        &["add"]
    } else if run_git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok() {
        &["restore", "--staged"]
    } else {
        &["rm", "--cached", "--quiet", "-r"]
    };
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    let mut git = git_command(root);
    git.args(command).args(from_stdin);
    let (success, _, stderr) = run_bounded(git, &format!("git {}", command[0]), GIT_TIMEOUT, GIT_MAX_OUTPUT, Some(input))?;
    if success {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&stderr).trim().to_owned())
    }
}

/// Commits staged changes; consumes the UTF-8 message without copying it.
/// Returns the short hash.
pub fn commit(root: &Path, message: String) -> Result<String, String> {
    // Commit hooks (husky, lint-staged, a first pre-commit run) easily take
    // longer than the polling timeout, and killing git mid-commit leaves
    // index.lock behind. Stdin also avoids Windows' command-line size limit.
    let mut command = git_command(root);
    command.args(["commit", "-F", "-"]);
    let (success, _, stderr) = run_bounded(command, "git commit", GIT_TIMEOUT_COMMIT, GIT_MAX_OUTPUT, Some(message.into_bytes()))?;
    if !success {
        return Err(String::from_utf8_lossy(&stderr).trim().to_owned());
    }
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
        // No --yes-always: it approves whatever the model proposes. --no-git:
        // the CLI runs outside the repository (see ai_commit_message).
        "aider" => Some(vec!["--message", prompt, "--no-auto-commits", "--no-git"]),
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

/// Windows does not resolve an npm `.cmd` shim like an executable. OpenCode's
/// npm package ships a native binary, so use it directly: a shell would
/// reinterpret diff metacharacters and impose its shorter command-line limit.
fn ai_cli_command(program: &str) -> Command {
    #[cfg(windows)]
    if program == "opencode" {
        let path = std::env::var_os("PATH");
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
        if let Some(executable) = opencode_executable(path.as_deref(), home.as_deref(), appdata.as_deref()) {
            return Command::new(executable);
        }
    }
    Command::new(program)
}

const WINDOWS_COMMAND_LIMIT: usize = 32_767; // Includes the terminating NUL.
const WINDOWS_BATCH_LIMIT: usize = 8_191; // Conservative, also including NUL.

fn utf16_len(text: &std::ffi::OsStr) -> usize {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        text.encode_wide().count()
    }
    #[cfg(not(windows))]
    text.to_string_lossy().encode_utf16().count()
}

/// Rust's regular Windows argv escaping, measured without building a second
/// command line. Batch arguments use a conservative always-quoted budget and
/// include Rust's expansion of percent signs.
fn windows_argument_units(text: &std::ffi::OsStr, batch: bool) -> usize {
    let quoted = batch || text.is_empty() || text.as_encoded_bytes().iter().any(|byte| *byte == b' ' || *byte == b'\t');
    let count = |units: &mut dyn Iterator<Item = u16>| {
        let mut size = if quoted { 2 } else { 0 };
        let mut backslashes = 0;
        for unit in units {
            size += 1;
            if unit == b'\\' as u16 {
                backslashes += 1;
            } else {
                if unit == b'"' as u16 {
                    size += backslashes + 1;
                } else if batch && matches!(unit, 37 | 13) {
                    size += 7;
                }
                backslashes = 0;
            }
        }
        size + if quoted { backslashes } else { 0 }
    };
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        count(&mut text.encode_wide())
    }
    #[cfg(not(windows))]
    count(&mut text.to_string_lossy().encode_utf16())
}

fn windows_batch_shell_units() -> usize {
    // Rust currently spells argv[0] as "cmd.exe" in its batch wrapper.
    // Budget a fully qualified/quoted shell too, for Windows installations
    // and custom COMSPEC settings with longer or supplementary-character paths.
    let shell = std::env::var_os("COMSPEC").or_else(|| {
        std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("System32").join("cmd.exe").into_os_string())
    });
    shell.as_deref().map(utf16_len).unwrap_or("cmd.exe".len()).max("cmd.exe".len()) + 2
}

fn windows_command_units(command: &Command, batch: bool) -> usize {
    let program_units = if batch {
        // Rust resolves a batch script to an absolute path before wrapping it
        // with cmd.exe. The verbatim prefix allowance is deliberately retained.
        let path = Path::new(command.get_program());
        let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| {
            std::env::current_dir().unwrap_or_default().join(path)
        });
        utf16_len(absolute.as_os_str()) + 8 + windows_batch_shell_units() + " /e:ON /v:OFF /d /c \"".len() + 1
    } else {
        utf16_len(command.get_program())
    };
    // Quoted argv[0], one separating space per argument, and terminating NUL.
    program_units + 3 + command.get_args().map(|arg| 1 + windows_argument_units(arg, batch)).sum::<usize>()
}

/// Budget the prompt against the fully resolved executable and every fixed
/// flag. Prefixes are cut at UTF-8 boundaries; a cut is always visible.
fn fit_ai_prompt<'a>(command: &Command, prompt: &'a str, suffix: &[String]) -> Result<std::borrow::Cow<'a, str>, String> {
    let batch = Path::new(command.get_program()).extension().and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat"));
    let suffix_units = suffix.iter().map(|arg| 1 + windows_argument_units(arg.as_ref(), batch)).sum::<usize>();
    let limit = if batch { WINDOWS_BATCH_LIMIT } else { WINDOWS_COMMAND_LIMIT };
    let available = limit.checked_sub(windows_command_units(command, batch) + suffix_units + 1)
        .ok_or_else(|| "Команда AI превышает лимит командной строки Windows".to_owned())?;
    if windows_argument_units(prompt.as_ref(), batch) <= available {
        return Ok(prompt.into());
    }
    // Do not introduce a newline into a single-line batch argument: Rust
    // safely rejects multiline batch arguments rather than shell-escaping them.
    let separator = if batch { ' ' } else { '\n' };
    let marker_units = windows_argument_units(AI_TRUNCATED.as_ref(), batch) + 1;
    let room = available.checked_sub(marker_units)
        .ok_or_else(|| "Команда AI не оставляет места для запроса".to_owned())?;
    let mut used = 0;
    let mut backslashes = 0;
    let mut end = 0;
    for (offset, character) in prompt.char_indices() {
        let extra = if character == '"' { backslashes + 1 } else if batch && matches!(character, '%' | '\r') { 7 } else { 0 };
        let size = character.len_utf16() + extra;
        if used + size > room {
            break;
        }
        used += size;
        backslashes = if character == '\\' { backslashes + 1 } else { 0 };
        end = offset + character.len_utf8();
    }
    let mut shortened = String::with_capacity(end + 1 + AI_TRUNCATED.len());
    shortened.push_str(&prompt[..end]);
    shortened.push(separator);
    shortened.push_str(AI_TRUNCATED);
    Ok(shortened.into())
}

#[cfg(windows)]
fn opencode_executable(path: Option<&std::ffi::OsStr>, home: Option<&Path>, appdata: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = path {
        for dir in std::env::split_paths(path).filter(|dir| dir.is_absolute()) {
            let native = dir.join("opencode.exe");
            if native.is_file() {
                return Some(native);
            }
            let npm = dir.join("node_modules/opencode-ai/bin/opencode.exe");
            if npm.is_file() {
                return Some(npm);
            }
        }
    }
    // A GUI launched before installation can have an outdated inherited PATH.
    if let Some(home) = home {
        let native = home.join(".opencode/bin/opencode.exe");
        if native.is_file() {
            return Some(native);
        }
    }
    if let Some(appdata) = appdata {
        let npm = appdata.join("npm/node_modules/opencode-ai/bin/opencode.exe");
        if npm.is_file() {
            return Some(npm);
        }
    }
    None
}

/// How long the AI CLI may think before the panel gives up on it.
pub const AI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The model IDs offered by the installed OpenCode and its configured providers.
pub fn opencode_models() -> Result<Vec<String>, String> {
    let mut command = ai_cli_command("opencode");
    command.args(["models", "--pure"]).current_dir(ai_workdir());
    let (success, stdout, stderr) = run_bounded(command, "opencode models", std::time::Duration::from_secs(30), 512 * 1024, None)?;
    if !success {
        return Err(String::from_utf8_lossy(&stderr).trim().to_owned());
    }
    let models = parse_opencode_models(&String::from_utf8_lossy(&stdout));
    if models.is_empty() {
        return Err("OpenCode не вернул список моделей".to_owned());
    }
    Ok(models)
}

fn parse_opencode_models(text: &str) -> Vec<String> {
    let mut models: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.split_once('/').is_some_and(|(provider, model)| !provider.is_empty() && !model.is_empty() && !provider.contains(':'))
                && !line.chars().any(char::is_whitespace)
        })
        .map(str::to_owned)
        .collect();
    models.sort_unstable();
    models.dedup();
    models
}

/// Add only our no-tools agent; retain any caller-supplied provider settings.
fn opencode_commit_config(existing: Option<&str>) -> Result<String, String> {
    let mut config: serde_json::Value = match existing {
        Some(text) => serde_json::from_str(text).map_err(|error| format!("OPENCODE_CONFIG_CONTENT: {error}"))?,
        None => serde_json::json!({}),
    };
    let object = config.as_object_mut().ok_or_else(|| "OPENCODE_CONFIG_CONTENT: нужен JSON-объект".to_owned())?;
    let agents = object.entry("agent").or_insert_with(|| serde_json::json!({}));
    let agents = agents.as_object_mut().ok_or_else(|| "OPENCODE_CONFIG_CONTENT.agent: нужен JSON-объект".to_owned())?;
    agents.insert(
        "anvil-commit".to_owned(),
        serde_json::json!({
            "description": "Write a Git commit message from the supplied diff.",
            "mode": "primary",
            "prompt": "You write Git commit messages, not code. All repository context is already in the user prompt. Return only the full commit message based on that context. Do not inspect the filesystem, run commands, search for a repository, delegate, or perform any development workflow.",
            "permission": { "*": "deny" }
        }),
    );
    serde_json::to_string(&config).map_err(|error| error.to_string())
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OpenCodeEvent {
    Text { part: OpenCodeText },
    Error { error: serde_json::Value },
    #[serde(other)]
    Other,
}

#[derive(serde::Deserialize)]
struct OpenCodeText {
    text: String,
}

fn opencode_message(text: &str) -> Result<String, String> {
    let mut message = String::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let event: OpenCodeEvent = serde_json::from_str(line).map_err(|error| format!("OpenCode: неверный JSON-ответ ({error})"))?;
        match event {
            OpenCodeEvent::Text { part } => message.push_str(&part.text),
            OpenCodeEvent::Error { error } => {
                return Err(error
                    .pointer("/data/message")
                    .or_else(|| error.get("message"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("OpenCode вернул ошибку")
                    .to_owned());
            }
            OpenCodeEvent::Other => {}
        }
    }
    Ok(message)
}

/// An empty folder the AI CLI runs in. Inside the repository the CLI would
/// load the repository's own agent config (`.claude/settings.json` hooks and
/// env, `opencode.json` MCP servers, `.aider.conf.yml`), so pressing the
/// button in a cloned repository could run that repository's code. The diff
/// is in the prompt; the CLI needs nothing from the folder.
pub fn ai_workdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("anvil-ai");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Asks the given CLI (or the auto-detected one) for a full commit message,
/// running it in `workdir` (see `ai_workdir`) for at most `timeout`.
pub fn ai_commit_message(
    workdir: &Path,
    command: Option<&str>,
    prompt: &str,
    timeout: std::time::Duration,
) -> Result<String, String> {
    let spec = command.unwrap_or("");
    let (program, args) = ai_command(spec, "").ok_or_else(|| crate::strings::WORKSPACE_NO_AI_COMMAND.to_owned())?;
    let prompt_index = if spec.trim() == "aider" { 1 } else { args.len() - 1 };
    let mut invocation = ai_cli_command(&program);
    let opencode = program == "opencode";
    invocation.args(&args[..prompt_index]);
    if opencode {
        let config = opencode_commit_config(std::env::var("OPENCODE_CONFIG_CONTENT").ok().as_deref())?;
        invocation
            .args(["--pure", "--agent", "anvil-commit", "--format", "json", "--title", "ANVIL commit message"])
            .env("OPENCODE_CONFIG_CONTENT", config);
    }
    // Count suffix flags too: aider places them after the prompt.
    let prompt = fit_ai_prompt(&invocation, prompt, &args[prompt_index + 1..])?;
    invocation.arg(prompt.as_ref()).args(&args[prompt_index + 1..]);
    invocation.current_dir(workdir).env("GIT_TERMINAL_PROMPT", "0");
    let (success, stdout, stderr) = run_bounded(invocation, &program, timeout, 64 * 1024, None)?;
    if opencode && !success {
        let stderr = String::from_utf8_lossy(&stderr);
        return Err(if stderr.trim().is_empty() {
            opencode_message(&String::from_utf8_lossy(&stdout)).err().unwrap_or_else(|| "OpenCode завершился с ошибкой".to_owned())
        } else {
            stderr.trim().to_owned()
        });
    }
    let text = String::from_utf8_lossy(&stdout);
    let text = if opencode { opencode_message(&text)?.into() } else { text };
    let message = clean_ai_message(&text);
    if message.is_empty() {
        let stderr = String::from_utf8_lossy(&stderr);
        let fallback = crate::strings::WORKSPACE_AI_EMPTY.to_owned();
        return Err(if stderr.trim().is_empty() { fallback } else { stderr.trim().to_owned() });
    }
    Ok(message.to_owned())
}

/// Remove an optional outer CLI/Markdown wrapper, not the message's body.
fn clean_ai_message(text: &str) -> &str {
    let text = text.trim();
    let text = if text.starts_with("```") {
        text.split_once('\n')
            .and_then(|(_, body)| body.trim_end().strip_suffix("```"))
            .map(str::trim)
            .unwrap_or(text)
    } else {
        text
    };
    text.strip_prefix('"').and_then(|body| body.strip_suffix('"')).unwrap_or(text).trim()
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
    /// ` `, `-` and `+` lines, without the trailing newline (for display).
    pub lines: Vec<String>,
    /// The header and the lines exactly as git printed them, `\r` included:
    /// the patch is rebuilt from these, never from the display text.
    raw: Vec<Vec<u8>>,
}

/// One file's part of a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    /// `diff --git` / `---` / `+++` lines (for display).
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub staged: bool,
    /// The header lines as git printed them, used to rebuild a patch.
    raw_header: Vec<Vec<u8>>,
}

/// Display text of one diff line: lossy UTF-8 without the CR of a CRLF line.
fn display_line(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw.strip_suffix(b"\r").unwrap_or(raw)).into_owned()
}

/// Splits a unified diff into files and hunks. Lines are split on `\n` only:
/// `str::lines()` would also drop the `\r` of CRLF content, and lossy UTF-8
/// would replace bytes of other encodings, so a rebuilt patch no longer
/// matched the file (or, for added lines, staged altered text).
pub fn parse_diff(diff: impl AsRef<[u8]>, staged: bool) -> Vec<FileDiff> {
    let bytes = diff.as_ref();
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current: Option<FileDiff> = None;
    let mut hunk: Option<Hunk> = None;
    let flush_hunk = |file: &mut Option<FileDiff>, hunk: &mut Option<Hunk>| {
        if let (Some(file), Some(hunk)) = (file.as_mut(), hunk.take()) {
            file.hunks.push(hunk);
        }
    };
    for raw in bytes.split(|byte| *byte == b'\n') {
        if bytes.is_empty() {
            break;
        }
        let line = display_line(raw);
        if line.starts_with("diff --git ") {
            flush_hunk(&mut current, &mut hunk);
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(FileDiff {
                path: path_from_diff_header(&line),
                header: vec![line],
                hunks: Vec::new(),
                staged,
                raw_header: vec![raw.to_vec()],
            });
            continue;
        }
        let Some(file) = current.as_mut() else { continue };
        if line.starts_with("@@") {
            flush_hunk(&mut current, &mut hunk);
            hunk = Some(Hunk { header: line, lines: Vec::new(), raw: vec![raw.to_vec()] });
        } else if let Some(hunk) = hunk.as_mut() {
            // "\ No newline at end of file" markers are kept: the rebuilt patch
            // must carry them or `git apply` rejects (or silently alters) the file.
            hunk.lines.push(line);
            hunk.raw.push(raw.to_vec());
        } else {
            // The b-side of the +++ line is authoritative for the file name
            // (the diff --git line is ambiguous when a path contains " b/").
            if let Some(path) = line.strip_prefix("+++ b/") {
                file.path = path.to_owned();
            }
            file.header.push(line);
            file.raw_header.push(raw.to_vec());
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

/// Rebuilds a patch containing only the selected hunks (all when empty), from
/// the bytes git printed.
pub fn hunk_patch(file: &FileDiff, selection: &[usize]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut push = |line: &[u8]| {
        out.extend_from_slice(line);
        out.push(b'\n');
    };
    for line in &file.raw_header {
        push(line);
    }
    for (index, hunk) in file.hunks.iter().enumerate() {
        if !selection.is_empty() && !selection.contains(&index) {
            continue;
        }
        for line in &hunk.raw {
            push(line);
        }
    }
    out
}

/// Applies selected hunks to the index (`staged = false`) or takes them back
/// out of it (`staged = true`), by piping the rebuilt patch to `git apply`.
pub fn apply_hunks(root: &Path, file: &FileDiff, selection: &[usize], from_index: bool) -> Result<(), String> {
    let patch = hunk_patch(file, selection);
    if patch.trim_ascii().is_empty() {
        return Ok(());
    }
    let mut command = git_command(root);
    command.args(["apply", "--cached", "--whitespace=nowarn"]);
    if from_index {
        command.arg("--reverse");
    }
    command.arg("-");
    let (success, _, stderr) = run_bounded(command, "git apply", GIT_TIMEOUT, GIT_MAX_OUTPUT, Some(patch))?;
    if success {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&stderr).trim().to_owned())
    }
}

/// Fetches and prunes the remote of the current branch (git resolves it).
pub fn fetch(root: &Path) -> Result<String, String> {
    let remote = run_git(root, &["config", "--get", &format!("branch.{}.remote", current_branch(root)?)])
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
/// Appended where the prompt's status or diff was cut.
pub const AI_TRUNCATED: &str = "[… truncated]";

/// At most `limit` characters of `text`, marked when cut.
fn clip(text: &str, limit: usize) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}\n{AI_TRUNCATED}", &text[..end]).into(),
        None => text.into(),
    }
}

/// Bound the context sections independently. The final executable, flags and
/// Windows argument escaping are budgeted again immediately before invocation.
pub fn ai_prompt(status_text: &str, diff_text: &str, recent: &[String]) -> String {
    const STATUS_LIMIT: usize = 3_000;
    const DIFF_LIMIT: usize = 20_000;
    const LOG_LIMIT: usize = 2_000;
    let status_text = clip(status_text, STATUS_LIMIT);
    let diff_text = clip(diff_text, DIFF_LIMIT);
    let log = recent.join("\n");
    let log = clip(&log, LOG_LIMIT);
    [
        "Write a Git commit message for the following changes.".to_owned(),
        "Respond with the full commit message only, without surrounding quotes, code fences or commentary. Write a descriptive subject, then a blank line and a detailed body organized into bullet points for each meaningful group of changes. Explain what changed and why when the diff supports it; include important behavior changes and compatibility implications. Match the language and style of the recent commits. Do not impose a character or line limit or compress a substantial diff into one sentence. Scale the detail to the changes: be thorough for large changes, avoid padding for small ones, and do not invent facts absent from the diff.".to_owned(),
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
        let oid = "0123456789abcdef0123456789abcdef01234567";
        let raw = status_bytes(&[&format!("# branch.oid {oid}"), "# branch.head main", "# branch.upstream origin/main", "# branch.ab +2 -1"]);
        let status = parse_status(&raw);
        assert_eq!(status.branch, "main");
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.head_oid.as_deref(), Some(oid));
        assert_eq!(parse_status(&status_bytes(&["# branch.oid (initial)", "# branch.head main"])).head_oid, None);
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
        let patch = |selection: &[usize]| String::from_utf8(hunk_patch(&files[0], selection)).unwrap();
        let all = patch(&[]);
        assert!(all.contains("@@ -1,3 +1,4 @@") && all.contains("@@ -10,2 +11,2 @@"));
        let second = patch(&[1]);
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
        let patch = String::from_utf8(hunk_patch(&files[0], &[])).unwrap();
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
        // A file that does not exist yet (a new file, a rename target) right
        // in the root used to be rejected: its nearest existing ancestor is
        // the root itself.
        assert_eq!(resolve_path(root, "new.txt"), Ok(root.join("new.txt")));
        assert_eq!(resolve_path(root, "new/deeper.txt"), Ok(root.join("new").join("deeper.txt")));
    }

    #[test]
    fn ai_message_preserves_the_full_body_and_removes_outer_wrappers() {
        let message = "Describe the terminal rendering and detailed AI commit workflow without cutting the subject short\n\n\
            - Preserve blank lines and all of the generated body, including explanations longer than the old 200-character cap.\n\
            - Показывать состояние генерации до завершения CLI, не сбрасывая его при обновлении git-панели.\n\
              Keep indented continuation lines intact.\n\n\
            Compatibility: existing manually written messages remain unchanged.";
        assert_eq!(clean_ai_message(&format!("\n```text\n{message}\n```\n")), message);
        assert_eq!(clean_ai_message(&format!("\"{message}\"")), message);
        assert_eq!(clean_ai_message(" \r\n\t "), "");
        assert_eq!(clean_ai_message("```\n\n```"), "");
    }

    #[test]
    fn windows_argument_budget_counts_utf16_and_rust_escaping() {
        for (argument, expected) in [
            ("", 2),
            ("plain", 5),
            ("two words", 11),
            ("😀", 2),
            ("a\"b", 4),
            ("\\\"", 4),
            ("two words\\", 13),
            ("two words\\\\", 15),
        ] {
            assert_eq!(windows_argument_units(argument.as_ref(), false), expected, "{argument:?}");
        }
        let mut command = Command::new("C:\\Program Files\\AI😀\\cli.exe");
        command.args(["--model", "provider/model with spaces", "\\\""]);
        let expected = utf16_len(command.get_program()) + 3 + 1 + 7 + 1 + 28 + 1 + 4;
        assert_eq!(windows_command_units(&command, false), expected);
        assert_eq!(windows_argument_units("%😀".as_ref(), true), 12);
    }

    #[test]
    fn final_ai_budget_handles_exact_boundary_and_multibyte_truncation() {
        let mut command = Command::new("C:\\Program Files\\AI😀\\cli.exe");
        command.args(["run", "--model", "provider/model with spaces"]);
        let suffix = vec!["--no-auto-commits".to_owned(), "--no-git".to_owned()];
        let fixed = windows_command_units(&command, false)
            + suffix.iter().map(|arg| 1 + windows_argument_units(arg.as_ref(), false)).sum::<usize>() + 1;
        let capacity = WINDOWS_COMMAND_LIMIT - fixed;
        let exact = "x".repeat(capacity);
        assert_eq!(fit_ai_prompt(&command, &exact, &suffix).unwrap(), exact);
        let over = format!("{exact}x");
        let cut = fit_ai_prompt(&command, &over, &suffix).unwrap();
        assert!(cut.ends_with(AI_TRUNCATED));
        assert!(fixed + windows_argument_units(cut.as_ref().as_ref(), false) <= WINDOWS_COMMAND_LIMIT);
        let difficult = "😀 \\\\\\\"quoted\\\\\\\" ".repeat(20_000);
        let cut = fit_ai_prompt(&command, &difficult, &suffix).unwrap();
        assert!(cut.ends_with(AI_TRUNCATED));
        assert!(fixed + windows_argument_units(cut.as_ref().as_ref(), false) <= WINDOWS_COMMAND_LIMIT);
        assert!(difficult.starts_with(cut.strip_suffix(&format!("\n{AI_TRUNCATED}")).unwrap()));
    }

    #[test]
    fn ai_budget_rejects_fixed_flags_that_leave_no_room() {
        let mut command = Command::new("cli.exe");
        command.arg("😀".repeat(WINDOWS_COMMAND_LIMIT));
        assert!(fit_ai_prompt(&command, "prompt", &[]).is_err());
        let mut command = Command::new("cli.cmd");
        command.arg("--model");
        let prompt = "%😀\\\" ".repeat(20_000);
        let cut = fit_ai_prompt(&command, &prompt, &[]).unwrap();
        command.arg(cut.as_ref());
        assert!(windows_command_units(&command, true) <= WINDOWS_BATCH_LIMIT);
    }

    #[test]
    fn batch_ai_budget_obeys_the_shell_boundary_and_preserves_single_line_input() {
        for program in ["cli.cmd", "cli.bat"] {
            let mut command = Command::new(program);
            command.args(["--model", "provider/model with spaces"]);
            let suffix = vec!["--custom-flag".to_owned()];
            let fixed = windows_command_units(&command, true)
                + suffix.iter().map(|arg| 1 + windows_argument_units(arg.as_ref(), true)).sum::<usize>() + 1;
            let exact = "x".repeat(WINDOWS_BATCH_LIMIT - fixed - 2);
            assert_eq!(fit_ai_prompt(&command, &exact, &suffix).unwrap(), exact);
            assert_eq!(fixed + windows_argument_units(exact.as_ref(), true), WINDOWS_BATCH_LIMIT);
            let over = format!("{exact}x");
            let cut = fit_ai_prompt(&command, &over, &suffix).unwrap();
            assert!(cut.ends_with(AI_TRUNCATED));
            assert!(!cut.contains(['\r', '\n']));
            assert!(fixed + windows_argument_units(cut.as_ref().as_ref(), true) <= WINDOWS_BATCH_LIMIT);
            command.arg("x".repeat(WINDOWS_BATCH_LIMIT));
            assert!(fit_ai_prompt(&command, "prompt", &suffix).is_err());
        }
    }

    #[test]
    fn aider_does_not_confirm_everything() {
        let (program, args) = ai_command("aider", "p").unwrap();
        assert_eq!(program, "aider");
        assert!(!args.iter().any(|arg| arg == "--yes-always"), "{args:?}");
    }

    #[cfg(windows)]
    #[test]
    fn opencode_lookup_resolves_npm_without_losing_path_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("npm install with spaces");
        let second = dir.path().join("native install");
        let npm = first.join("node_modules/opencode-ai/bin/opencode.exe");
        std::fs::create_dir_all(npm.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("opencode.cmd"), "npm shim").unwrap();
        std::fs::write(&npm, []).unwrap();
        std::fs::write(second.join("opencode.exe"), []).unwrap();
        let path = std::env::join_paths([&first, &second]).unwrap();
        assert_eq!(opencode_executable(Some(&path), None, None), Some(npm));

        let native = first.join("opencode.exe");
        std::fs::write(&native, []).unwrap();
        assert_eq!(opencode_executable(Some(&path), None, None), Some(native));
    }

    #[cfg(windows)]
    #[test]
    fn opencode_lookup_finds_installs_missing_from_the_inherited_path() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("user home");
        let appdata = dir.path().join("roaming");
        let native = home.join(".opencode/bin/opencode.exe");
        let npm = appdata.join("npm/node_modules/opencode-ai/bin/opencode.exe");
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::create_dir_all(npm.parent().unwrap()).unwrap();
        std::fs::write(&native, []).unwrap();
        std::fs::write(&npm, []).unwrap();
        let path = std::env::join_paths([dir.path().join("old path")]).unwrap();
        assert_eq!(opencode_executable(Some(&path), Some(&home), Some(&appdata)), Some(native.clone()));
        std::fs::remove_file(native).unwrap();
        assert_eq!(opencode_executable(Some(&path), Some(&home), Some(&appdata)), Some(npm));
    }

    #[test]
    fn opencode_catalog_keeps_nested_model_ids_without_cli_diagnostics() {
        let models = parse_opencode_models("Available models:\nopenrouter/deepseek/deepseek-r1\ndeepseek/deepseek-flash\nhttps://example.com/error\nprovider/model with spaces\ndeepseek/deepseek-flash\n");
        assert_eq!(models, ["deepseek/deepseek-flash", "openrouter/deepseek/deepseek-r1"]);
    }

    #[test]
    fn opencode_json_response_excludes_reasoning_and_surfaces_provider_errors() {
        let response = concat!(
            "{\"type\":\"step_start\",\"part\":{\"type\":\"step-start\"}}\n",
            "{\"type\":\"reasoning\",\"part\":{\"text\":\"private reasoning\"}}\n",
            "{\"type\":\"text\",\"part\":{\"text\":\"Detailed subject\\n\\n- Preserve the complete message.\"}}\n",
            "{\"type\":\"step_finish\",\"part\":{\"tokens\":{\"input\":10}}}\n"
        );
        assert_eq!(opencode_message(response).unwrap(), "Detailed subject\n\n- Preserve the complete message.");
        assert_eq!(
            opencode_message("{\"type\":\"error\",\"error\":{\"data\":{\"message\":\"Provider authentication failed\"}}}"),
            Err("Provider authentication failed".to_owned())
        );
    }

    #[test]
    fn commit_agent_overlay_preserves_existing_provider_configuration() {
        let source = r#"{"provider":{"custom":{"options":{"baseURL":"https://example.com"}}},"agent":{"review":{"mode":"subagent"}}}"#;
        let config: serde_json::Value = serde_json::from_str(&opencode_commit_config(Some(source)).unwrap()).unwrap();
        assert_eq!(config.pointer("/provider/custom/options/baseURL").unwrap(), "https://example.com");
        assert_eq!(config.pointer("/agent/review/mode").unwrap(), "subagent");
    }
}
