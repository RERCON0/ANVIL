//! Real `git` integration for the workspace panel: detect the repository,
//! read the status, stage, diff and commit.

use std::path::{Path, PathBuf};
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
fn trust_digest_ignores_routine_keys_and_tracks_program_runners() {
    let Some(dir) = repo() else { return };
    let root = dir.path();
    let clean = git::repository_stamp(root).expect("stamp");
    assert!(clean.is_hazard_free(), "a plain repository has nothing to approve");
    assert!(git::trust_approved(root, &clean), "nothing to run, nothing to ask");
    // Keys `push --set-upstream`, `checkout -b`, `remote add` and the editor
    // write are routine: they must not invalidate an approval.
    run(root, &["config", "branch.main.remote", "origin"]);
    run(root, &["config", "branch.main.merge", "refs/heads/main"]);
    run(root, &["config", "remote.origin.url", "."]);
    run(root, &["config", "core.editor", "notepad"]);
    run(root, &["config", "submodule.lib.url", "https://example.com/lib"]);
    run(root, &["config", "submodule.lib.active", "true"]);
    run(root, &["config", "submodule.lib.update", "checkout"]);
    run(root, &["config", "protocol.ext.allow", "never"]);
    let after_routine = git::repository_stamp(root).expect("stamp");
    assert_eq!(clean, after_routine, "tracking keys are not a configuration change");
    // A filter command is not routine: it runs on status, diff and add.
    run(root, &["config", "filter.hostile.clean", "cat"]);
    let hazardous = git::repository_stamp(root).expect("stamp");
    assert_ne!(after_routine, hazardous);
    assert_eq!(hazardous.hazards(), ["filter.hostile.clean"]);
    assert!(!git::trust_approved(root, &hazardous), "a filter needs approval");
    git::remember_trust(root, hazardous.clone());
    assert!(git::trust_approved(root, &hazardous), "the process remembers the approval");
}

#[test]
fn trust_tracks_transport_values_and_custom_submodule_updates() {
    let Some(dir) = repo() else { return };
    for (key, value) in [
        ("protocol.ext.allow", "always"),
        ("url.ext::command.insteadOf", "https://example.com/"),
        ("url.fd::0.pushInsteadOf", "https://example.com/"),
        ("remote.origin.vcs", "custom-helper"),
        ("remote.origin.url", "ext::command"),
        ("remote.origin.pushurl", "fd::0"),
        ("submodule.lib.update", "!command"),
    ] {
        run(dir.path(), &["config", key, value]);
        let stamp = git::repository_stamp(dir.path()).expect("transport stamp");
        assert!(stamp.hazards().iter().any(|hazard| hazard.eq_ignore_ascii_case(key)), "{key}");
        assert!(!git::trust_approved(dir.path(), &stamp), "{key} needs approval");
        run(dir.path(), &["config", "--unset", key]);
    }
    for value in ["checkout", "merge", "rebase", "none"] {
        run(dir.path(), &["config", "submodule.lib.update", value]);
        assert!(git::repository_stamp(dir.path()).unwrap().is_hazard_free(), "{value} is ordinary metadata");
    }
}

