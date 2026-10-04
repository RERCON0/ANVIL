//! Real `git` integration for the workspace panel: detect the repository,
//! read the status, stage, diff and commit.

use std::path::Path;
use std::time::{Duration, Instant};
use std::process::Command;

use anvil::git;

fn git_available() -> bool {
    let mut command = Command::new("git");
    command.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.output().map(|out| out.status.success()).unwrap_or(false)
}

fn run(dir: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let status = command.status().expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn repo() -> Option<tempfile::TempDir> {
    if !git_available() {
        eprintln!("git is not installed; skipping");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    run(dir.path(), &["init", "--quiet"]);
    run(dir.path(), &["config", "user.email", "anvil@test"]);
    run(dir.path(), &["config", "user.name", "ANVIL test"]);
    std::fs::write(dir.path().join("tracked.txt"), "one\n").unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src").join("lib.rs"), "pub fn a() {}\n").unwrap();
    run(dir.path(), &["add", "-A"]);
    run(dir.path(), &["commit", "--quiet", "-m", "init: first"]);
    Some(dir)
}

#[test]
fn detects_reads_stages_and_commits() {
    let Some(dir) = repo() else { return };
    let root = git::find_root(dir.path()).expect("repository root");
    assert_eq!(root.canonicalize().unwrap(), dir.path().canonicalize().unwrap());

    std::fs::write(dir.path().join("tracked.txt"), "one\ntwo\n").unwrap();
    std::fs::write(dir.path().join("src").join("lib.rs"), "pub fn a() { /* x */ }\n").unwrap();
    std::fs::write(dir.path().join("fresh.txt"), "hello\n").unwrap();

    let status = git::status(&root).expect("status");
    assert!(!status.branch.is_empty());
    let paths: Vec<&str> = status.changes.iter().map(|c| c.path.as_str()).collect();
    assert!(paths.contains(&"tracked.txt"), "{paths:?}");
    assert!(paths.contains(&"src/lib.rs"), "{paths:?}");
    assert!(paths.contains(&"fresh.txt"), "{paths:?}");
    let fresh = status.changes.iter().find(|c| c.path == "fresh.txt").unwrap();
    assert!(fresh.untracked && fresh.letter() == '?');

    let diff = git::diff(&root, "tracked.txt", false);
    assert!(diff.contains("+two"), "diff: {diff}");

    git::stage(&root, &["tracked.txt".to_owned()], true).expect("stage");
    let staged = git::status(&root).expect("status");
    let tracked = staged.changes.iter().find(|c| c.path == "tracked.txt").unwrap();
    assert!(tracked.staged(), "tracked.txt must be staged");
    assert!(!git::diff(&root, "tracked.txt", true).is_empty(), "staged diff");

    let hash = git::commit(&root, "test: stage tracked").expect("commit");
    assert!(!hash.is_empty());
    let after = git::status(&root).expect("status");
    assert!(!after.changes.iter().any(|c| c.path == "tracked.txt" && c.staged()));
    assert_eq!(git::recent_subjects(&root, 5).first().map(String::as_str), Some("test: stage tracked"));
}

#[test]
fn stages_and_unstages_a_single_hunk() {
    let Some(dir) = repo() else { return };
    let root = git::find_root(dir.path()).expect("root");
    // Two far-apart edits in one file produce two separate hunks.
    let mut lines: Vec<String> = (1..=40).map(|index| format!("line {index}")).collect();
    std::fs::write(dir.path().join("big.txt"), lines.join("\n") + "\n").unwrap();
    run(dir.path(), &["add", "big.txt"]);
    run(dir.path(), &["commit", "--quiet", "-m", "add big.txt"]);
    lines[1] = "FIRST change".into();
    lines[29] = "SECOND change".into();
    std::fs::write(dir.path().join("big.txt"), lines.join("\n") + "\n").unwrap();

    let diff = git::diff(&root, "big.txt", false);
    let files = git::parse_diff(&diff, false);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].hunks.len(), 2, "two hunks expected: {diff}");

    git::apply_hunks(&root, &files[0], &[0], false).expect("stage the first hunk");
    let staged = git::diff(&root, "big.txt", true);
    assert!(staged.contains("+FIRST change"), "{staged}");
    assert!(!staged.contains("SECOND change"), "only one hunk is staged: {staged}");

    let staged_files = git::parse_diff(&staged, true);
    git::apply_hunks(&root, &staged_files[0], &[], true).expect("take the hunk back");
    assert!(git::diff(&root, "big.txt", true).trim().is_empty(), "index is clean again");
    let unstaged = git::diff(&root, "big.txt", false);
    assert!(unstaged.contains("+FIRST change") && unstaged.contains("+SECOND change"));
}

