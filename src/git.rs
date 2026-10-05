//! Git integration for the workspace panel. Shells out to `git` and parses its
//! plumbing output; the parsers are pure and unit-tested.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::process::run_bounded;

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

/// The branch name `git rev-parse --abbrev-ref HEAD` reports in `dir`.
///
/// Used by the standalone Claude status helper, which runs next to ANVIL
/// without a trust prompt: a guarded refusal (a network repository path) or
/// any Git failure is simply reported as no branch, never as an error.
pub fn branch_at(dir: &Path, timeout: std::time::Duration) -> Option<String> {
    // No repository means no branch; not spawning Git also keeps the status
    // line cheap in ordinary directories.
    let checked = preflight(dir).ok()?;
    checked.repository.as_ref()?;
    let text = run_git_timeout(dir, &["rev-parse", "--abbrev-ref", "HEAD"], timeout).ok()?;
    let branch = String::from_utf8_lossy(&text).trim().to_owned();
    (!branch.is_empty()).then_some(branch)
}

/// `git` in `root`, configured the same way for every call the panel makes.
/// The panel follows the shell into any folder, so the repository's own
/// config must not be able to run code on a mere poll, and the user's config
/// must not change the output the parsers read.
fn git_command(root: &Path) -> Result<Command, String> {
    let checked = preflight(root)?;
    Ok(base_git_command(root, &checked.executable))
}