fn quoted_config(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[test]
fn nested_local_includes_and_created_include_revoke_old_approval() {
    let Some(dir) = repo() else { return };
    let root = dir.path();
    let includes = root.join("include space");
    std::fs::create_dir(&includes).unwrap();
    let first = includes.join("first # file.cfg");
    std::fs::write(&first, "[include] path = \"sec\"\\\r\nond.cfg # an unmatched comment quote: \"\r\n").unwrap();
    run(root, &["config", "include.path", "../include space/first # file.cfg"]);
    run(root, &["config", "--add", "include.path", "../include space/../include space/first # file.cfg"]);
    let before = git::repository_stamp(root).expect("missing nested include");
    git::remember_trust(root, before.clone());
    assert!(git::trust_approved(root, &before));
    std::fs::write(includes.join("second.cfg"), "[filter \"nested\"]\n clean = changed-command\n").unwrap();
    assert!(!git::trust_approved(root, &before), "a stale stamp must not authorize commands");
    let created = git::repository_stamp(root).expect("created nested include");
    assert!(created.hazards().iter().any(|key| key == "filter.nested.clean"));
    assert_eq!(created.hazards().iter().filter(|key| key.as_str() == "filter.nested.clean").count(), 1);
    assert!(!git::trust_approved(root, &created), "creation must revoke the remembered digest");
    git::remember_trust(root, created.clone());
    std::fs::write(includes.join("second.cfg"), "[filter \"nested\"]\n clean = another-command\n").unwrap();
    let changed = git::repository_stamp(root).unwrap();
    assert_ne!(created, changed);
    assert!(!git::trust_approved(root, &changed));
}

#[test]
fn network_gitfiles_and_commondir_are_rejected_by_all_command_routes() {
    let Some(dir) = repo() else { return };
    let fake = dir.path().join("nested");
    std::fs::create_dir(&fake).unwrap();
    for target in [
        "//127.0.0.1/ANVIL-denied/config",
        r"\\?\UNC\127.0.0.1\ANVIL-denied\config",
        r"\\.\GLOBALROOT\Device\Mup\127.0.0.1\ANVIL-denied",
        "https://example.invalid/git",
    ] {
        std::fs::write(fake.join(".git"), format!("gitdir: {target}\n")).unwrap();
        let error = git::repository_identity(&fake).unwrap_err();
        assert!(error.contains("сетевые"), "{error}");
        assert!(git::find_root(&fake).is_none());
        assert!(git::run_git(&fake, &["config", "--list"]).unwrap_err().contains("сетевые"));
        assert!(git::status(&fake).unwrap_err().contains("сетевые"));
        assert!(git::commit(&fake, "must not run".to_owned()).unwrap_err().contains("сетевые"));
        assert!(git::resolve_path(Path::new(target), "tracked.txt").unwrap_err().contains("сетевые"));
    }
    std::fs::write(dir.path().join(".git/commondir"), "//127.0.0.1/ANVIL-denied/common\n").unwrap();
    assert!(git::repository_stamp(dir.path()).unwrap_err().contains("сетевые"));
}

#[test]
fn network_includes_and_external_attributes_are_rejected_before_git_reads_them() {
    let Some(dir) = repo() else { return };
    let config = dir.path().join(".git/config");
    let original = std::fs::read_to_string(&config).unwrap();
    for (section, key) in [("include", "path"), ("includeIf \"onbranch:never-selected\"", "path"), ("core", "attributesFile")] {
        for target in ["//127.0.0.1/ANVIL-denied/file", r"\\?\UNC\127.0.0.1\ANVIL-denied\file", "file://example.invalid/file"] {
            std::fs::write(&config, format!("{original}\n[{section}]\n {key} = {}\n", quoted_config(target))).unwrap();
            assert!(git::repository_identity(dir.path()).unwrap_err().contains("сетевые"));
            assert!(git::run_git(dir.path(), &["config", "--includes", "--list"]).unwrap_err().contains("сетевые"));
        }
    }
}

#[test]
fn worktree_submodule_and_empty_repositories_have_routine_metadata() {
    let Some(dir) = repo() else { return };
    let worktree = tempfile::tempdir().unwrap();
    let path = worktree.path().join("tree");
    run(dir.path(), &["worktree", "add", "--quiet", "--detach", path.to_str().unwrap()]);
    let identity = git::repository_identity(&path).unwrap().unwrap();
    assert_eq!(identity.root.canonicalize().unwrap(), path.canonicalize().unwrap());
    assert!(identity.stamp.is_hazard_free());
    assert!(!git::status(&path).unwrap().branch.is_empty());
    let Some(module) = repo() else { return };
    run(dir.path(), &["-c", "protocol.file.allow=always", "submodule", "add", "--quiet", module.path().to_str().unwrap(), "module"]);
    assert!(git::repository_identity(dir.path()).unwrap().unwrap().stamp.is_hazard_free());
    assert!(git::repository_identity(&dir.path().join("module")).unwrap().unwrap().stamp.is_hazard_free());
    let empty = tempfile::tempdir().unwrap();
    run(empty.path(), &["init", "--quiet"]);
    assert!(git::repository_identity(empty.path()).unwrap().unwrap().stamp.is_hazard_free());
    assert!(git::status(empty.path()).unwrap().head_oid.is_none());
    assert!(git::log(empty.path()).unwrap().commits.is_empty());
}

#[test]
fn global_conditional_missing_sources_and_config_read_races_revoke_approval() {
    const CHILD: &str = "ANVIL_TRUST_GUARD_TEST_CHILD";
    // Never touch the real profile: the body runs only in the child process
    // that was given an isolated HOME and its own global/system config files.
    let isolated_child = std::env::var_os(CHILD).is_some()
        && std::env::var_os("GIT_CONFIG_GLOBAL").is_some()
        && std::env::var_os("HOME").is_some();
    if !isolated_child {
        let isolated = tempfile::tempdir().unwrap();
        let global = isolated.path().join("global.cfg");
        let system = isolated.path().join("system.cfg");
        std::fs::write(&global, "").unwrap();
        std::fs::write(&system, "").unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", "global_conditional_missing_sources_and_config_read_races_revoke_approval", "--nocapture"])
            .env(CHILD, "1").env("GIT_CONFIG_GLOBAL", &global).env("GIT_CONFIG_SYSTEM", &system)
            .env("HOME", isolated.path()).env("XDG_CONFIG_HOME", isolated.path().join(".config"))
            .env("GIT_TRACE2_EVENT", isolated.path().join("trace.json"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            child.creation_flags(0x0800_0000);
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "isolated child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        return;
    }
    let Some(dir) = repo() else { return };
    let root = dir.path();
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    std::fs::write(home.join("tilde.cfg"), "[filter \"tilde\"]\n clean = tilde-command\n").unwrap();
    run(root, &["config", "include.path", "~/tilde.cfg"]);
    assert!(git::repository_stamp(root).unwrap().hazards().iter().any(|key| key == "filter.tilde.clean"));
    run(root, &["config", "--unset", "include.path"]);
    let global = std::path::PathBuf::from(std::env::var_os("GIT_CONFIG_GLOBAL").unwrap());
    let condition = root.join(".git").to_string_lossy().replace('\\', "/");
    let injected = root.join("injected.cfg");
    std::fs::write(&global, format!("[includeIf {}]\n path = {}\n", quoted_config(&format!("gitdir:{condition}")), quoted_config(injected.to_str().unwrap()))).unwrap();
    run(root, &["config", "filter.approved.clean", "old-command"]);
    let initial = git::repository_stamp(root).unwrap();
    git::remember_trust(root, initial.clone());
    assert!(git::trust_approved(root, &initial));
    std::fs::write(&global, format!("[user]\n name = unrelated global edit\n[includeIf {}]\n path = {}\n", quoted_config(&format!("gitdir:{condition}")), quoted_config(injected.to_str().unwrap()))).unwrap();
    let routine = git::repository_stamp(root).unwrap();
    assert_eq!(initial, routine, "unrelated user configuration does not change hazards");
    assert!(git::trust_approved(root, &routine));
    std::fs::write(&injected, "[filter \"injected\"]\n clean = hostile-command\n").unwrap();
    let changed = git::repository_stamp(root).unwrap();
    assert!(changed.hazards().iter().any(|key| key == "filter.injected.clean"));
    assert!(!git::trust_approved(root, &changed), "a missing global include target must have been watched");
    std::fs::remove_file(&injected).unwrap();
    let trace = std::path::PathBuf::from(std::env::var_os("GIT_TRACE2_EVENT").unwrap());
    let config = root.join(".git/config");
    for iteration in 0..12 {
        run(root, &["config", "filter.approved.clean", &format!("old-{iteration}")]);
        let old = git::repository_stamp(root).unwrap();
        git::remember_trust(root, old);
        run(root, &["config", "user.name", &format!("invalidate-{iteration}")]);
        let mut replacement = std::fs::read_to_string(&config).unwrap();
        replacement = replacement.replace(&format!("old-{iteration}"), &format!("new-command-{iteration}"));
        std::fs::write(&trace, "").unwrap();
        let trace_reader = trace.clone();
        let replacement_path = config.clone();
        let writer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if std::fs::read_to_string(&trace_reader).unwrap_or_default().contains("\"event\":\"exit\"") {
                    break;
                }
                std::thread::yield_now();
            }
            // Best effort: if the real git process never reported, the write
            // still lands and the final assertion holds.
            std::fs::write(&replacement_path, replacement).unwrap();
        });
        let _ = git::repository_stamp(root); // a racing read may conservatively fail
        writer.join().unwrap();
        let current = git::repository_stamp(root).unwrap();
        assert!(!git::trust_approved(root, &current), "a racing config output must not inherit a newer stat");
    }
    // Creating the global config itself must revoke an approval, exactly like
    // creating a file that it includes.
    std::fs::remove_file(&global).unwrap();
    let without_global = git::repository_stamp(root).unwrap();
    git::remember_trust(root, without_global.clone());
    assert!(git::trust_approved(root, &without_global), "removing the include chain leaves the rest valid");
    std::fs::write(&global, format!("[includeIf {}]\n path = {}\n", quoted_config(&format!("gitdir:{condition}")), quoted_config(injected.to_str().unwrap()))).unwrap();
    std::fs::write(&injected, "[filter \"later\"]\n clean = later-command\n").unwrap();
    let recreated = git::repository_stamp(root).unwrap();
    assert!(recreated.hazards().iter().any(|key| key == "filter.later.clean"), "{:?}", recreated.hazards());
    assert!(!git::trust_approved(root, &recreated), "a recreated include chain needs a new approval");
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

    let hash = git::commit(&root, "test: stage tracked".to_owned()).expect("commit");
    assert!(!hash.is_empty());
    let after = git::status(&root).expect("status");
    assert!(!after.changes.iter().any(|c| c.path == "tracked.txt" && c.staged()));
    assert_eq!(git::recent_subjects(&root, 5).first().map(String::as_str), Some("test: stage tracked"));
}

