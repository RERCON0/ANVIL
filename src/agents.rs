//! Session restart uses fixed agent names and flags, never saved command lines.
use crate::profiles::{Profile, ProfileKind};

pub(crate) fn continue_args(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "codex" => Some(&["resume", "--last"]),
        "claude" | "opencode" | "omp" => Some(&["--continue"]),
        _ => None,
    }
}

pub(crate) fn resumed_profile(profile: &Profile, name: &str) -> Result<Option<Profile>, String> {
    let Some(args) = continue_args(name) else { return Ok(None) };
    // Windows process discovery cannot identify agents inside a WSL VM.
    // A hand-edited session must not turn one into a Windows conversation.
    if profile.kind == ProfileKind::Wsl {
        return Ok(None);
    }
    let command = crate::git::installed_cli_command(name).ok_or_else(|| crate::tr_format!(
        "Cannot restore {name}: the CLI is not installed on PATH. Install it or disable Restore CLI agents.",
        "Не удалось восстановить {name}: CLI не установлен в PATH. Установите его или выключите восстановление CLI-агентов."
    ))?;
    let mut result = profile.clone();
    result.command = command.get_program().to_string_lossy().into_owned();
    result.args = command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
    result.args.extend(args.iter().map(|arg| (*arg).to_owned()));
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;

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