fn base_git_command(root: &Path, executable: &Path) -> Command {
    let mut command = Command::new(executable);
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
    // Discovery belongs to the panel's cwd, not an inherited shell override.
    // These variables can also redirect Git to an unchecked metadata path.
    for key in [
        "GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE", "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_INDEX_FILE", "GIT_SHALLOW_FILE",
        "GIT_CONFIG", "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS", "GIT_CEILING_DIRECTORIES", "GIT_EXEC_PATH",
    ] {
        command.env_remove(key);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// Spawns `git` with prompts disabled, literal pathspecs and a hard deadline.
fn run_git_capped(root: &Path, args: &[&str], timeout: std::time::Duration, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut command = git_command(root)?;
    command.args(args);
    let label = format!("git {}", args.first().copied().unwrap_or(""));
    let (success, stdout, stderr) = run_bounded(command, &label, timeout, max_bytes, None)?;
    if success {
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
    PathResolver::new(root)?.resolve(path)
}

/// One canonical root per bounded worker operation. Each child still passes
/// the local/reparse checks; this is not a cache across filesystem mutations.
pub(crate) struct PathResolver<'a> {
    root: &'a Path,
    canonical_root: PathBuf,
}

impl<'a> PathResolver<'a> {
    pub(crate) fn new(root: &'a Path) -> Result<Self, String> {
        local_path(root)?;
        let canonical_root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        Ok(Self { root, canonical_root })
    }

    pub(crate) fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        resolve_inside(self.root, &self.canonical_root, path)
    }
}

fn resolve_inside(root: &Path, canonical_root: &Path, path: &str) -> Result<PathBuf, String> {
    let inside = crate::strings::WORKSPACE_PATH_INSIDE_REPO;
    local_path(root)?;
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
    local_path(&full)?;
    let mut probe = full.clone();
    loop {
        match std::fs::canonicalize(&probe) {
            Ok(canonical) => {
                // The nearest existing ancestor may be the root itself (a new
                // file at the top level); the target itself may not be the
                // root (a junction pointing back at it).
                let is_root = canonical == canonical_root && probe == full;
                if is_root || !canonical.starts_with(canonical_root) {
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
    preflight(cwd).ok()?.repository.as_ref().map(|repository| repository.root.clone())
}

// Budgets apply to metadata, not the number of ordinary loose object files.
const MAX_WATCHED_SOURCES: usize = 8192;
const MAX_SOURCE_ALIASES: usize = 16384;
const MAX_ALTERNATE_STORES: usize = 128;
const MAX_REPOSITORIES: usize = 256;
const MAX_METADATA_ENTRIES: usize = 4 * 1024 * 1024;
const MAX_METADATA_DEPTH: usize = 64;
const MAX_CACHED_ROOTS: usize = 128;

/// One configuration file whose stat decides whether a cached digest is stale.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ConfigSource {
    path: PathBuf,
    /// All local aliases are checked: deduplicating a target must not hide a
    /// junction/symlink replacement at a second include path.
    aliases: std::collections::HashSet<PathBuf>,
    /// Length and mtime of the file when the digest was computed; `None` when
    /// it did not exist then.
    state: Option<(u64, Option<std::time::SystemTime>)>,
}

impl ConfigSource {
    fn read(path: &Path) -> ConfigSource {
        let resolved = local_path(path);
        let state = resolved.as_ref().ok().and_then(|path| {
            std::fs::metadata(path).ok().map(|meta| (meta.len(), meta.modified().ok()))
        });
        let resolved = resolved.unwrap_or_else(|_| path.to_path_buf());
        ConfigSource { path: resolved, aliases: std::collections::HashSet::from([path.to_path_buf()]), state }
    }

    fn is_current(&self) -> bool {
        // Never follow a redirect installed since the previous snapshot, and
        // never let a differently cased spelling of the same file look stale.
        self.aliases.iter().all(|alias| local_path(alias).is_ok_and(|resolved| same_path(&resolved, &self.path)))
            && std::fs::metadata(&self.path).ok().map(|meta| (meta.len(), meta.modified().ok())) == self.state
    }
}

/// Windows paths compare case-insensitively, exactly as the filesystem does.
fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    #[cfg(windows)]
    {
        left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Configuration keys that let a repository start a program or pull in more
/// settings while the panel reads status, diff or log, stages, commits or
/// applies a patch. Everything else is routine: `push --set-upstream` and
/// `checkout -b` write `branch.*` tracking keys, `remote add` writes
/// `remote.*`, and the user's own editor or pager settings are not the
/// repository's business.
fn is_hazardous_key(key: &str, value: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let (section, rest) = key.split_once('.').unwrap_or((key.as_str(), ""));
    match section {
        // Filter commands run on status, diff and add for every selected file.
        "filter" => true,
        // Includes decide which files the configuration is read from.
        "include" | "includeif" => true,
        // Credential and SSH helpers.
        "credential" => true,
        // External diff and merge tools.
        "difftool" => true,
        // Signature programs run on commit, tag and `--show-signature`.
        "gpg" => true,
        // `submodule.<name>.update = !cmd` runs a command.
        "submodule" => rest.ends_with(".update") && value.trim_start().starts_with('!'),
        "core" => matches!(rest, "fsmonitor" | "hookspath" | "sshcommand" | "gitproxy" | "attributesfile"),
        "diff" => rest == "external" || rest.ends_with(".command") || rest.ends_with(".textconv"),
        "commit" | "tag" => matches!(rest, "gpgsign" | "gpgformat"),
        "log" => rest == "showsignature",
        "protocol" => (rest == "allow" || rest.ends_with(".allow")) && !value.eq_ignore_ascii_case("never"),
        "url" => (rest.ends_with(".insteadof") || rest.ends_with(".pushinsteadof")) && !value.is_empty(),
        "remote" => {
            rest.ends_with(".receivepack") || rest.ends_with(".uploadpack") || rest.ends_with(".proxy")
                || (rest.ends_with(".vcs") && !value.trim().is_empty())
                || ((rest.ends_with(".url") || rest.ends_with(".pushurl"))
                    && matches!(value.trim_start().split_once("::"), Some(("ext" | "fd", _))))
        }
        _ => false,
    }
}

/// The configuration the user approves: only the repository's own hazardous
/// keys, plus any file a global `includeIf` points at inside the repository.
/// Values stay opaque (they may hold credentials) and never reach the UI.
#[derive(Clone)]
pub struct RepositoryStamp {
    digest: std::sync::Arc<Vec<u8>>,
    /// Hazardous key names, for the trust prompt. No values.
    hazards: std::sync::Arc<Vec<String>>,
    /// Files whose stat invalidates a cached digest.
    sources: std::sync::Arc<Vec<ConfigSource>>,
    source_index: std::sync::Arc<HashMap<PathBuf, usize>>,
}

impl PartialEq for RepositoryStamp {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest
    }
}

impl Eq for RepositoryStamp {}

impl std::fmt::Debug for RepositoryStamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RepositoryStamp")
    }
}

impl RepositoryStamp {
    /// A repository with nothing that can run a program needs no approval.
    pub fn is_hazard_free(&self) -> bool {
        self.digest.is_empty()
    }

    pub fn hazards(&self) -> &[String] {
        &self.hazards
    }

    /// Whether the files behind this digest still look untouched.
    fn is_current(&self) -> bool {
        self.sources.iter().all(ConfigSource::is_current)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryIdentity {
    pub root: PathBuf,
    pub stamp: RepositoryStamp,
}

/// Discover metadata locally before Git may follow a gitfile, commondir or
/// configuration include. Discovery itself never starts a Git process.
pub fn repository_identity(cwd: &Path) -> Result<Option<RepositoryIdentity>, String> {
    let checked = preflight(cwd)?;
    let Some(repository) = checked.repository.as_ref() else { return Ok(None) };
    let root = repository.root.clone();
    let stamp = repository_stamp(&root)?;
    Ok(Some(RepositoryIdentity { root, stamp }))
}

/// The digest the user has to approve before repository commands run.
///
/// Recomputed only when one of the configuration files behind the previous
/// digest changed, so a poll cycle does not spawn a `git config` per request.
pub fn repository_stamp(root: &Path) -> Result<RepositoryStamp, String> {
    let checked = preflight(root)?;
    if let Some(cached) = cached_stamp(root) {
        // The cached digest may watch more than preflight alone discovers (it
        // also carries the origins Git reported). It stays valid while every
        // file behind it is untouched and no new source has appeared.
        if cached.is_current()
            && checked.sources.iter().all(|source| cached.source_index.get(&source.path)
                .is_some_and(|index| cached.sources[*index] == *source))
        {
            return Ok(cached);
        }
    }
    let stamp = read_stamp(root)?;
    store_stamp(root, &stamp);
    Ok(stamp)
}

fn read_stamp(root: &Path) -> Result<RepositoryStamp, String> {
    const CONFIG_MAX_OUTPUT: usize = 1024 * 1024;
    const CONFIG_ARGS: [&str; 6] = ["config", "--includes", "--show-origin", "--show-scope", "--null", "--list"];
    // All initialized children participate in the root approval. Capture their
    // includes and identities before reading any effective configuration.
    let deadline = std::time::Instant::now() + GIT_TIMEOUT;
    for _ in 0..3 {
        let checked = preflight(root)?;
        let mut sources = (*checked.sources).clone();
        let mut source_index: HashMap<PathBuf, usize> = sources.iter().enumerate()
            .map(|(index, source)| (source.path.clone(), index)).collect();
        let mut snapshots = Vec::new();
        let mut output_bytes = 0;
        let mut alias_count: usize = sources.iter().map(|source| source.aliases.len()).sum();
        for repository in &checked.repositories {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() { return Err(config_refusal()); }
            let mut command = base_git_command(&repository.root, &checked.executable);
            command.args(CONFIG_ARGS);
            let (success, config, _) = run_bounded(command, "git config", remaining, CONFIG_MAX_OUTPUT, None)?;
            output_bytes += config.len();
            if !success || config.len() >= CONFIG_MAX_OUTPUT || config.last() != Some(&0)
                || output_bytes > 4 * CONFIG_MAX_OUTPUT
            {
                return Err(config_refusal());
            }
            for origin in config_origins(&config, &repository.root)? {
                if source_index.contains_key(&origin) { continue; }
                if sources.len() >= MAX_WATCHED_SOURCES || alias_count >= MAX_SOURCE_ALIASES {
                    return Err(config_refusal());
                }
                alias_count += 1;
                source_index.insert(origin.clone(), sources.len());
                sources.push(ConfigSource::read(&origin));
            }
            snapshots.push((repository, config));
        }
        if !sources.iter().all(ConfigSource::is_current) { continue; }
        // An origin discovered only from Git needs a second read under its
        // already captured stat; never bless output with a post-read snapshot.
        if sources.len() != checked.sources.len() {
            let mut changed = false;
            for (repository, config) in &snapshots {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() { return Err(config_refusal()); }
                let mut command = base_git_command(&repository.root, &checked.executable);
                command.args(CONFIG_ARGS);
                let (success, confirm, _) = run_bounded(command, "git config", remaining, CONFIG_MAX_OUTPUT, None)?;
                if !success || &confirm != config { changed = true; break; }
            }
            if changed || !sources.iter().all(ConfigSource::is_current) { continue; }
        }
        let mut combined = digest_of(&[], root, std::sync::Arc::new(sources));
        for (repository, config) in snapshots {
            let child = digest_of(&config, &repository.root, Default::default());
            merge_repository_digest(&mut combined, repository, &child);
        }
        return Ok(combined);
    }
    Err(config_refusal())
}

fn merge_repository_digest(combined: &mut RepositoryStamp, repository: &RepositoryPaths, stamp: &RepositoryStamp) {
    if stamp.is_hazard_free() { return; }
    let digest = std::sync::Arc::make_mut(&mut combined.digest);
    // Length-prefix identity and content: moving the same dangerous config to
    // another initialized child must not inherit the previous child's trust.
    for part in [
        repository.root.as_os_str().as_encoded_bytes(), repository.git_dir.as_os_str().as_encoded_bytes(),
        repository.common_dir.as_os_str().as_encoded_bytes(), stamp.digest.as_slice(),
    ] {
        digest.extend_from_slice(&(part.len() as u64).to_le_bytes());
        digest.extend_from_slice(part);
    }
    let hazards = std::sync::Arc::make_mut(&mut combined.hazards);
    hazards.extend(stamp.hazards.iter().cloned());
    hazards.sort();
    hazards.dedup();
}

/// The repository-scoped configuration files Git names with `--show-origin`
/// (local/worktree scope, or any origin inside the repository), resolved for
/// the watch list. The guarded resolver also fails closed on a network path.
fn config_origins(config: &[u8], root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut origins = std::collections::HashSet::new();
    let mut tokens = config.split(|byte| *byte == 0).filter(|token| !token.is_empty());
    while let (Some(scope), Some(origin), Some(_)) = (tokens.next(), tokens.next(), tokens.next()) {
        let local = matches!(scope, b"local" | b"worktree");
        let Some(path) = origin_path(origin)? else { continue };
        let path = if path.is_absolute() { path } else { root.join(path) };
        if !local && !path_is_inside(&path, root) {
            continue;
        }
        if origins.len() >= MAX_WATCHED_SOURCES { return Err(config_refusal()); }
        origins.insert(local_path(&path)?);
    }
    Ok(origins.into_iter().collect())
}

/// Local scope and files inside the repository only: the user's own global
/// settings (where Git for Windows even ships `filter.lfs.*`) are not the
/// repository's, and changing them must not revoke approval. A global
/// `includeIf` that points at a repository file still counts, because that
/// file is repository-controlled.
fn digest_of(config: &[u8], root: &Path, sources: std::sync::Arc<Vec<ConfigSource>>) -> RepositoryStamp {
    let mut digest = Vec::new();
    let mut hazards = Vec::new();
    // Each entry contributes three NUL-separated tokens: scope, origin, and
    // `key\nvalue`; the entries themselves are not separable by NUL alone.
    let mut tokens = config.split(|byte| *byte == 0).filter(|token| !token.is_empty());
    while let (Some(scope), Some(origin), Some(key_value)) = (tokens.next(), tokens.next(), tokens.next()) {
        let (key, value) = match key_value.iter().position(|byte| *byte == b'\n') {
            Some(index) => (&key_value[..index], &key_value[index + 1..]),
            None => (key_value, &[][..]),
        };
        // Git prints `.git/config` relative to the work tree; resolve it
        // against the repository, never against the process working directory.
        let path = origin_path(origin).ok().flatten().map(|path| if path.is_absolute() { path } else { root.join(path) });
        let local = matches!(scope, b"local" | b"worktree");
        let inside = path.as_deref().is_some_and(|path| path_is_inside(path, root));
        if !local && !inside {
            continue;
        }
        if is_hazardous_key(&String::from_utf8_lossy(key), &String::from_utf8_lossy(value)) {
            digest.extend_from_slice(key);
            digest.push(b'\n');
            digest.extend_from_slice(value);
            digest.push(0);
            hazards.push(String::from_utf8_lossy(key).into_owned());
        }
    }
    hazards.sort();
    hazards.dedup();
    RepositoryStamp {
        digest: std::sync::Arc::new(digest),
        hazards: std::sync::Arc::new(hazards),
        source_index: std::sync::Arc::new(sources.iter().enumerate().map(|(index, source)| (source.path.clone(), index)).collect()),
        sources,
    }
}

/// Git C-style quoting is byte-oriented (including three-digit octal bytes).
/// A path without an opening quote is literal, backslashes included.
fn git_unquote(path: &[u8]) -> Result<Vec<u8>, String> {
    if !path.starts_with(b"\"") { return Ok(path.to_vec()); }
    let mut decoded = Vec::with_capacity(path.len());
    let mut index = 1;
    while let Some(&byte) = path.get(index) {
        index += 1;
        match byte {
            b'"' if index == path.len() => return Ok(decoded),
            b'"' => return Err(config_refusal()),
            b'\\' => {
                let escaped = *path.get(index).ok_or_else(config_refusal)?;
                index += 1;
                decoded.push(match escaped {
                    b'a' => 7, b'b' => 8, b't' => b'\t', b'n' => b'\n',
                    b'v' => 11, b'f' => 12, b'r' => b'\r', b'\\' => b'\\', b'"' => b'"',
                    b'0'..=b'3' => {
                        let second = *path.get(index).ok_or_else(config_refusal)?;
                        let third = *path.get(index + 1).ok_or_else(config_refusal)?;
                        if !(b'0'..=b'7').contains(&second) || !(b'0'..=b'7').contains(&third) {
                            return Err(config_refusal());
                        }
                        index += 2;
                        (escaped - b'0') * 64 + (second - b'0') * 8 + (third - b'0')
                    }
                    _ => return Err(config_refusal()),
                });
            }
            _ => decoded.push(byte),
        }
    }
    Err(config_refusal())
}

fn bytes_path(bytes: Vec<u8>) -> Result<PathBuf, String> {
    if bytes.is_empty() || bytes.contains(&0) { return Err(config_refusal()); }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(String::from_utf8(bytes).map_err(|_| config_refusal())?))
    }
}

/// A malformed file origin is not an ignorable, non-file origin.
fn origin_path(origin: &[u8]) -> Result<Option<PathBuf>, String> {
    let Some(path) = origin.strip_prefix(b"file:") else { return Ok(None) };
    let path = bytes_path(git_unquote(path)?)?;
    reject_remote_path(&path)?;
    Ok(Some(path))
}

/// Case-insensitive prefix test on the platform's own separators.
fn path_is_inside(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().replace('\\', "/").to_lowercase();
    let mut root = root.to_string_lossy().replace('\\', "/").to_lowercase();
    while root.ends_with('/') {
        root.pop();
    }
    !root.is_empty() && (path == root || path.starts_with(&format!("{root}/")))
}

fn config_refusal() -> String {
    "Не удалось безопасно прочитать конфигурацию Git; доступ к репозиторию не разрешён.".to_owned()
}

fn network_refusal() -> String {
    "Git-панель не открывает сетевые репозитории и сетевые пути конфигурации.".to_owned()
}

/// Lexical rejection happens before any filesystem call. In particular, do
/// not canonicalize a UNC path to find out whether it is a network path.
fn reject_remote_path(path: &Path) -> Result<(), String> {
    let text = path.to_string_lossy().replace('\\', "/");
    let lower = text.to_ascii_lowercase();
    let ordinary_verbatim = lower.starts_with("//?/") && lower.as_bytes().get(5) == Some(&b':');
    let text = if ordinary_verbatim { &text[4..] } else { &text };
    if text.len() > 32767 { return Err(config_refusal()); }
    if text.starts_with("//") || lower.starts_with("/??/") || lower.starts_with("/device/")
        || text.chars().any(char::is_control) || text.contains("://")
        || text.char_indices().any(|(index, character)| character == ':' && index != 1)
    {
        return Err(network_refusal());
    }
    #[cfg(windows)]
    for part in text.split('/') {
        let stem = part.trim_end_matches(['.', ' ']).split('.').next().unwrap_or("");
        let numbered = stem.len() == 4
            && stem.get(..3).is_some_and(|head| head.eq_ignore_ascii_case("COM") || head.eq_ignore_ascii_case("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9');
        if numbered || ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].iter().any(|name| stem.eq_ignore_ascii_case(name)) {
            return Err(network_refusal());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
        // Win32 DRIVE_REMOTE; the windows-sys 0.59 binding lives behind an
        // unrelated feature gate, and the value is part of the stable API.
        const DRIVE_REMOTE: u32 = 4;
        if text.as_bytes().get(1) == Some(&b':') {
            if text.as_bytes().get(2) != Some(&b'/') {
                return Err(network_refusal()); // drive-relative paths depend on hidden cwd state
            }
            let mut drive: Vec<u16> = std::ffi::OsStr::new(&text[..3]).encode_wide().collect();
            drive.push(0);
            if unsafe { GetDriveTypeW(drive.as_ptr()) } == DRIVE_REMOTE {
                return Err(network_refusal());
            }
        }
    }
    Ok(())
}

/// Resolve one component at a time. read_link reads the LOCAL reparse entry,
/// not its target; reject that target before inspecting the next component.
fn local_path(path: &Path) -> Result<PathBuf, String> {
    reject_remote_path(path)?;
    let mut path = path.to_path_buf();
    #[cfg(windows)]
    {
        let text = path.to_string_lossy().replace('\\', "/");
        if let Some(plain) = text.strip_prefix("//?/") {
            path = PathBuf::from(plain);
        }
    }
    if !path.is_absolute() {
        path = std::env::current_dir().map_err(|e| e.to_string())?.join(path);
    }
    for _ in 0..32 {
        reject_remote_path(&path)?;
        let mut components = path.components();
        let mut probe = PathBuf::new();
        let mut redirected = None;
        while let Some(component) = components.next() {
            match component {
                std::path::Component::CurDir => continue,
                std::path::Component::Prefix(_) => { probe.push(component.as_os_str()); continue; }
                std::path::Component::ParentDir => { probe.pop(); continue; }
                _ => probe.push(component.as_os_str()),
            }
            match std::fs::symlink_metadata(&probe) {
                Ok(meta) => {
                    #[cfg(windows)]
                    let reparse = {
                        use std::os::windows::fs::MetadataExt;
                        meta.file_attributes() & 0x400 != 0
                    };
                    #[cfg(not(windows))]
                    let reparse = meta.file_type().is_symlink();
                    if reparse {
                        // Junctions and symlinks are inspectable, and their
                        // targets are rejected before anything follows them.
                        // A vendor filter (OneDrive, WCI, app aliases) cannot be
                        // given an attacker-chosen target by unprivileged code,
                        // so an unreadable link keeps resolving lexically
                        // instead of refusing the whole folder.
                        if let Ok(target) = std::fs::read_link(&probe) {
                            reject_remote_path(&target)?;
                            let mut target = if target.is_absolute() {
                                target
                            } else {
                                probe.parent().ok_or_else(network_refusal)?.join(target)
                            };
                            for rest in components {
                                target.push(rest.as_os_str());
                            }
                            redirected = Some(target);
                            break;
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        match redirected {
            Some(target) => {
                #[cfg(windows)]
                let target = {
                    let text = target.to_string_lossy().replace('\\', "/");
                    PathBuf::from(text.strip_prefix("//?/").unwrap_or(&text))
                };
                path = target;
            }
            None => return Ok(probe),
        }
    }
    Err(config_refusal())
}

#[derive(Clone)]
struct RepositoryPaths {
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

#[derive(Clone)]
struct Preflight {
    repository: Option<RepositoryPaths>,
    repositories: Vec<RepositoryPaths>,
    sources: std::sync::Arc<Vec<ConfigSource>>,
    executable: PathBuf,
}

struct ConfigScan {
    sources: Vec<ConfigSource>,
    source_index: HashMap<PathBuf, usize>,
    alias_index: HashMap<PathBuf, usize>,
    repositories: Vec<RepositoryPaths>,
    repository_keys: std::collections::HashSet<(PathBuf, PathBuf)>,
    metadata_dirs: std::collections::HashSet<PathBuf>,
    alternate_stores: std::collections::HashSet<PathBuf>,
    metadata_entries: usize,
    visited: std::collections::HashSet<(PathBuf, PathBuf, PathBuf)>,
    global_paths: std::sync::Arc<Vec<PathBuf>>,
    bytes: usize,
    cwd: PathBuf,
    active: std::collections::HashSet<PathBuf>,
    worktree_override: bool,
    git_dir: PathBuf,
}

impl ConfigScan {
    fn new(cwd: &Path) -> Self {
        Self {
            sources: Vec::new(), visited: Default::default(), bytes: 0,
            cwd: cwd.to_path_buf(), active: Default::default(), worktree_override: false,
            git_dir: cwd.to_path_buf(),
            source_index: Default::default(), alias_index: Default::default(),
            repositories: Vec::new(), repository_keys: Default::default(),
            metadata_dirs: Default::default(), alternate_stores: Default::default(), metadata_entries: 0,
            global_paths: Default::default(),
        }
    }

    fn source(&mut self, path: &Path) -> Result<PathBuf, String> {
        if let Some(&index) = self.alias_index.get(path) { return Ok(self.sources[index].path.clone()); }
        if self.alias_index.len() >= MAX_SOURCE_ALIASES { return Err(config_refusal()); }
        let resolved = local_path(path)?;
        let index = if let Some(&index) = self.source_index.get(&resolved) {
            self.sources[index].aliases.insert(path.to_path_buf());
            index
        } else {
            if self.sources.len() >= MAX_WATCHED_SOURCES { return Err(config_refusal()); }
            let index = self.sources.len();
            // Record the stat BEFORE reading, including absent metadata.
            let state = std::fs::metadata(&resolved).ok().map(|meta| (meta.len(), meta.modified().ok()));
            self.sources.push(ConfigSource {
                path: resolved.clone(), aliases: std::collections::HashSet::from([path.to_path_buf()]), state,
            });
            self.source_index.insert(resolved.clone(), index);
            index
        };
        self.alias_index.insert(path.to_path_buf(), index);
        Ok(resolved)
    }

    fn read(&mut self, path: &Path) -> Result<Option<String>, String> {
        let resolved = self.source(path)?;
        let meta = match std::fs::metadata(&resolved) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if !meta.is_file() || meta.len() > 1024 * 1024 {
            return Err(config_refusal());
        }
        self.bytes += meta.len() as usize;
        if self.bytes > 4 * 1024 * 1024 {
            return Err(config_refusal());
        }
        // A growing file is bounded too, not just its initial metadata.
        use std::io::Read;
        let mut text = String::new();
        std::fs::File::open(resolved).map_err(|e| e.to_string())?
            .take(1024 * 1024 + 1).read_to_string(&mut text).map_err(|_| config_refusal())?;
        if text.len() > 1024 * 1024 {
            return Err(config_refusal());
        }
        Ok(Some(text))
    }

    fn config(&mut self, path: &Path, depth: usize) -> Result<(), String> {
        let resolved = self.source(path)?;
        if depth > 10 { return Err(config_refusal()); }
        if self.active.contains(&resolved) { return Err(config_refusal()); }
        let context = (resolved.clone(), self.cwd.clone(), self.git_dir.clone());
        if self.visited.contains(&context) { return Ok(()); }
        if self.visited.len() >= 1024 { return Err(config_refusal()); }
        self.visited.insert(context);
        self.active.insert(resolved.clone());
        let Some(text) = self.read(path)? else {
            self.active.remove(&resolved);
            return Ok(());
        };
        for (key, value) in config_paths(&text)? {
            if key == "submodule.path" { continue; }
            if key == "core.worktree" { self.worktree_override = true; }
            let parent = if key.starts_with("include") {
                path.parent().ok_or_else(config_refusal)?
            } else if key == "core.worktree" {
                &self.git_dir
            } else {
                &self.cwd
            };
            let target = config_path(&value, parent, &self.cwd)?;
            if !key.starts_with("include") {
                self.source(&target)?;
            } else {
                // Inspect conditional includes conservatively too: an absent
                // target must be watched before Git ever prints its origin.
                self.config(&target, depth + 1)?;
            }
        }
        self.active.remove(&resolved);
        Ok(())
    }
}

/// Decode the path-bearing subset with Git's quoting/escape rules, but parse
/// every line so an ambiguous/malformed config fails closed before Git runs.
fn config_paths(text: &str) -> Result<Vec<(String, String)>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
    // Split Git's physical lines into logical ones: a backslash that is not
    // itself escaped continues the value on the next line; comments run to the
    // end of the line and may contain either kind of quote.
    let mut logical: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut characters = text.chars();
    let mut comment = false;
    let mut quoted = false;
    while let Some(character) = characters.next() {
        if comment {
            if character == '\n' { comment = false; logical.push(std::mem::take(&mut current)); }
            continue;
        }
        match character {
            '\n' => {
                if quoted { return Err(config_refusal()); }
                logical.push(std::mem::take(&mut current));
            }
            '\\' => {
                let next = characters.next().ok_or_else(config_refusal)?;
                if next != '\n' {
                    current.push('\\');
                    current.push(next);
                }
            }
            // Git ends an unquoted value at `#`/`;`; inside quotes they are
            // literal characters (that is why Git itself quotes such values).
            '#' | ';' if !quoted => comment = true,
            '"' => {
                quoted = !quoted;
                current.push(character);
            }
            _ => current.push(character),
        }
    }
    if quoted { return Err(config_refusal()); }
    if !current.is_empty() { logical.push(current); }
    let mut section = String::new();
    let mut paths = Vec::new();
    for line in logical {
        let mut line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) { continue; }
        if let Some(header) = line.strip_prefix('[') {
            // A ] within a quoted subsection is not the section terminator.
            let mut quoted = false;
            let mut escaped = false;
            let end = header.char_indices().find_map(|(index, character)| {
                if escaped { escaped = false; return None; }
                if character == '\\' { escaped = true; return None; }
                if character == '"' { quoted = !quoted; }
                (character == ']' && !quoted).then_some(index)
            }).ok_or_else(config_refusal)?;
            let rest = header[end + 1..].trim();
            section = header[..end].split([' ', '\t', '.']).next().unwrap_or("").to_ascii_lowercase();
            if section.is_empty() { return Err(config_refusal()); }
            line = rest;
            if line.is_empty() { continue; }
        }
        let key_end = line.find(|c: char| c == '=' || c.is_whitespace()).unwrap_or(line.len());
        let name = &line[..key_end];
        if section.is_empty() || name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(config_refusal());
        }
        let rest = line[key_end..].trim_start();
        let value = if let Some(value) = rest.strip_prefix('=') {
            config_value(value)?
        } else if rest.is_empty() {
            String::new()
        } else {
            return Err(config_refusal());
        };
        let key = format!("{section}.{}", name.to_ascii_lowercase());
        if matches!(key.as_str(), "include.path" | "includeif.path" | "core.attributesfile" | "core.excludesfile" | "core.worktree" | "submodule.path") {
            if value.is_empty() {
                // An empty include target is nonsense; an empty `core.*`
                // path or submodule path means the key is unset.
                if key.starts_with("include") { return Err(config_refusal()); }
                continue;
            }
            paths.push((key, value));
        }
    }
    Ok(paths)
}

/// Git unquotes an alternates entry only when it starts with a quote; an
/// unquoted entry is a literal path, backslashes included.
fn alternates_path(line: &str) -> Result<std::ffi::OsString, String> {
    let path = bytes_path(git_unquote(line.as_bytes())?)?;
    reject_remote_path(&path)?;
    Ok(path.into_os_string())
}

fn config_value(value: &str) -> Result<String, String> {
    let mut result = String::new();
    let mut quoted = false;
    let mut characters = value.trim_start().chars();
    let mut kept = 0;
    while let Some(character) = characters.next() {
        match character {
            '"' => quoted = !quoted,
            '#' | ';' if !quoted => break,
            '\\' => {
                result.push(match characters.next().ok_or_else(config_refusal)? {
                    'n' => '\n', 't' => '\t', 'b' => '\u{8}', '"' => '"', '\\' => '\\',
                    _ => return Err(config_refusal()),
                });
                kept = result.len();
            }
            _ => {
                result.push(character);
                if quoted || !character.is_whitespace() { kept = result.len(); }
            }
        }
    }
    if quoted { return Err(config_refusal()); }
    result.truncate(kept);
    Ok(result)
}

fn home_path() -> Result<PathBuf, String> {
    // Git reads `$HOME` first, then falls back to `%HOMEDRIVE%%HOMEPATH%`
    // (and only then to USERPROFILE) on Windows.
    let home = match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => home,
        _ => match (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH")) {
            (Some(drive), Some(path)) if !path.is_empty() => {
                let mut home = PathBuf::from(drive);
                home.push(path);
                home.into_os_string()
            }
            _ => std::env::var_os("USERPROFILE").ok_or_else(config_refusal)?,
        },
    };
    local_path(Path::new(&home))
}

/// Installation root: `git` under `cmd`, `bin` or `mingw64/bin` all belong to
/// the same root that holds `etc/gitconfig`.
#[cfg(windows)]
fn git_install_root(executable: &Path) -> Result<PathBuf, String> {
    let mut root = executable.parent().ok_or_else(config_refusal)?.to_path_buf();
    for _ in 0..2 {
        let name = root.file_name().and_then(|name| name.to_str()).map(str::to_ascii_lowercase);
        match name.as_deref() {
            Some("cmd" | "bin") => root = root.parent().ok_or_else(config_refusal)?.to_path_buf(),
            Some("mingw64" | "mingw32") => root = root.parent().ok_or_else(config_refusal)?.to_path_buf(),
            _ => break,
        }
    }
    Ok(root)
}

/// The prefix `%(prefix)` expands to: the `mingw64`/`mingw32` subtree that
/// contains the running `git.exe`, else the installation root itself.
fn git_prefix(executable: &Path) -> Result<PathBuf, String> {
    let parent = executable.parent().ok_or_else(config_refusal)?;
    let base = match parent.file_name().and_then(|name| name.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("cmd" | "bin" | "mingw64" | "mingw32") => parent.parent().ok_or_else(config_refusal)?,
        _ => parent,
    };
    for architecture in ["mingw64", "mingw32"] {
        let candidate = base.join(architecture);
        #[cfg(windows)]
        let binary = candidate.join("bin/git.exe");
        #[cfg(not(windows))]
        let binary = candidate.join("bin/git");
        if local_path(&binary).is_ok_and(|binary| binary.is_file()) { return Ok(candidate); }
    }
    Ok(base.to_path_buf())
}

/// Resolve a config path value against its anchor. On Windows a leading
/// separator means the current drive's root, exactly as Git for Windows
/// resolves it, not the process working directory.
fn anchored_path(anchor: &Path, value: &Path) -> Result<PathBuf, String> {
    if value.is_absolute() { return Ok(value.to_path_buf()); }
    let text = value.as_os_str().to_string_lossy();
    if text.starts_with(['/', '\\']) {
        let mut base = PathBuf::new();
        match anchor.components().next() {
            Some(prefix @ std::path::Component::Prefix(_)) => base.push(prefix.as_os_str()),
            _ => return Err(config_refusal()),
        }
        base.push(text.trim_start_matches(['/', '\\']));
        return Ok(base);
    }
    Ok(anchor.join(value))
}

fn config_path(value: &str, parent: &Path, anchor: &Path) -> Result<PathBuf, String> {
    reject_remote_path(Path::new(value))?;
    let path = if let Some(relative) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        home_path()?.join(relative)
    } else if let Some(relative) = value.strip_prefix("%(prefix)/") {
        git_prefix(&git_executable()?)?.join(relative)
    } else if value.starts_with('~') {
        return Err(config_refusal()); // ~user cannot be resolved safely by the panel
    } else if value.contains("%(") {
        return Err(config_refusal());
    } else {
        let path = PathBuf::from(value);
        if path.is_absolute() || value.starts_with(['/', '\\']) {
            anchored_path(anchor, &path)?
        } else {
            parent.join(path)
        }
    };
    local_path(&path)?;
    Ok(path)
}

fn git_executable() -> Result<PathBuf, String> {
    let path = std::env::var_os("PATH").ok_or_else(config_refusal)?;
    for directory in std::env::split_paths(&path) {
        #[cfg(windows)]
        let candidate = directory.join("git.exe");
        #[cfg(not(windows))]
        let candidate = directory.join("git");
        // A PATH entry that is not a plain local path (a UNC share, a
        // device path) is never a source of executables for the panel.
        let Ok(candidate) = local_path(&candidate) else { continue };
        if candidate.is_file() { return Ok(candidate); }
    }
    Err("Не найден Git.".to_owned())
}

fn global_configs(executable: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let no_system = std::env::var("GIT_CONFIG_NOSYSTEM").unwrap_or_default().to_ascii_lowercase();
    if !matches!(no_system.as_str(), "1" | "true" | "yes" | "on") {
        if let Some(system) = std::env::var_os("GIT_CONFIG_SYSTEM") {
            if !system.is_empty() { paths.push(PathBuf::from(system)); }
        } else {
            #[cfg(not(windows))]
            {
                paths.push(PathBuf::from("/etc/gitconfig"));
                paths.push(git_prefix(executable)?.join("etc/gitconfig"));
            }
            #[cfg(windows)]
            {
                if let Some(program_data) = std::env::var_os("PROGRAMDATA") {
                    paths.push(PathBuf::from(program_data).join("Git/config"));
                }
                // Git reads `<install root>/etc/gitconfig`; the root holds
                // `cmd`, `bin` or `mingw64/bin`, and MSYS layouts use the
                // prefix's own `etc` instead.
                let root = git_install_root(executable)?;
                paths.push(root.join("etc/gitconfig"));
                for architecture in ["mingw64", "mingw32"] {
                    paths.push(root.join(architecture).join("etc/gitconfig"));
                }
            }
        }
    }
    if let Some(global) = std::env::var_os("GIT_CONFIG_GLOBAL") {
        if !global.is_empty() { paths.push(PathBuf::from(global)); }
    } else {
        let home = home_path()?;
        paths.push(home.join(".gitconfig"));
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        paths.push(xdg.join("git/config"));
    }
    Ok(paths)
}

/// Resolve a candidate git directory exactly as Git's `is_git_directory()`
/// does: `HEAD` lives in the directory itself, while `objects/` and `refs/`
/// live in the common directory — a `commondir` file (relative to the git
/// directory) redirects there for a linked worktree. Returns the common
/// directory when the candidate is a real git directory, `None` otherwise.
fn valid_git_dir(git_dir: &Path, scan: &mut ConfigScan) -> Result<Option<PathBuf>, String> {
    let git_dir = scan.source(git_dir)?;
    let head = scan.source(&git_dir.join("HEAD"))?;
    if !head.is_file() {
        return Ok(None);
    }
    // Git reads HEAD as well and refuses the directory when it names neither a
    // ref under `refs/` nor an object: such a `.git` is skipped and the walk
    // continues, so a stray directory cannot become ANVIL's repository.
    if !head_reference(&scan.read(&head)?.unwrap_or_default()) {
        return Ok(None);
    }
    let common_dir = match scan.read(&git_dir.join("commondir"))? {
        Some(text) => {
            let target = text.trim();
            if target.is_empty() {
                return Ok(None);
            }
            config_path(target, &git_dir, &git_dir)?
        }
        None => git_dir.clone(),
    };
    // The guarded resolver refuses a network or device object store before it
    // is ever touched.
    let objects = local_path(&common_dir.join("objects"))?;
    let refs = local_path(&common_dir.join("refs"))?;
    if objects.is_dir() && refs.is_dir() {
        Ok(Some(common_dir))
    } else {
        Ok(None)
    }
}

/// `HEAD` content git accepts, probed against git 2.54: either `ref:` followed
/// by whitespace and a name under `refs/`, or an object name — the first forty
/// bytes must be hex digits, and git ignores whatever follows them. The
/// whitespace set is exactly space, tab, newline and carriage return: vertical
/// tab and form feed are *not* skipped, a leading space before `ref:` is not
/// skipped either, and `ref: HEAD`, a short hash and an empty file are all
/// rejected — a rejected marker means "not a repository here, keep walking up".
fn head_reference(text: &str) -> bool {
    if let Some(name) = text.strip_prefix("ref:") {
        return name.trim_start_matches(is_ref_space).starts_with("refs/");
    }
    let bytes = text.as_bytes();
    bytes.len() >= 40 && bytes[..40].iter().all(u8::is_ascii_hexdigit)
}

fn is_ref_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
}

fn discover_repository(cwd: &Path, scan: &mut ConfigScan) -> Result<Option<RepositoryPaths>, String> {
    let cwd = local_path(cwd)?;
    if !cwd.is_dir() { return Ok(None); }
    for root in cwd.ancestors() {
        let dot_git = root.join(".git");
        let marker = scan.source(&dot_git)?;
        if marker.is_dir() {
            if let Some(common_dir) = valid_git_dir(&marker, scan)? {
                return Ok(Some(RepositoryPaths { root: root.to_path_buf(), git_dir: marker, common_dir }));
            }
            // An ordinary directory that merely contains a `.git` folder is
            // not a repository here: Git keeps walking up (and may still find
            // a bare repository at this level, checked below).
        } else if marker.is_file() {
            // A gitfile is authoritative. Git refuses the repository outright
            // when the file is malformed or its target is not a valid git
            // directory, rather than continuing to the parent, so fail closed
            // the same way instead of hijacking the walk.
            let text = scan.read(&dot_git)?.ok_or_else(config_refusal)?;
            let target = text.strip_prefix("gitdir:").ok_or_else(config_refusal)?.trim();
            if target.is_empty() {
                return Err(config_refusal());
            }
            let target = config_path(target, root, root)?;
            let Some(common_dir) = valid_git_dir(&target, scan)? else {
                return Err(config_refusal());
            };
            return Ok(Some(RepositoryPaths { root: root.to_path_buf(), git_dir: target, common_dir }));
        }
        // No `.git` marker (or only an invalid directory): a bare repository
        // keeps HEAD, objects and refs directly in the directory.
        if let Some(common_dir) = valid_git_dir(root, scan)? {
            return Ok(Some(RepositoryPaths { root: root.to_path_buf(), git_dir: root.to_path_buf(), common_dir }));
        }
    }
    Ok(None)
}

/// Enumerate local entries without resolving ordinary files one by one.
/// Directory snapshots catch newly inserted/replaced reparse entries, while
/// only actual redirects need a guarded target lookup and an alias watch.
fn inspect_metadata_tree(directory: &Path, scan: &mut ConfigScan, depth: usize) -> Result<(), String> {
    if depth > MAX_METADATA_DEPTH { return Err(config_refusal()); }
    let directory = scan.source(directory)?;
    if !scan.metadata_dirs.insert(directory.clone()) { return Ok(()); }
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        if scan.metadata_entries >= MAX_METADATA_ENTRIES { return Err(config_refusal()); }
        scan.metadata_entries += 1;
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            // DirEntry::metadata does not follow Windows reparse points.
            entry.metadata().map_err(|error| error.to_string())?.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = kind.is_symlink();
        if reparse {
            // Never ask is_dir/metadata about a target before local_path.
            let target = scan.source(&entry.path())?;
            if target.is_dir() { inspect_metadata_tree(&entry.path(), scan, depth + 1)?; }
        } else if kind.is_dir() {
            inspect_metadata_tree(&entry.path(), scan, depth + 1)?;
        }
    }
    Ok(())
}

fn inspect_symbolic_head(repository: &RepositoryPaths, scan: &mut ConfigScan) -> Result<(), String> {
    let mut path = repository.git_dir.join("HEAD");
    let mut visited = std::collections::HashSet::new();
    for _ in 0..32 {
        let resolved = scan.source(&path)?;
        if !visited.insert(resolved) { return Err(config_refusal()); }
        let Some(text) = scan.read(&path)? else { return Ok(()) };
        let Some(reference) = text.strip_prefix("ref:") else { return Ok(()) };
        let reference = reference.trim_matches(is_ref_space);
        if !reference.starts_with("refs/") || reference.contains('\\') || reference.contains(':')
            || reference.chars().any(char::is_control)
            || reference.split('/').any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(config_refusal());
        }
        // Git's three per-worktree namespaces bypass commondir.
        let per_worktree = ["refs/bisect/", "refs/worktree/", "refs/rewritten/"]
            .iter().any(|prefix| reference.starts_with(prefix));
        path = (if per_worktree { &repository.git_dir } else { &repository.common_dir }).join(reference);
    }
    Err(config_refusal())
}

fn inspect_objects(directory: &Path, scan: &mut ConfigScan, depth: usize) -> Result<(), String> {
    if depth > 10 { return Err(config_refusal()); }
    if scan.alternate_stores.len() >= MAX_ALTERNATE_STORES
        && !scan.alias_index.get(directory).is_some_and(|&index| scan.alternate_stores.contains(&scan.sources[index].path))
    {
        return Err(config_refusal());
    }
    let directory = scan.source(directory)?;
    if scan.alternate_stores.contains(&directory) { return Ok(()); }
    if scan.alternate_stores.len() >= MAX_ALTERNATE_STORES { return Err(config_refusal()); }
    scan.alternate_stores.insert(directory.clone());
    inspect_metadata_tree(&directory, scan, 0)?;
    if let Some(alternates) = scan.read(&directory.join("info/alternates"))? {
        for line in alternates.lines().filter(|line| !line.is_empty()) {
            let target = alternates_path(line)?;
            let target = anchored_path(&directory, Path::new(&target))?;
            inspect_objects(&target, scan, depth + 1)?;
        }
    }
    Ok(())
}

fn inspect_metadata(repository: &RepositoryPaths, scan: &mut ConfigScan, depth: usize) -> Result<(), String> {
    if depth > 10 { return Err(config_refusal()); }
    if !scan.repository_keys.insert((repository.root.clone(), repository.git_dir.clone())) { return Ok(()); }
    if scan.repositories.len() >= MAX_REPOSITORIES { return Err(config_refusal()); }
    scan.repositories.push(repository.clone());
    let previous_git_dir = std::mem::replace(&mut scan.git_dir, repository.git_dir.clone());
    let previous_cwd = std::mem::replace(&mut scan.cwd, repository.root.clone());
    // Relative global attribute/include settings apply in each child's cwd,
    // not just the superproject's. Context-keyed visited entries preserve this.
    let globals = scan.global_paths.clone();
    for path in globals.iter() { scan.config(path, 0)?; }
    scan.config(&repository.common_dir.join("config"), 0)?;
    scan.config(&repository.git_dir.join("config.worktree"), 0)?;
    // Watch HEAD as well: includeIf.onbranch can change its selected source
    // without changing a config file.
    inspect_symbolic_head(repository, scan)?;
    inspect_objects(&repository.common_dir.join("objects"), scan, 0)?;
    for directory in [
        repository.common_dir.join("refs"), repository.git_dir.join("refs"),
        repository.common_dir.join("reftable"), repository.git_dir.join("reftable"),
    ] {
        inspect_metadata_tree(&directory, scan, 0)?;
    }
    for path in [
        repository.git_dir.join("index"),
        repository.common_dir.join("refs"),
        repository.common_dir.join("packed-refs"),
        repository.common_dir.join("info/attributes"),
        repository.common_dir.join("shallow"),
        repository.git_dir.join("shallow"),
    ] {
        scan.source(&path)?;
    }
    // Status/fetch can descend into initialized submodules without an
    // explicit request for their identity. Inspect their gitfiles FIRST too.
    if let Some(modules) = scan.read(&repository.root.join(".gitmodules"))? {
        for (key, value) in config_paths(&modules)? {
            if key != "submodule.path" { continue; }
            let module = config_path(&value, &repository.root, &repository.root)?;
            if !path_is_inside(&module, &repository.root) { return Err(config_refusal()); }
            let marker = scan.source(&module.join(".git"))?;
            if marker.exists() {
                if let Some(child) = discover_repository(&module, scan)? {
                    if !same_path(&child.root, &local_path(&module)?) { return Err(config_refusal()); }
                    inspect_metadata(&child, scan, depth + 1)?;
                }
            }
        }
    }
    scan.git_dir = previous_git_dir;
    scan.cwd = previous_cwd;
    Ok(())
}

fn head_is_unborn(root: &Path) -> Result<bool, String> {
    let checked = preflight(root)?;
    let repository = checked.repository.as_ref().ok_or_else(config_refusal)?;
    let mut scan = ConfigScan::new(root);
    let Some(head) = scan.read(&repository.git_dir.join("HEAD"))? else { return Ok(false) };
    let Some(reference) = head.trim().strip_prefix("ref: ") else { return Ok(false) };
    if !reference.starts_with("refs/heads/") || reference.contains('\\')
        || reference.split('/').any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(config_refusal());
    }
    let reference_path = scan.source(&repository.common_dir.join(reference))?;
    if reference_path.exists() { return Ok(false); }
    // Reftable repositories store refs differently; leave their resolution
    // to Git, rather than treating them as empty.
    if scan.source(&repository.common_dir.join("reftable"))?.exists() { return Ok(false); }
    let packed = scan.read(&repository.common_dir.join("packed-refs"))?.unwrap_or_default();
    let packed_has_head = packed.lines().any(|line| line.split_once(' ').is_some_and(|(_, name)| name == reference));
    if !scan.sources.iter().all(ConfigSource::is_current) { return Err(config_refusal()); }
    Ok(!packed_has_head)
}

type PreflightEnvironment = [Option<std::ffi::OsString>; 10];
type PreflightCache = HashMap<PathBuf, (PreflightEnvironment, std::sync::Arc<Preflight>)>;
static PREFLIGHT_CACHE: std::sync::Mutex<Option<PreflightCache>> = std::sync::Mutex::new(None);

fn preflight(cwd: &Path) -> Result<std::sync::Arc<Preflight>, String> {
    // Env redirects are part of freshness, not just the files already found.
    let environment: PreflightEnvironment = [
        "PATH", "HOME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "XDG_CONFIG_HOME", "PROGRAMDATA",
        "GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM", "GIT_CONFIG_NOSYSTEM",
    ].map(std::env::var_os);
    let cwd = local_path(cwd)?;
    let cached = PREFLIGHT_CACHE.lock().ok().and_then(|cache| cache.as_ref()
        .and_then(|cache| cache.get(&cwd)).cloned());
    if let Some((previous, checked)) = cached {
        if previous == environment && checked.sources.iter().all(ConfigSource::is_current) {
            return Ok(checked);
        }
    }
    for _ in 0..3 {
        let mut scan = ConfigScan::new(&cwd);
        let mut repository = discover_repository(&cwd, &mut scan)?;
        let executable = git_executable()?;
        if let Some(repository) = &repository { scan.git_dir = repository.git_dir.clone(); }
        if let Some(repository) = &repository { scan.cwd = repository.root.clone(); }
        scan.global_paths = std::sync::Arc::new(global_configs(&executable)?);
        let globals = scan.global_paths.clone();
        for path in globals.iter() { scan.config(path, 0)?; }
        // Git's default external attribute/exclude files are path-bearing
        // configuration too, even with an explicit GIT_CONFIG_GLOBAL.
        let home = home_path()?;
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        scan.source(&xdg.join("git/attributes"))?;
        scan.source(&xdg.join("git/ignore"))?;
        if let Some(repository) = &repository {
            inspect_metadata(repository, &mut scan, 0)?;
        }
        if !scan.sources.iter().all(ConfigSource::is_current) { continue; }
        if scan.worktree_override {
            let mut command = base_git_command(&cwd, &executable);
            command.args(["rev-parse", "--show-toplevel"]);
            let (success, stdout, stderr) = run_bounded(command, "git rev-parse", GIT_TIMEOUT, GIT_MAX_OUTPUT, None)?;
            if !success { return Err(String::from_utf8_lossy(&stderr).trim().to_owned()); }
            let actual = PathBuf::from(String::from_utf8(stdout).map_err(|_| config_refusal())?.trim());
            if let Some(repository) = &mut repository {
                repository.root = local_path(&actual)?;
                if let Some(first) = scan.repositories.first_mut() { first.root = repository.root.clone(); }
            }
        }
        if scan.sources.iter().all(ConfigSource::is_current) {
            let checked = std::sync::Arc::new(Preflight {
                repository, repositories: scan.repositories, sources: std::sync::Arc::new(scan.sources), executable,
            });
            if let Ok(mut cache) = PREFLIGHT_CACHE.lock() {
                let cache = cache.get_or_insert_with(HashMap::new);
                if cache.len() >= MAX_CACHED_ROOTS && !cache.contains_key(&cwd) { cache.clear(); }
                cache.insert(cwd.clone(), (environment, checked.clone()));
            }
            return Ok(checked);
        }
    }
    Err(config_refusal())
}

/// A guarded worktree-specific metadata file (not the common worktree index).
pub fn metadata_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." {
        return Err(config_refusal());
    }
    let checked = preflight(root)?;
    let repository = checked.repository.as_ref().ok_or_else(config_refusal)?;
    let path = repository.git_dir.join(name);
    local_path(&path)?;
    Ok(path)
}