fn output(dir: &Path, args: &[&str]) -> Vec<u8> {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let out = command.output().expect("git");
    assert!(out.status.success(), "git {args:?} failed");
    out.stdout
}

/// Regression: the hunk patch was rebuilt from a lossy UTF-8 string split
/// with `str::lines()`, so a CP1251 line reached the index as U+FFFD and a
/// CRLF line lost its CR. The index must get the worktree bytes exactly.
#[test]
fn staging_a_hunk_keeps_the_exact_bytes() {
    let Some(dir) = repo() else { return };
    let root = git::find_root(dir.path()).expect("root");
    run(dir.path(), &["config", "core.autocrlf", "false"]);
    // "Привет" and "Строка" in CP1251, CRLF line ends.
    let base: &[u8] = b"first\r\n\xcf\xf0\xe8\xe2\xe5\xf2\r\nlast\r\n";
    std::fs::write(dir.path().join("cp1251.txt"), base).unwrap();
    run(dir.path(), &["add", "cp1251.txt"]);
    run(dir.path(), &["commit", "--quiet", "-m", "add cp1251.txt"]);
    let changed: &[u8] = b"first\r\n\xd1\xf2\xf0\xee\xea\xe0\r\n\xcf\xf0\xe8\xe2\xe5\xf2\r\nlast\r\n";
    std::fs::write(dir.path().join("cp1251.txt"), changed).unwrap();

    let files = git::parse_diff(git::diff_bytes(&root, "cp1251.txt", false), false);
    git::apply_hunks(&root, &files[0], &[0], false).expect("stage the hunk");
    assert_eq!(output(dir.path(), &["show", ":cp1251.txt"]), changed, "the index holds the worktree bytes");

    let staged = git::parse_diff(git::diff_bytes(&root, "cp1251.txt", true), true);
    git::apply_hunks(&root, &staged[0], &[0], true).expect("unstage the hunk");
    assert_eq!(output(dir.path(), &["show", ":cp1251.txt"]), base, "unstaging restores the committed bytes");
}

/// A Cyrillic file name: with git's default core.quotePath the diff header
/// was `"a/\320\244..."`, so the parsed path never matched the file.
#[test]
fn hunks_of_a_cyrillic_file_name_can_be_staged() {
    let Some(dir) = repo() else { return };
    let root = git::find_root(dir.path()).expect("root");
    std::fs::write(dir.path().join("файл.txt"), "one\n").unwrap();
    run(dir.path(), &["add", "файл.txt"]);
    run(dir.path(), &["commit", "--quiet", "-m", "add файл.txt"]);
    std::fs::write(dir.path().join("файл.txt"), "one\ntwo\n").unwrap();

    let files = git::parse_diff(git::diff_bytes(&root, "файл.txt", false), false);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "файл.txt");
    git::apply_hunks(&root, &files[0], &[0], false).expect("stage the hunk");
    assert_eq!(output(dir.path(), &["show", ":файл.txt"]), b"one\ntwo\n");
}

/// Regression: the branch's remote was looked up as `config --get branch
/// <b>.remote` (key "branch"), which always failed, so fetch went to origin.
#[test]
fn fetch_uses_the_remote_of_the_current_branch() {
    let Some(dir) = repo() else { return };
    let root = git::find_root(dir.path()).expect("root");
    let remote = tempfile::tempdir().unwrap();
    run(remote.path(), &["init", "--quiet", "--bare"]);
    let remote_path = remote.path().to_string_lossy().into_owned();
    run(dir.path(), &["remote", "add", "upstream", &remote_path]);
    let branch = String::from_utf8(output(dir.path(), &["rev-parse", "--abbrev-ref", "HEAD"])).unwrap();
    run(dir.path(), &["config", &format!("branch.{}.remote", branch.trim()), "upstream"]);
    assert_eq!(git::fetch(&root), Ok("fetch upstream".to_owned()));
}

