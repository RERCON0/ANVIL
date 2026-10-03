//! Real `git` integration for the workspace panel: detect the repository,
//! read the status, stage, diff and commit.

use std::path::Path;
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
fn non_repository_is_reported_as_such() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("no-repo");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(git::find_root(&nested).is_none());
    assert!(git::status(&nested).is_err() || git::status(&nested).unwrap().changes.is_empty());
}