static STAMP_CACHE: std::sync::Mutex<Option<HashMap<PathBuf, RepositoryStamp>>> = std::sync::Mutex::new(None);

fn cached_stamp(root: &Path) -> Option<RepositoryStamp> {
    STAMP_CACHE.lock().ok()?.as_ref()?.get(root).cloned()
}

fn store_stamp(root: &Path, stamp: &RepositoryStamp) {
    if let Ok(mut cache) = STAMP_CACHE.lock() {
        let cache = cache.get_or_insert_with(HashMap::new);
        if cache.len() >= MAX_CACHED_ROOTS && !cache.contains_key(root) { cache.clear(); }
        cache.insert(root.to_path_buf(), stamp.clone());
    }
}

/// Repositories trusted in this process, keyed by root and digest: every pane
/// and tab shares them, so a split or a restored panel does not ask again for
/// the same repository. Approvals live in memory only, so a window opened with
/// Ctrl+Shift+N — a separate process — starts with none and asks again.
static TRUSTED: std::sync::Mutex<Option<HashMap<PathBuf, RepositoryStamp>>> = std::sync::Mutex::new(None);

/// A hazard-free repository always runs; a hazardous one needs a remembered
/// approval for exactly this digest, and a stale stamp never authorizes
/// commands even when the digest still matches what was remembered.
pub fn trust_approved(root: &Path, stamp: &RepositoryStamp) -> bool {
    if !stamp.is_current() { return false; }
    if stamp.is_hazard_free() {
        return true;
    }
    TRUSTED
        .lock()
        .map(|trusted| trusted.as_ref().is_some_and(|trusted| trusted.get(root).is_some_and(|approved| approved == stamp)))
        .unwrap_or(false)
}