#[test]
fn oversized_multiline_commit_message_is_stored_intact() {
    let Some(dir) = repo() else { return };
    let root = dir.path();
    std::fs::write(root.join("tracked.txt"), "one\nlarge message\n").unwrap();
    git::stage(root, &["tracked.txt".to_owned()], true).expect("stage");
    let message = format!(
        "feat: retain the entire detailed commit message\n\n{}\nCompatibility: preserve every bullet and UTF-8 character.\n",
        "- Explain a meaningful change with quotes \"detail\", backslashes C:\\source\\file, and supplementary characters 😀.\n".repeat(700)
    );
    assert!(message.encode_utf16().count() > 32_767);
    let short = git::commit(root, message.clone()).expect("commit over Windows command-line limit");
    assert_eq!(short, String::from_utf8(output(root, &["rev-parse", "--short", "HEAD"])).unwrap().trim());
    let commit = String::from_utf8(output(root, &["cat-file", "commit", "HEAD"])).unwrap();
    assert_eq!(commit.split_once("\n\n").expect("commit headers").1, message);
}

#[test]
fn status_snapshots_track_head_and_upstream_even_when_counts_match() {
    let Some(dir) = repo() else { return };
    let root = dir.path();
    run(root, &["branch", "-M", "main"]);
    let first = String::from_utf8(output(root, &["rev-parse", "HEAD"])).unwrap().trim().to_owned();
    run(root, &["checkout", "--quiet", "-b", "remote-one"]);
    run(root, &["commit", "--quiet", "--allow-empty", "-m", "remote one"]);
    let remote_one = String::from_utf8(output(root, &["rev-parse", "HEAD"])).unwrap().trim().to_owned();
    run(root, &["checkout", "--quiet", "-b", "remote-two", &first]);
    run(root, &["commit", "--quiet", "--allow-empty", "-m", "remote two"]);
    let remote_two = String::from_utf8(output(root, &["rev-parse", "HEAD"])).unwrap().trim().to_owned();
    run(root, &["checkout", "--quiet", "main"]);
    run(root, &["commit", "--quiet", "--allow-empty", "-m", "local"]);
    run(root, &["remote", "add", "origin", "."]);
    run(root, &["update-ref", "refs/remotes/origin/main", &remote_one]);
    let fallback = git::status(root).expect("status without configured upstream");
    assert_eq!(fallback.upstream, None);
    assert_eq!(fallback.upstream_oid.as_deref(), Some(remote_one.as_str()));
    run(root, &["branch", "--set-upstream-to=origin/main", "main"]);
    let before = git::status(root).expect("status");
    let head = String::from_utf8(output(root, &["rev-parse", "HEAD"])).unwrap().trim().to_owned();
    assert_eq!(before.head_oid.as_deref(), Some(head.as_str()));
    assert_eq!(before.upstream_oid.as_deref(), Some(remote_one.as_str()));
    assert_eq!((before.ahead, before.behind), (1, 1));

    run(root, &["update-ref", "refs/remotes/origin/main", &remote_two]);
    let remote_changed = git::status(root).expect("status after remote ref rewrite");
    assert_eq!((remote_changed.ahead, remote_changed.behind), (before.ahead, before.behind));
    assert_eq!(remote_changed.head_oid, before.head_oid);
    assert_eq!(remote_changed.upstream_oid.as_deref(), Some(remote_two.as_str()));

    run(root, &["commit", "--quiet", "--allow-empty", "--amend", "-m", "local rewritten"]);
    let head_changed = git::status(root).expect("status after external amend");
    let head = String::from_utf8(output(root, &["rev-parse", "HEAD"])).unwrap().trim().to_owned();
    assert_eq!((head_changed.ahead, head_changed.behind), (before.ahead, before.behind));
    assert_eq!(head_changed.head_oid.as_deref(), Some(head.as_str()));
    assert_ne!(head_changed.head_oid, before.head_oid);
    assert_eq!(head_changed.upstream_oid, remote_changed.upstream_oid);
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

/// Runs git without asserting success: `(succeeded, stdout-or-stderr)`.
fn attempt(dir: &Path, args: &[&str]) -> (bool, String) {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let out = command.output().expect("git");
    let text = if out.status.success() {
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    (out.status.success(), text.trim().to_owned())
}

/// The root real git reports for `dir`; panics if git refuses the directory.
fn git_toplevel(dir: &Path) -> String {
    let (ok, text) = attempt(dir, &["rev-parse", "--show-toplevel"]);
    assert!(ok, "git rev-parse --show-toplevel failed in {}: {text}", dir.display());
    text
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

/// Each generation owns a fresh neutral folder and removes it before returning;
/// planted config cannot survive into another request. Custom commands still
/// observe the same isolation and deadline as safe built-in profiles.
#[test]
fn ai_message_runs_outside_the_repository_with_a_deadline() {
    let probe = env!("CARGO_BIN_EXE_anvil-probe");
    let command = format!("\"{probe}\" pwd");
    let first = git::ai_commit_message(Some(&command), "prompt", Duration::from_secs(20)).expect("the probe answers");
    let second = git::ai_commit_message(Some(&command), "prompt", Duration::from_secs(20)).expect("the probe answers again");
    assert_ne!(first, second, "requests never reuse a config directory");
    assert!(!Path::new(&first).exists(), "first generation directory is removed");
    assert!(!Path::new(&second).exists(), "second generation directory is removed");

    let started = Instant::now();
    let hung = git::ai_commit_message(Some(&format!("\"{probe}\" sleep")), "prompt", Duration::from_millis(500));
    assert!(hung.is_err(), "a CLI that never answers is an error, got {hung:?}");
    assert!(started.elapsed() < Duration::from_secs(10), "the deadline holds: {:?}", started.elapsed());
}

#[test]
fn built_in_executable_paths_cannot_bypass_commit_safety() {
    for command in [
        "\"C:\\Program Files\\Claude\\claude.exe\" --dangerously-skip-permissions",
        "\"C:\\npm install\\codex.cmd\" exec --dangerously-bypass-approvals-and-sandbox",
        "gemini --yolo",
        "aider --yes-always",
        "opencode run --auto",
        "node \"C:\\npm install\\node_modules\\@google\\gemini-cli\\bundle\\gemini.js\" --yolo",
    ] {
        let error = git::ai_commit_message(Some(command), "injected diff", Duration::from_secs(1)).unwrap_err();
        assert!(error.contains("AI:"), "unsafe options are refused before starting {command}: {error}");
    }
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
    let unborn = git::status(&root).expect("unborn status");
    assert_eq!(unborn.head_oid, None);
    assert_eq!(unborn.upstream_oid, None);
    assert!(unborn.is_repo());
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

/// Regression: a nested `.git` that real git rejects used to become ANVIL's
/// root. An empty `.git` directory must be skipped exactly as git skips it,
/// and a dangling gitfile must fail closed exactly as git refuses.
#[test]
fn invalid_nested_git_markers_follow_real_git() {
    let Some(dir) = repo() else { return };
    let outer = dir.path();

    // An empty `.git` directory under `sub`: git walks up to the outer
    // repository, and so must ANVIL.
    let sub = outer.join("sub-empty");
    std::fs::create_dir_all(sub.join(".git")).unwrap();
    assert_eq!(
        PathBuf::from(git_toplevel(&sub)).canonicalize().unwrap(),
        outer.canonicalize().unwrap(),
        "git ignores a nested .git directory without HEAD/objects/refs"
    );
    let root = git::find_root(&sub).expect("the outer repository is still found");
    assert_eq!(root.canonicalize().unwrap(), outer.canonicalize().unwrap());
    assert!(!git::status(&root).unwrap().branch.is_empty());

    // A `.git` file whose target is not a git directory: git refuses the
    // repository instead of walking up, so ANVIL must not promote it either.
    let bogus = outer.join("sub-bogus");
    std::fs::create_dir_all(&bogus).unwrap();
    std::fs::write(bogus.join(".git"), format!("gitdir: {}\n", outer.join("not-a-git-dir").display())).unwrap();
    let (ok, error) = attempt(&bogus, &["rev-parse", "--show-toplevel"]);
    assert!(!ok, "git refuses a dangling gitfile: {error}");
    assert!(git::find_root(&bogus).is_none(), "ANVIL must refuse it too");
    assert!(git::repository_identity(&bogus).is_err());
}

/// A real nested repository and a valid gitfile worktree are discovered at
/// their own root, matching `git rev-parse --show-toplevel`.
#[test]
fn valid_nested_repositories_match_git() {
    let Some(dir) = repo() else { return };
    let outer = dir.path();

    // A real nested repository keeps its own root.
    let nested = outer.join("nested-repo");
    std::fs::create_dir_all(&nested).unwrap();
    run(&nested, &["init", "--quiet"]);
    assert_eq!(
        PathBuf::from(git_toplevel(&nested)).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    assert_eq!(git::find_root(&nested).unwrap().canonicalize().unwrap(), nested.canonicalize().unwrap());

    // A valid gitfile points a nested directory at metadata elsewhere; git
    // reports the gitfile's own directory as the work tree.
    let worktree = tempfile::tempdir().unwrap();
    let target = worktree.path().join("tree");
    run(outer, &["worktree", "add", "--quiet", "--detach", target.to_str().unwrap()]);
    let gitfile = std::fs::read_to_string(target.join(".git")).unwrap();
    assert!(gitfile.starts_with("gitdir: "), "{gitfile}");
    let linked = outer.join("linked-worktree");
    std::fs::create_dir_all(&linked).unwrap();
    std::fs::write(linked.join(".git"), gitfile).unwrap();
    assert_eq!(
        PathBuf::from(git_toplevel(&linked)).canonicalize().unwrap(),
        linked.canonicalize().unwrap(),
        "git resolves a valid gitfile to its own directory"
    );
    let root = git::find_root(&linked).expect("gitfile worktree root");
    assert_eq!(root.canonicalize().unwrap(), linked.canonicalize().unwrap());
    assert!(!git::status(&root).unwrap().branch.is_empty());
}

/// Opening the outer repository from a directory whose own `.git` is invalid
/// must address paths relative to git's real root, not the nested directory.
#[test]
fn staging_from_an_outer_repository_uses_gits_own_root() {
    let Some(dir) = repo() else { return };
    let outer = dir.path();
    let sub = outer.join("sub");
    std::fs::create_dir_all(sub.join(".git")).unwrap();
    std::fs::write(sub.join("inner.txt"), "inner\n").unwrap();

    let git_root = PathBuf::from(git_toplevel(&sub)).canonicalize().unwrap();
    let root = git::find_root(&sub).expect("root");
    assert_eq!(root.canonicalize().unwrap(), outer.canonicalize().unwrap());
    assert_eq!(root.canonicalize().unwrap(), git_root);

    git::stage(&root, &["sub/inner.txt".to_owned()], true).expect("stage relative to git's root");
    let (ok, staged) = attempt(outer, &["diff", "--cached", "--name-only"]);
    assert!(ok, "{staged}");
    assert_eq!(staged.lines().collect::<Vec<_>>(), ["sub/inner.txt"]);
    // Real git, run from the nested directory, sees the same staged path.
    let (ok, from_sub) = attempt(&sub, &["diff", "--cached", "--name-only"]);
    assert!(ok, "{from_sub}");
    assert_eq!(from_sub.lines().collect::<Vec<_>>(), ["sub/inner.txt"]);
    assert!(git::diff(&root, "sub/inner.txt", true).contains("+inner"));
}

#[test]
fn commit_detail_lists_files_and_loads_only_the_selected_patch() {
    let Some(dir) = repo() else { return };
    let root = git::find_root(dir.path()).expect("root");
    std::fs::write(dir.path().join("tracked.txt"), "one\ntwo\n").unwrap();
    std::fs::write(dir.path().join("файл[1].txt"), "literal-only\n").unwrap();
    std::fs::write(dir.path().join("файл1.txt"), "other-only\n").unwrap();
    run(dir.path(), &["add", "-A"]);
    run(dir.path(), &["commit", "--quiet", "-m", "add a line"]);

    let head = git::log(&root).expect("log").commits.first().expect("a commit").hash.clone();
    let detail = git::commit_detail(&root, &head).expect("commit detail");
    assert!(
        detail.files.iter().any(|(status, path, additions, _)| *status == 'M' && path == "tracked.txt" && *additions == 1),
        "files: {:?}",
        detail.files
    );
    let patch = git::commit_file_diff(&root, &head, "tracked.txt").expect("selected file diff");
    assert!(patch.contains("+two") && !patch.contains("literal-only") && !patch.contains("other-only"), "patch: {patch}");
    let literal_patch = git::commit_file_diff(&root, &head, "файл[1].txt").expect("literal path diff");
    assert!(literal_patch.contains("+literal-only") && !literal_patch.contains("other-only"), "patch: {literal_patch}");
    assert!(detail.header.contains("add a line"), "header: {}", detail.header);

    // A root commit has no parent and must still show its files and patch.
    let first_hash = git::log(&root).expect("log").commits.last().expect("a commit").hash.clone();
    let first = git::commit_detail(&root, &first_hash).expect("root commit detail");
    assert!(first.files.iter().any(|(_, path, _, _)| path == "src/lib.rs"), "files: {:?}", first.files);
    let first_patch = git::commit_file_diff(&root, &first_hash, "src/lib.rs").expect("root commit file diff");
    assert!(first_patch.contains("+pub fn a() {}") && !first_patch.contains("+one"), "patch: {first_patch}");
}