/// The AI CLI used to run inside the repository, where it loads the
/// repository's own agent config (`.claude/settings.json` hooks, an
/// `opencode.json` MCP server): a cloned repository could run code on a
/// button press. It runs in the given neutral folder, with a deadline.
#[test]
fn ai_message_runs_outside_the_repository_with_a_deadline() {
    let probe = env!("CARGO_BIN_EXE_anvil-probe");
    let work = tempfile::tempdir().unwrap();
    let message = git::ai_commit_message(work.path(), Some(&format!("{probe} pwd")), "prompt", Duration::from_secs(20))
        .expect("the probe answers");
    assert_eq!(Path::new(&message).canonicalize().unwrap(), work.path().canonicalize().unwrap());

    let started = Instant::now();
    let hung = git::ai_commit_message(work.path(), Some(&format!("{probe} sleep")), "prompt", Duration::from_millis(500));
    assert!(hung.is_err(), "a CLI that never answers is an error, got {hung:?}");
    assert!(started.elapsed() < Duration::from_secs(10), "the deadline holds: {:?}", started.elapsed());
}

/// "Stage all" passed every path on the command line (os error 206 past
/// ~32K characters), and unstaging used `restore --staged`, which cannot
/// resolve HEAD in a repository without commits.
#[test]
fn staging_many_paths_and_unstaging_before_the_first_commit() {
    if !git_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    run(dir.path(), &["init", "--quiet"]);
    let root = git::find_root(dir.path()).expect("root");
    let folder = "a-rather-long-folder-name-to-make-the-command-line-long";
    std::fs::create_dir_all(dir.path().join(folder)).unwrap();
    let paths: Vec<String> = (0..600)
        .map(|index| {
            let path = format!("{folder}/file-number-{index:04}-with-a-long-name.txt");
            std::fs::write(dir.path().join(&path), "x\n").unwrap();
            path
        })
        .collect();
    assert!(paths.iter().map(|path| path.len() + 1).sum::<usize>() > 40_000, "longer than a command line");

    git::stage(&root, &paths, true).expect("stage all");
    let staged = git::status(&root).expect("status");
    assert_eq!(staged.changes.iter().filter(|change| change.staged()).count(), paths.len());

    git::stage(&root, &paths[..2], false).expect("unstage without a HEAD");
    let after = git::status(&root).expect("status");
    assert_eq!(after.changes.iter().filter(|change| change.staged()).count(), paths.len() - 2);
}

#[test]
fn non_repository_is_reported_as_such() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("no-repo");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(git::find_root(&nested).is_none());
    assert!(git::status(&nested).is_err() || git::status(&nested).unwrap().changes.is_empty());
}

#[test]
fn commit_detail_lists_files_and_patch() {
    let Some(dir) = repo() else { return };
    let root = git::find_root(dir.path()).expect("root");
    std::fs::write(dir.path().join("tracked.txt"), "one\ntwo\n").unwrap();
    run(dir.path(), &["add", "tracked.txt"]);
    run(dir.path(), &["commit", "--quiet", "-m", "add a line"]);

    let head = git::log(&root).expect("log").commits.first().expect("a commit").hash.clone();
    let detail = git::commit_detail(&root, &head).expect("commit detail");
    assert!(
        detail.files.iter().any(|(status, path, additions, _)| *status == 'M' && path == "tracked.txt" && *additions == 1),
        "files: {:?}",
        detail.files
    );
    assert!(detail.patch.contains("+two"), "patch: {}", detail.patch);
    assert!(detail.header.contains("add a line"), "header: {}", detail.header);

    // A root commit has no parent and must still show its files and patch.
    let first_hash = git::log(&root).expect("log").commits.last().expect("a commit").hash.clone();
    let first = git::commit_detail(&root, &first_hash).expect("root commit detail");
    assert!(first.files.iter().any(|(_, path, _, _)| path == "src/lib.rs"), "files: {:?}", first.files);
    assert!(first.patch.contains("+pub fn a() {}"), "patch: {}", first.patch);
}