pub fn remember_trust(root: &Path, stamp: RepositoryStamp) {
    if let Ok(mut trusted) = TRUSTED.lock() {
        trusted.get_or_insert_with(HashMap::new).insert(root.to_path_buf(), stamp);
    }
}


/// Reads branch, ahead/behind and every change (staged, unstaged, untracked).
pub fn status(root: &Path) -> Result<Status, String> {
    let raw = run_git_bytes(root, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])?;
    let mut status = parse_status(&raw);
    if status.head_oid.is_some() || status.upstream.is_some() {
        // Match log's remote/<branch> fallback without making a branch name
        // an option or interpreting it as a revision expression.
        let reference: std::borrow::Cow<'_, str> = if status.upstream.is_some() {
            "@{upstream}^{commit}".into()
        } else {
            let remote = branch_remote(root, &status.branch, false)?;
            format!("refs/remotes/{remote}/{}^{{commit}}", status.branch).into()
        };
        status.upstream_oid = run_git(root, &["rev-parse", "--verify", "--quiet", "--end-of-options", &reference])
            .ok()
            .map(|oid| oid.trim().to_owned())
            .filter(|oid| is_object_hash(oid));
    }
    if status.changes.iter().any(|change| change.unstaged() && !change.untracked) {
        if let Ok(numstat) = run_git(root, &["diff", "--no-ext-diff", "--no-textconv", "--numstat", "-z", "--no-renames"]) {
            apply_numstat(&mut status.changes, &parse_numstat(numstat.as_bytes()), false);
        }
    }
    if status.changes.iter().any(Change::staged) {
        if let Ok(numstat) = run_git(root, &["diff", "--no-ext-diff", "--no-textconv", "--cached", "--numstat", "-z", "--no-renames"]) {
            apply_numstat(&mut status.changes, &parse_numstat(numstat.as_bytes()), true);
        }
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
    let remote = branch_remote(root, branch, false).ok()?;
    if remote == "." {
        return None;
    }
    run_git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{remote}/{branch}")]).ok()?;
    Some(format!("{remote}/{branch}"))
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
    if head_is_unborn(root)? { return Ok(CommitLog::default()); }
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
        format!("--max-count={}", MAX_COMMITS + 1),
        "--topo-order".to_owned(),
        format!("--pretty=format:{format}"),
    ];
    args.extend(revs.iter().map(|rev| (*rev).to_owned()));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let text = run_git(root, &arg_refs)?;
    let mut commits = parse_log(&text, &outgoing, &incoming);
    let truncated = commits.len() > MAX_COMMITS;
    commits.truncate(MAX_COMMITS);
    Ok(CommitLog { commits, upstream, truncated })
}

