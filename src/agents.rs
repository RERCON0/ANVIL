//! Session restart uses fixed agent names and flags, never saved command lines.
use std::path::Path;

use crate::profiles::{Profile, ProfileKind};

pub(crate) fn continue_args(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "codex" => Some(&["resume", "--last"]),
        "claude" | "opencode" | "omp" => Some(&["--continue"]),
        _ => None,
    }
}

pub(crate) fn resumed_profile(profile: &Profile, name: &str, cwd: &Path) -> Result<Option<Profile>, String> {
    let Some(args) = continue_args(name) else { return Ok(None) };
    // Windows process discovery cannot identify agents inside a WSL VM.
    // A hand-edited session must not turn one into a Windows conversation.
    if profile.kind == ProfileKind::Wsl {
        return Ok(None);
    }
    let mut result = profile.clone();
    let bash = profile.kind == ProfileKind::GitBash
        || (profile.kind == ProfileKind::Custom
            && crate::profiles::is_shell(profile)
            && Path::new(&profile.command)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.eq_ignore_ascii_case("bash")));
    if bash {
        // Resolve the canonical CLI *after* the same interactive/login startup
        // as a manual launch. ANVIL's PATH (and its npm-native preference) can
        // select a different OpenCode version; HOME/XDG_* may differ as well.
        // Only allowlisted names/flags enter this script. The folder is data,
        // not shell syntax, and is restored after any startup-file `cd`.
        result.env.insert("ANVIL_RESTORE_CWD".into(), cwd.to_string_lossy().replace('\\', "/"));
        // Without -c a Bash attached to ConPTY is implicitly interactive.
        // Make that explicit so custom profiles also keep their .bashrc.
        if !result.args.iter().any(|arg| matches!(arg.as_str(), "-i" | "-il" | "-li")) {
            result.args.push("-i".into());
        }
        result.args.extend([
            "-c".into(),
            format!("builtin cd -- \"$ANVIL_RESTORE_CWD\" && command {name} {}", args.join(" ")),
        ]);
        return Ok(Some(result));
    }
    let command = crate::git::installed_cli_command(name).ok_or_else(|| crate::tr_format!(
        "Cannot restore {name}: the CLI is not installed on PATH. Install it or disable Restore CLI agents.",
        "Не удалось восстановить {name}: CLI не установлен в PATH. Установите его или выключите восстановление CLI-агентов."
    ))?;
    result.command = command.get_program().to_string_lossy().into_owned();
    result.args = command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
    result.args.extend(args.iter().map(|arg| (*arg).to_owned()));
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_restoration_uses_the_profile_instead_of_anvils_cli_lookup() {
        let profile = Profile {
            id: "git-bash".into(),
            name: "Git Bash".into(),
            command: "fixture-bash.exe".into(),
            args: vec!["--login".into(), "-i".into()],
            cwd: None,
            env: std::collections::BTreeMap::from([("HOME".into(), "fixture-home".into())]),
            kind: ProfileKind::GitBash,
        };
        let resumed = resumed_profile(&profile, "opencode", Path::new("C:\\fixture folder")).unwrap().unwrap();
        assert_eq!(resumed.command, profile.command, "resolve OpenCode only after Bash has loaded its profile");
        assert_eq!(&resumed.args[..2], &profile.args);
        assert_eq!(resumed.env["HOME"], "fixture-home");
        assert!(resumed.args.iter().any(|arg| arg == "-c"));
        assert_eq!(resumed.env["ANVIL_RESTORE_CWD"], "C:/fixture folder");
        assert_eq!(resumed.args.last().unwrap(), "builtin cd -- \"$ANVIL_RESTORE_CWD\" && command opencode --continue");
        assert_eq!(profile.args, ["--login", "-i"], "the fallback profile is not modified");
    }

    #[test]
    fn bash_restore_scripts_have_only_canonical_commands_and_keep_cwd_as_data() {
        let mut profile = Profile {
            id: "custom-bash".into(),
            name: "Bash".into(),
            command: "bash.exe".into(),
            args: vec!["--noprofile".into(), "--norc".into(), "-i".into()],
            cwd: None,
            env: Default::default(),
            kind: ProfileKind::Custom,
        };
        let cwd = Path::new("C:\\project '$; & ()");
        for (name, invocation) in [
            ("opencode", "opencode --continue"),
            ("codex", "codex resume --last"),
            ("claude", "claude --continue"),
            ("omp", "omp --continue"),
        ] {
            let resumed = resumed_profile(&profile, name, cwd).unwrap().unwrap();
            assert_eq!(resumed.command, "bash.exe");
            assert_eq!(
                resumed.args.last().unwrap(),
                &format!("builtin cd -- \"$ANVIL_RESTORE_CWD\" && command {invocation}")
            );
            assert_eq!(resumed.env["ANVIL_RESTORE_CWD"], "C:/project '$; & ()");
        }
        for name in ["opencode --standalone", "opencode;whoami", "cx", "opencode\nwhoami"] {
            assert!(resumed_profile(&profile, name, cwd).unwrap().is_none());
        }
        profile.args.clear();
        let resumed = resumed_profile(&profile, "opencode", cwd).unwrap().unwrap();
        assert_eq!(&resumed.args[..2], ["-i", "-c"], "a plain Bash profile must still load .bashrc");
        profile.kind = ProfileKind::Wsl;
        assert!(resumed_profile(&profile, "opencode", cwd).unwrap().is_none());
    }

    #[test]
    fn only_fixed_interactive_continue_commands_are_restored() {
        assert_eq!(continue_args("codex"), Some(["resume", "--last"].as_slice()));
        for name in ["claude", "opencode", "omp"] {
            assert_eq!(continue_args(name), Some(["--continue"].as_slice()));
        }
        for name in
            ["claude --dangerously-skip-permissions", "cmd", "powershell", "../../agent.exe", "codex\nwhoami", ""]
        {
            assert!(continue_args(name).is_none());
        }
    }
}