/// Parses `git log` records into commits. A commit subject may itself contain
/// the record/field separators; only records whose hash is a real object id are
/// trusted, so a crafted subject cannot inject a "hash" that later reaches git
/// as an option.
fn parse_log(text: &str, outgoing: &std::collections::HashSet<String>, incoming: &std::collections::HashSet<String>) -> Vec<Commit> {
    let mut commits = Vec::new();
    for record in text.split('\u{1e}') {
        let record = record.trim_start_matches(['\n', '\r']);
        if record.is_empty() {
            continue;
        }
        let fields: Vec<&str> = record.split('\u{1f}').collect();
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
    commits
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
    let mut git = git_command(root)?;
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
    let mut command = git_command(root)?;
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

#[path = "ai_commit.rs"]
mod ai_commit;

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
    #[cfg(windows)]
    {
        let explicit = Path::new(program);
        if explicit.file_stem().is_some_and(|name| name.eq_ignore_ascii_case("opencode")) {
            if let Some(parent) = explicit.parent().filter(|p| !p.as_os_str().is_empty()) {
                for native in [parent.join("opencode.exe"), parent.join("node_modules/opencode-ai/bin/opencode.exe")] {
                    if native.is_file() { return Command::new(native); }
                }
            }
        }
    }
    #[cfg(windows)]
    if let Some(command) = npm_script_command(program, std::env::var_os("PATH").as_deref()) {
        return command;
    }
    Command::new(program)
}

/// The command an npm shim stands for, or `None` when the name resolves to a
/// real executable or to nothing.
///
/// A `.cmd` shim runs through `cmd.exe`, and Rust refuses to pass a multi-line
/// argument to it (`batch file arguments are invalid`), which every commit
/// prompt is. The shims npm writes are a fixed template around
/// `"%dp0%\node_modules\...\*.js"`, so the script can be started directly:
/// the prompt then travels as an ordinary argument on the full Windows budget.
#[cfg(windows)]
fn npm_script_command(program: &str, path: Option<&std::ffi::OsStr>) -> Option<Command> {
    let explicit = Path::new(program);
    let shim = if explicit.parent().is_some_and(|parent| !parent.as_os_str().is_empty()) {
        if explicit.extension().is_some_and(|e| e.eq_ignore_ascii_case("ps1")) {
            explicit.with_extension("cmd")
        } else {
            explicit.to_path_buf()
        }
    } else {
        find_shim(program, path?)?
    };
    let (node, script) = npm_shim_targets(&shim)?;
    let mut command = Command::new(node);
    command.arg(script);
    Some(command)
}

/// `<dir>\<name>.cmd` (or `.bat`) for the first PATH entry that has one, unless
/// that entry already holds a real `<name>.exe`, which Windows would prefer.
#[cfg(windows)]
fn find_shim(program: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    if Path::new(program).extension().is_some() {
        return None;
    }
    for dir in std::env::split_paths(path).filter(|dir| dir.is_absolute()) {
        if dir.join(format!("{program}.exe")).is_file() {
            return None;
        }
        for extension in ["cmd", "bat"] {
            let candidate = dir.join(format!("{program}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Parses the npm shim template: `"%_prog%" "%dp0%\node_modules\...\cli.js"`.
#[cfg(windows)]
fn npm_shim_targets(shim: &Path) -> Option<(PathBuf, PathBuf)> {
    const SHIM_MAX_BYTES: u64 = 64 * 1024;
    if std::fs::metadata(shim).ok()?.len() > SHIM_MAX_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(shim).ok()?;
    let directory = shim.parent()?;
    let script = text
        .match_indices("%dp0%")
        .filter_map(|(index, marker)| {
            let rest = &text[index + marker.len()..];
            let rest = rest.strip_prefix('\\').unwrap_or(rest);
            let end = rest.find('"')?;
            let relative = &rest[..end];
            let extension = Path::new(relative).extension()?.to_str()?.to_ascii_lowercase();
            matches!(extension.as_str(), "js" | "mjs" | "cjs").then_some(relative)
        })
        .map(|relative| directory.join(relative))
        .find(|script| script.is_file())?;
    let node = directory.join("node.exe");
    let node = if node.is_file() { node } else { find_on_path("node.exe")? };
    Some((node, script))
}

#[cfg(windows)]
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
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
    let ai_commit::Invocation { program, directory: _directory, .. } = ai_commit::prepare_models()?;
    let Some(command) = program else { return Err("OpenCode: нет команды".to_owned()); };
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

/// Close the no-tools agent and remove every executable surface a caller config
/// could add (MCP servers, plugins, commands, instruction files), while keeping
/// inert provider settings so authentication and model choice still work.
fn opencode_commit_config(existing: Option<&str>) -> Result<String, String> {
    let mut config: serde_json::Value = match existing {
        Some(text) => serde_json::from_str(text).map_err(|error| format!("OPENCODE_CONFIG_CONTENT: {error}"))?,
        None => serde_json::json!({}),
    };
    let object = config.as_object_mut().ok_or_else(|| "OPENCODE_CONFIG_CONTENT: нужен JSON-объект".to_owned())?;
    object.insert("mcp".to_owned(), serde_json::json!({}));
    object.insert("plugin".to_owned(), serde_json::json!([]));
    object.insert("command".to_owned(), serde_json::json!({}));
    object.insert("instructions".to_owned(), serde_json::json!([]));
    object.insert("share".to_owned(), serde_json::json!("disabled"));
    object.insert("autoupdate".to_owned(), serde_json::json!(false));
    object.insert("tools".to_owned(), serde_json::json!({ "*": false }));
    object.insert("permission".to_owned(), serde_json::json!({ "*": "deny" }));
    object.insert("agent".to_owned(), serde_json::json!({}));
    let agents = object.get_mut("agent").unwrap().as_object_mut().unwrap();
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

/// Credential copies left behind by a crashed or killed generation. The AI
/// worker sweeps before every run; startup sweeps too, because otherwise the
/// files would sit in `%TEMP%` until the next generation happens to run.
pub fn sweep_stale_ai_state() {
    ai_commit::sweep_stale_ai_state();
}

/// Generates a full commit message with an inference-only built-in profile.
/// Every invocation owns a fresh private directory, removed on success/error.
/// Explicit unknown custom executables remain user code, not safe profiles.
pub fn ai_commit_message(
    command: Option<&str>,
    prompt: &str,
    timeout: std::time::Duration,
) -> Result<String, String> {
    use ai_commit::Backend;
    let ai_commit::Invocation { program, directory: _directory, backend, model } = ai_commit::prepare(command.unwrap_or(""))?;
    let text = if let Some(mut process) = program {
        if backend == Backend::Gemini { process.arg("--prompt"); }
        if backend == Backend::Aider { process.arg("--message"); }
        let prompt = fit_ai_prompt(&process, prompt, &[])?;
        process.arg(prompt.as_ref());
        let label = format!("{backend:?}");
        let (success, stdout, stderr) = run_bounded(process, &label, timeout, 64 * 1024, None)?;
        let output = String::from_utf8_lossy(&stdout);
        if !success {
            let stderr = String::from_utf8_lossy(&stderr);
            return Err(if !stderr.trim().is_empty() {
                stderr.trim().to_owned()
            } else {
                ai_commit::response(backend, &output).err().unwrap_or_else(|| format!("{label}: {}", output.trim()))
            });
        }
        ai_commit::response(backend, &output)?
    } else {
        ai_commit::codex_generate(model.as_deref(), prompt, timeout)?
    };
    let message = clean_ai_message(&text);
    if message.is_empty() {
        return Err(crate::strings::WORKSPACE_AI_EMPTY.to_owned());
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
    let mut command = git_command(root)?;
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
    let remote = branch_remote(root, &current_branch(root)?, false)?;
    run_git_timeout(root, &["fetch", "--no-tags", "--quiet", "--prune", "--", &remote], GIT_TIMEOUT_NETWORK)
        .map(|_| strings_fetch_done(&remote))
}

fn branch_remote(root: &Path, branch: &str, pushing: bool) -> Result<String, String> {
    let mut keys = Vec::new();
    if pushing {
        keys.push(format!("branch.{branch}.pushRemote"));
        keys.push("remote.pushDefault".to_owned());
    }
    keys.push(format!("branch.{branch}.remote"));
    let remote = keys
        .iter()
        .find_map(|key| {
            run_git(root, &["config", "--get", key]).ok().map(|text| text.trim().to_owned()).filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "origin".to_owned());
    if remote.starts_with('-') || remote.chars().any(char::is_control) {
        return Err(crate::strings::WORKSPACE_PATH_INSIDE_REPO.to_owned());
    }
    Ok(remote)
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
    if run_git(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]).is_err() {
        let remote = branch_remote(root, &branch, true)?;
        run_git_timeout(
            root,
            &[
                "push",
                "--porcelain",
                "--set-upstream",
                "--",
                &remote,
                &format!("refs/heads/{branch}:refs/heads/{branch}"),
            ],
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

    /// Values pinned by probing git 2.54: every accepted string kept
    /// `git rev-parse --show-toplevel` inside the directory that held this
    /// `HEAD`, every rejected one made git walk up to the parent.
    #[test]
    fn head_reference_matches_git_probes() {
        let hex = "a".repeat(40);
        for accepted in [
            "ref: refs/heads/main\n",
            "ref:refs/heads/main\n",
            "ref:\nrefs/heads/main\n",
            "ref:\r\n\t refs/heads/main\n",
            "ref: refs/\n",
            &hex,
            &format!("{hex}ZZ"),
            &format!("{}\n", "a".repeat(64)),
        ] {
            assert!(head_reference(accepted), "{accepted:?} is a repository marker");
        }
        for rejected in [
            "",
            "\n",
            "garbage\n",
            "HEAD\n",
            "ref: HEAD\n",
            "ref:\nHEAD\n",
            "ref:\n",
            "ref: refsX/heads/main\n",
            "ref:\u{b}refs/heads/main\n",
            "ref:\u{c}refs/heads/main\n",
            " ref: refs/heads/main\n",
            "refs/heads/main\n",
            "REF: refs/heads/main\n",
            &"a".repeat(39),
            &format!("{}\n", "a".repeat(20)),
        ] {
            assert!(!head_reference(rejected), "{rejected:?} is not a repository marker");
        }
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
        let commits = parse_log(&text, &std::collections::HashSet::new(), &std::collections::HashSet::new());
        assert_eq!(commits.iter().map(|commit| commit.hash.as_str()).collect::<Vec<_>>(), vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]);
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
    fn hazard_keys_cover_program_runners_only() {
        for key in [
            "filter.lfs.clean",
            "filter.hostile.process",
            "core.fsmonitor",
            "core.hooksPath",
            "core.sshCommand",
            "core.attributesFile",
            "core.gitProxy",
            "diff.external",
            "diff.mydriver.command",
            "diff.mydriver.textconv",
            "difftool.meld.cmd",
            "credential.helper",
            "credential.https://host.helper",
            "gpg.program",
            "commit.gpgSign",
            "tag.gpgSign",
            "log.showSignature",
            "include.path",
            "includeIf.gitdir:C:/repo/.path",
            "submodule.lib.update",
            "remote.origin.uploadpack",
        ] {
            assert!(is_hazardous_key(key, "!run"), "{key} can start a program or pull in settings");
        }
        for key in [
            "branch.main.remote",
            "branch.main.merge",
            "remote.origin.url",
            "remote.origin.fetch",
            "submodule.lib.url",
            "submodule.lib.active",
            "user.name",
            "user.email",
            "core.editor",
            "core.pager",
            "core.autocrlf",
            "diff.algorithm",
            "diff.context",
            "log.date",
            "pull.rebase",
            "init.defaultBranch",
            "status.showUntrackedFiles",
            "push.default",
            "merge.conflictStyle",
        ] {
            assert!(!is_hazardous_key(key, "ordinary"), "{key} is routine and must not ask for trust");
        }
    }

    #[test]
    fn transport_hazards_depend_on_the_configured_value() {
        for (key, value) in [
            ("protocol.ext.allow", "always"),
            ("protocol.origin.allow", "always"),
            ("protocol.ext.allow", ""),
            ("protocol.ext.allow", "user"),
            ("url.ext::command.insteadOf", "https://example.com/"),
            ("url.fd::0.pushInsteadOf", "https://example.com/"),
            ("remote.origin.vcs", "helper"),
            ("remote.origin.url", "ext::command"),
            ("remote.origin.pushurl", "fd::0"),
            ("submodule.lib.update", "!command"),
        ] {
            assert!(is_hazardous_key(key, value), "{key} = {value} can leave the machine");
        }
        for (key, value) in [
            ("protocol.ext.allow", "never"),
            ("protocol.allow", "NEVER"),
            ("url.ext::command.insteadOf", ""),
            ("url.fd::0.pushInsteadOf", ""),
            ("remote.origin.url", "https://example.com/repo"),
            ("remote.origin.pushurl", "git@example.com:repo"),
            ("remote.origin.vcs", ""),
            ("submodule.lib.update", "checkout"),
            ("submodule.lib.update", " merge"),
            ("submodule.lib.update", "none"),
        ] {
            assert!(!is_hazardous_key(key, value), "{key} = {value} is routine metadata");
        }
    }

    #[test]
    fn config_parser_matches_git_values_and_fails_closed() {
        let parsed = config_paths("[include]\n path = \"sec\"\\\r\nond.cfg # comment \"\n[includeIf \"gitdir:C:/repo\"]\n path = ~/x.cfg\n").unwrap();
        assert_eq!(parsed, [
            ("include.path".to_owned(), "second.cfg".to_owned()),
            ("includeif.path".to_owned(), "~/x.cfg".to_owned()),
        ]);
        // Unquoted Windows paths keep backslashes only when Git escapes them;
        // an unknown escape is a configuration Git itself rejects.
        assert!(config_paths("[include]\n path = C:\\x\n").is_err());
        assert_eq!(config_paths("[include]\n path = C:\\\\x\n").unwrap()[0].1, "C:\\x");
        // An unterminated value quote is malformed; a nested include cycle is
        // not (Git reads the file once).
        assert!(config_paths("[user]\n name = \"unterminated\n").is_err());
        assert!(config_paths("[include]\n path = a.cfg\n").is_ok());
    }

    #[test]
    fn git_path_consumers_decode_octal_bytes_and_fail_closed() {
        assert_eq!(alternates_path(r#""..\\store\040space""#).unwrap(), std::ffi::OsString::from(r"..\store space"));
        assert_eq!(alternates_path(r"..\store literal ").unwrap(), std::ffi::OsString::from(r"..\store literal "));
        assert_eq!(origin_path(br#"file:"store\040space""#).unwrap(), Some(PathBuf::from("store space")));
        assert_eq!(git_unquote(br#""\303\251""#).unwrap(), "é".as_bytes());
        assert_eq!(git_unquote(br#""\377""#).unwrap(), [255]);
        for malformed in [r#""unterminated"#, r#""\4""#, r#""\400""#, r#""\12""#, r#""\q""#, r#""ok"suffix"#, r#""\000""#] {
            assert!(alternates_path(malformed).is_err(), "{malformed}");
            assert!(origin_path(format!("file:{malformed}").as_bytes()).is_err(), "{malformed}");
        }
        for remote in [r#""\134\134127.0.0.1\134ANVIL-denied""#, r#""\057\057127.0.0.1/ANVIL-denied""#] {
            assert!(alternates_path(remote).unwrap_err().contains("сетевые"));
            assert!(origin_path(format!("file:{remote}").as_bytes()).unwrap_err().contains("сетевые"));
        }
    }

    #[test]
    fn missing_alternate_fanout_stops_at_the_store_budget() {
        let dir = tempfile::tempdir().unwrap();
        let objects = dir.path().join("objects");
        std::fs::create_dir_all(objects.join("info")).unwrap();
        let list = (0..MAX_ALTERNATE_STORES).map(|index| format!("../missing-{index}\n")).collect::<String>();
        std::fs::write(objects.join("info/alternates"), list).unwrap();
        let mut scan = ConfigScan::new(dir.path());
        assert!(inspect_objects(&objects, &mut scan, 0).is_err());
        assert_eq!(scan.alternate_stores.len(), MAX_ALTERNATE_STORES);
        assert!(scan.sources.len() <= 2 * MAX_ALTERNATE_STORES + 2);
        // Exactly root +127 missing stores is permitted, including a repeat.
        let list = (0..MAX_ALTERNATE_STORES - 1).map(|index| format!("../missing-{index}\n")).collect::<String>();
        std::fs::write(objects.join("info/alternates"), format!("{list}../missing-0\n")).unwrap();
        let mut scan = ConfigScan::new(dir.path());
        inspect_objects(&objects, &mut scan, 0).unwrap();
        assert_eq!(scan.alternate_stores.len(), MAX_ALTERNATE_STORES);
    }

    #[test]
    fn source_and_alias_budgets_cover_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let mut scan = ConfigScan::new(dir.path());
        for index in 0..MAX_WATCHED_SOURCES {
            scan.source(&dir.path().join(format!("missing-{index}"))).unwrap();
        }
        assert!(scan.source(&dir.path().join("one-too-many")).is_err());
        assert_eq!(scan.source_index.len(), MAX_WATCHED_SOURCES);
        let mut aliases = ConfigScan::new(dir.path());
        for index in 0..MAX_SOURCE_ALIASES {
            aliases.source(&dir.path().join(format!("alias-{index}/../config"))).unwrap();
        }
        assert_eq!(aliases.sources.len(), 1);
        assert_eq!(aliases.sources[0].aliases.len(), MAX_SOURCE_ALIASES);
        assert!(aliases.source(&dir.path().join("alias-extra/../config")).is_err());
    }

    #[test]
    fn ordinary_loose_objects_do_not_consume_source_slots() {
        let dir = tempfile::tempdir().unwrap();
        let objects = dir.path().join("objects");
        let fanout = objects.join("ab");
        std::fs::create_dir_all(&fanout).unwrap();
        for index in 0..MAX_WATCHED_SOURCES + 1 {
            std::fs::write(fanout.join(format!("{index:038x}")), b"local object").unwrap();
        }
        let mut scan = ConfigScan::new(dir.path());
        inspect_objects(&objects, &mut scan, 0).unwrap();
        assert!(scan.sources.len() < 10, "only directories and path-bearing metadata are watched");
        assert_eq!(scan.metadata_entries, MAX_WATCHED_SOURCES + 2);
        let mut bounded = ConfigScan::new(dir.path());
        bounded.metadata_entries = MAX_METADATA_ENTRIES - 1;
        assert!(inspect_objects(&objects, &mut bounded, 0).is_err());
        assert_eq!(bounded.metadata_entries, MAX_METADATA_ENTRIES);
    }

    #[cfg(any(unix, windows))]
    fn metadata_link(target: &Path, path: &Path, directory: bool) -> bool {
        #[cfg(unix)]
        let result = {
            let _ = directory;
            std::os::unix::fs::symlink(target, path)
        };
        #[cfg(windows)]
        let result = if directory {
            std::os::windows::fs::symlink_dir(target, path)
        } else {
            std::os::windows::fs::symlink_file(target, path)
        };
        match result {
            Ok(()) => true,
            #[cfg(windows)]
            Err(error) if error.raw_os_error() == Some(1314) => {
                eprintln!("symlink privilege is unavailable; skipping reparse fixture");
                false
            }
            Err(error) => panic!("create metadata link: {error}"),
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn nested_metadata_reparse_targets_are_guarded_before_following() {
        for (entry, directory) in [
            ("refs/heads", true), ("objects/pack", true),
            ("objects/ab", true), ("reftable/tables.list", false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(entry);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            if !metadata_link(Path::new("//127.0.0.1/ANVIL-denied"), &path, directory) { return; }
            let mut scan = ConfigScan::new(dir.path());
            assert!(inspect_metadata_tree(path.parent().unwrap(), &mut scan, 0).unwrap_err().contains("сетевые"), "{entry}");
        }
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir_all(git_dir.join("refs")).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        if !metadata_link(Path::new("//127.0.0.1/ANVIL-denied"), &git_dir.join("refs/heads"), true) { return; }
        let repository = RepositoryPaths { root: dir.path().to_path_buf(), git_dir: git_dir.clone(), common_dir: git_dir };
        assert!(inspect_symbolic_head(&repository, &mut ConfigScan::new(dir.path())).unwrap_err().contains("сетевые"));
    }

    #[test]
    fn symbolic_head_chains_watch_targets_and_reject_escaping_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("refs/heads")).unwrap();
        std::fs::write(dir.path().join("HEAD"), "ref:\trefs/heads/main\n").unwrap();
        std::fs::write(dir.path().join("refs/heads/main"), "ref: refs/heads/next\n").unwrap();
        let repository = RepositoryPaths { root: dir.path().to_path_buf(), git_dir: dir.path().to_path_buf(), common_dir: dir.path().to_path_buf() };
        let mut scan = ConfigScan::new(dir.path());
        inspect_symbolic_head(&repository, &mut scan).unwrap();
        assert!(scan.source_index.contains_key(&dir.path().join("refs/heads/next")));
        std::fs::write(dir.path().join("refs/heads/next"), "ref: refs/../outside\n").unwrap();
        assert!(!scan.sources.iter().all(ConfigSource::is_current));
        assert!(inspect_symbolic_head(&repository, &mut ConfigScan::new(dir.path())).is_err());
    }

    #[test]
    fn symbolic_head_resolves_per_worktree_namespaces_in_the_gitdir() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join("linked");
        let common_dir = dir.path().join("common");
        std::fs::create_dir_all(&common_dir).unwrap();
        for namespace in ["bisect", "worktree", "rewritten"] {
            std::fs::create_dir_all(git_dir.join(format!("refs/{namespace}"))).unwrap();
            std::fs::write(git_dir.join("HEAD"), format!("ref: refs/{namespace}/main\n")).unwrap();
            std::fs::write(git_dir.join(format!("refs/{namespace}/main")), "ref: refs/heads/next\n").unwrap();
            let repository = RepositoryPaths { root: dir.path().to_path_buf(), git_dir: git_dir.clone(), common_dir: common_dir.clone() };
            let mut scan = ConfigScan::new(dir.path());
            inspect_symbolic_head(&repository, &mut scan).unwrap();
            assert!(scan.source_index.contains_key(&git_dir.join(format!("refs/{namespace}/main"))));
            assert!(scan.source_index.contains_key(&common_dir.join("refs/heads/next")));
        }
    }

    fn config_records(entries: &[(&str, &str, &str, &str)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (scope, origin, key, value) in entries {
            bytes.extend_from_slice(scope.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(origin.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(key.as_bytes());
            bytes.push(b'\n');
            bytes.extend_from_slice(value.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn digest_covers_repository_hazards_and_ignores_the_user_scope() {
        let root = Path::new("C:/repo");
        // The user's own global settings are not the repository's business.
        let global = config_records(&[("global", "file:C:/Users/u/.gitconfig", "filter.evil.clean", "rm -rf")]);
        assert!(digest_of(&global, root, Default::default()).is_hazard_free());
        // Routine repository keys never ask for trust either.
        let routine = config_records(&[
            ("local", "file:.git/config", "branch.main.remote", "origin"),
            ("local", "file:.git/config", "remote.origin.url", "https://example.com"),
            ("worktree", "file:.git/config", "core.editor", "notepad"),
        ]);
        assert!(digest_of(&routine, root, Default::default()).is_hazard_free());
        // A global includeIf that points inside the repository is repository-controlled.
        let injected = config_records(&[("global", "file:C:/repo/injected.cfg", "filter.evil.clean", "run")]);
        assert!(!digest_of(&injected, root, Default::default()).is_hazard_free());
        let hazard = config_records(&[("local", "file:.git/config", "filter.hostile.clean", "cat")]);
        let stamp = digest_of(&hazard, root, Default::default());
        assert!(!stamp.is_hazard_free());
        assert_eq!(stamp.hazards(), ["filter.hostile.clean"]);
        let other = digest_of(&config_records(&[("local", "file:.git/config", "filter.hostile.clean", "cat -A")]), root, Default::default());
        assert_ne!(stamp, other, "a changed command is a changed digest");
        // The cached digest is dropped when the file behind it moves on.
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(&config, b"[branch]\n").unwrap();
        let before = digest_of(&config_records(&[("local", "file:config", "filter.x.clean", "one")]), root, std::sync::Arc::new(vec![ConfigSource::read(&config)]));
        assert!(before.is_current());
        std::fs::write(&config, b"[branch]\nextra = 1\n").unwrap();
        assert!(!before.is_current(), "a written config file invalidates the digest");
    }

    #[test]
    fn approvals_are_shared_in_memory_and_never_cover_the_stripe_of_a_digest() {
        let root = PathBuf::from("C:/repo");
        let stamp = |value: &str| digest_of(&config_records(&[("local", "file:.git/config", "filter.x.clean", value)]), &root, Default::default());
        let first = stamp("one");
        let second = stamp("two");
        assert!(!trust_approved(&root, &first));
        remember_trust(&root, first.clone());
        assert!(trust_approved(&root, &first), "every pane of the process sees the approval");
        assert!(!trust_approved(&root, &second), "a different digest is not covered");
        let clean = digest_of(&[], &root, Default::default());
        assert!(trust_approved(&root, &clean), "nothing to run, nothing to approve");
    }

    #[cfg(windows)]
    #[test]
    fn npm_shims_resolve_to_the_script_they_wrap() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("node_modules").join("faketool").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("faketool.js");
        std::fs::write(&script, "console.log(JSON.stringify(process.argv.slice(2)))\n").unwrap();
        let shim = dir.path().join("faketool.cmd");
        std::fs::write(
            &shim,
            "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\nendLocal & \"node\" \"%dp0%\\node_modules\\faketool\\bin\\faketool.js\" %*\r\n",
        )
        .unwrap();
        let Some(node) = find_on_path("node.exe") else { return };
        let (resolved_node, resolved_script) = npm_shim_targets(&shim).expect("npm template");
        assert_eq!(resolved_node, node);
        assert_eq!(resolved_script, script);
        // A shim without a real script, and any other file, stay untouched.
        assert!(npm_shim_targets(&dir.path().join("missing.cmd")).is_none());
        std::fs::write(dir.path().join("empty.cmd"), "@echo off\r\n").unwrap();
        assert!(npm_shim_targets(&dir.path().join("empty.cmd")).is_none());
    }

    #[test]
    fn commit_agent_overlay_preserves_existing_provider_configuration() {
        let source = r#"{"provider":{"custom":{"options":{"baseURL":"https://example.com"}}},"agent":{"review":{"mode":"subagent"}}}"#;
        let config: serde_json::Value = serde_json::from_str(&opencode_commit_config(Some(source)).unwrap()).unwrap();
        assert_eq!(config.pointer("/provider/custom/options/baseURL").unwrap(), "https://example.com");
        assert!(config.pointer("/agent/review").is_none(), "a caller agent with tools must not replace anvil-commit");
        assert_eq!(config.pointer("/tools/*").unwrap(), false);
        assert_eq!(config.pointer("/permission/*").unwrap(), "deny");
        assert_eq!(config.pointer("/mcp").unwrap(), &serde_json::json!({}));
        assert_eq!(config.pointer("/plugin").unwrap(), &serde_json::json!([]));
        assert_eq!(config.pointer("/command").unwrap(), &serde_json::json!({}));
    }

    #[test]
    fn built_in_specs_reject_unsafe_options() {
        for spec in ["gemini --yolo", "claude --dangerously-skip-permissions", "aider --yes-always",
            "opencode run --auto", "codex exec --dangerously-bypass-approvals-and-sandbox"] {
            let error = ai_commit::prepare(spec).err().unwrap_or_else(|| panic!("{spec} must be refused"));
            assert!(error.contains("AI:"), "{spec}: {error}");
        }
        assert!(ai_commit::prepare("").is_err());
    }

    #[test]
    fn wrapper_commands_cannot_downgrade_a_built_in_to_custom() {
        for spec in [
            "node C:\\npm\\node_modules\\@openai\\codex\\bin\\codex.js",
            "cmd /c \"claude -p hi\"",
        ] {
            let error = ai_commit::prepare(spec).err().unwrap_or_else(|| panic!("{spec} must be refused"));
            assert!(error.contains("AI:"), "{spec}: {error}");
        }
    }

    #[test]
    fn quoted_command_paths_are_not_split() {
        let words = ai_commit::words("\"C:\\Program Files\\Claude\\claude.exe\" --model opus").unwrap();
        assert_eq!(words, ["C:\\Program Files\\Claude\\claude.exe", "--model", "opus"]);
        assert!(ai_commit::words("\"unclosed").is_err());
    }

    #[test]
    fn codex_requests_never_advertise_or_accept_tools() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.json"), r#"{"auth_mode":"chatgpt","tokens":{"access_token":"token","account_id":"account"}}"#).unwrap();
        std::fs::write(dir.path().join("config.toml"), "model = \"gpt-6-astra\"\n").unwrap();
        let (url, headers, body, token) = ai_commit::codex_request_for_tests(dir.path(), None, "diff").unwrap();
        assert_eq!(url, "https://chatgpt.com/backend-api/codex/responses");
        assert_eq!(token, "token", "the stored access token is used as-is");
        assert!(headers.iter().any(|(name, value)| name.eq_ignore_ascii_case("ChatGPT-Account-ID") && value == "account"));
        assert!(body.get("tools").is_none(), "no tool is advertised, so none can be called");
        assert_eq!(body.get("tool_choice").unwrap(), "none");
        assert_eq!(body.get("model").unwrap(), "gpt-6-astra");

        // An executable auth helper is not part of the request path: only the stored token is used.
        std::fs::write(dir.path().join("auth.json"), r#"{"auth_mode":"apikey","OPENAI_API_KEY":"key"}"#).unwrap();
        let (url, headers, _, token) = ai_commit::codex_request_for_tests(dir.path(), Some("gpt-5.2-codex"), "diff").unwrap();
        assert_eq!(url, "https://api.openai.com/v1/responses");
        assert_eq!(token, "key");
        assert!(!headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("ChatGPT-Account-ID")));

        // A non-openai provider would need user-defined executable options: refuse.
        std::fs::write(dir.path().join("config.toml"), "model_provider = \"custom\"\n").unwrap();
        assert!(ai_commit::codex_request_for_tests(dir.path(), None, "diff").is_err());
    }

    #[test]
    fn claude_managed_policy_gate_flags_executable_settings() {
        let hooks = serde_json::json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "evil.exe"}]}]}});
        assert!(ai_commit::policy_is_executable(&hooks, true));
        assert!(!ai_commit::policy_is_executable(&hooks, false), "bare mode runs no hooks module");
        assert!(ai_commit::policy_is_executable(&serde_json::json!({"apiKeyHelper": "evil.exe"}), false));
        assert!(ai_commit::policy_is_executable(&serde_json::json!({"managedMcpServers": {"x": {}}}), false));
        assert!(ai_commit::policy_is_executable(&serde_json::json!({"env": {"NODE_OPTIONS": "--require evil"}}), false));
        assert!(!ai_commit::policy_is_executable(&serde_json::json!({"permissions": {"deny": ["Read(./x)"]}}), true));
    }
}
