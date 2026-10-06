//! Inference-only CLI profiles. Configuration is projected as data, never copied wholesale.
use std::path::{Path, PathBuf};
use std::process::Command;

// The request-shape regression test (src/git.rs) exercises this module even
// when the optional HTTP client is not part of the build.
#[cfg(any(feature = "codex", test))]
#[path = "ai_codex.rs"]
mod ai_codex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Backend {
    Claude,
    OpenCode,
    Codex,
    Gemini,
    Aider,
    Custom,
}

pub(super) struct Invocation {
    pub program: Option<Command>,
    pub directory: tempfile::TempDir,
    pub backend: Backend,
    pub model: Option<String>,
}

/// The credential values this invocation hands the child, so a caller that
/// surfaces the child's diagnostics can scrub them first. Only environment
/// names that actually carry a secret qualify: a model name redacted out of an
/// error message would cost more than it protects.
pub(super) fn command_secrets(command: &Command) -> Vec<String> {
    command
        .get_envs()
        .filter_map(|(key, value)| {
            let name = key.to_string_lossy().to_ascii_uppercase();
            let is_secret = ["KEY", "TOKEN", "SECRET", "PASSWORD"].iter().any(|marker| name.contains(marker));
            is_secret
                .then(|| value.map(|value| value.to_string_lossy().into_owned()))
                .flatten()
                .filter(|value| value.len() >= 8)
        })
        .collect()
}

/// Replaces every known secret in `text` with an ellipsis.
pub(super) fn redact(text: &str, secrets: &[String]) -> String {
    secrets.iter().fold(text.to_owned(), |text, secret| text.replace(secret.as_str(), "…"))
}

pub(super) fn words(spec: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = spec.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && quote == Some('"') {
            let mut count = 1;
            while chars.peek() == Some(&'\\') {
                count += 1;
                chars.next();
            }
            if chars.peek() == Some(&'"') {
                word.extend(std::iter::repeat_n('\\', count / 2));
                if count % 2 != 0 {
                    chars.next();
                    word.push('"');
                }
            } else {
                word.extend(std::iter::repeat_n('\\', count));
            }
            started = true;
        } else if matches!(ch, '\'' | '"') && (quote.is_none() || quote == Some(ch)) {
            quote = if quote.is_some() { None } else { Some(ch) };
            started = true;
        } else if ch.is_whitespace() && quote.is_none() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(ch);
            started = true;
        }
    }
    if quote.is_some() {
        return Err("AI: незакрытая кавычка в команде".to_owned());
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

fn classify_backend(program: &str) -> Backend {
    // Both separators matter even when tests run on a non-Windows host.
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program).to_ascii_lowercase();
    let stem = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".cmd"))
        .or_else(|| name.strip_suffix(".bat"))
        .or_else(|| name.strip_suffix(".ps1"))
        .unwrap_or(&name);
    match stem {
        "claude" => Backend::Claude,
        "opencode" => Backend::OpenCode,
        "codex" => Backend::Codex,
        "gemini" => Backend::Gemini,
        "aider" => Backend::Aider,
        _ => Backend::Custom,
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

fn read_config(path: &Path) -> Result<Option<String>, String> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > 2 * 1024 * 1024 => Err(format!("AI: конфигурация слишком велика: {}", path.display())),
        Ok(_) => std::fs::read_to_string(path).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn project(value: &serde_json::Value, keys: &[&str]) -> serde_json::Value {
    serde_json::Value::Object(
        keys.iter().filter_map(|key| value.get(*key).map(|v| ((*key).to_owned(), v.clone()))).collect(),
    )
}

fn json_config(path: &Path) -> Result<serde_json::Value, String> {
    read_config(path)?.map_or(Ok(serde_json::json!({})), |text| {
        json5::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    })
}

fn save(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, serde_json::to_vec(value).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn strip_runtime_injection(command: &mut Command) {
    for key in ["NODE_OPTIONS", "NODE_PATH", "BUN_OPTIONS", "BUN_PRELOAD", "PYTHONPATH", "PYTHONSTARTUP", "CLAUDECODE"]
    {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if name.starts_with("OPENCODE_") || name.starts_with("GEMINI_CLI_") {
            command.env_remove(key);
        }
    }
}

pub(super) fn prepare(spec: &str) -> Result<Invocation, String> {
    let mut parts = words(spec)?.into_iter();
    let program =
        parts.next().filter(|s| !s.is_empty()).ok_or_else(|| crate::strings::WORKSPACE_NO_AI_COMMAND.to_owned())?;
    let backend = classify_backend(&program);
    let args: Vec<String> = parts.collect();
    #[cfg(windows)]
    let backend = if backend == Backend::Custom {
        let explicit = Path::new(&program);
        let shim = if explicit.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
            Some(if explicit.extension().is_some_and(|e| e.eq_ignore_ascii_case("ps1")) {
                explicit.with_extension("cmd")
            } else {
                explicit.to_path_buf()
            })
        } else {
            std::env::var_os("PATH").and_then(|path| super::find_shim(&program, &path))
        };
        shim.and_then(|shim| super::npm_shim_targets(&shim)).map_or(backend, |(_, script)| {
            let path = script.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
            if path.contains("@anthropic-ai/claude-code/") {
                Backend::Claude
            } else if path.contains("@openai/codex/") {
                Backend::Codex
            } else if path.contains("@google/gemini-cli/") {
                Backend::Gemini
            } else {
                backend
            }
        })
    } else {
        backend
    };
    // A shell/node wrapper around a known agent is not an escape hatch from its safe profile.
    if backend == Backend::Custom && args.iter().any(|arg| wrapper_hides_agent(arg)) {
        return Err("AI: укажите исполняемый файл AI напрямую, без shell/node-обёртки".to_owned());
    }
    let directory = {
        sweep_stale_ai_state();
        tempfile::Builder::new().prefix("anvil-ai-").tempdir().map_err(|e| format!("AI: {e}"))?
    };
    let mut command = super::ai_cli_command(&program);
    strip_runtime_injection(&mut command);
    if backend == Backend::Custom {
        // Explicit custom commands are user code, not claimed to be inference-only.
        command.args(args);
    } else {
        let mut model = None;
        let mut variant = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if matches!(arg.as_str(), "run" | "exec" | "-p" | "--print") {
                continue;
            }
            match arg.as_str() {
                "-m" | "--model" => model = Some(args.next().ok_or_else(|| "AI: не указана модель".to_owned())?),
                "--variant" if backend == Backend::OpenCode => {
                    variant = Some(args.next().ok_or_else(|| "AI: не указан вариант модели".to_owned())?)
                }
                _ if arg.starts_with("--model=") => model = Some(arg[8..].to_owned()),
                _ => {
                    return Err(format!(
                        "AI: параметр {arg} не разрешён в безопасном режиме генерации; разрешён --model"
                    ))
                }
            }
        }
        match backend {
            Backend::Claude => claude(&mut command)?,
            Backend::OpenCode => opencode(&mut command, directory.path(), false)?,
            Backend::Aider => aider(&mut command, directory.path())?,
            // Codex starts no process: its profile is native inference only.
            Backend::Codex => return Ok(Invocation { program: None, directory, backend, model }),
            Backend::Gemini => gemini(&mut command, directory.path())?,
            Backend::Custom => unreachable!(),
        }
        if let Some(model) = model {
            command.args(["--model", &model]);
        }
        if let Some(variant) = variant {
            command.args(["--variant", &variant]);
        }
        command.current_dir(directory.path()).env("GIT_TERMINAL_PROMPT", "0");
        return Ok(Invocation { program: Some(command), directory, backend, model: None });
    }
    command.current_dir(directory.path()).env("GIT_TERMINAL_PROMPT", "0");
    Ok(Invocation { program: Some(command), directory, backend, model: None })
}

#[cfg(feature = "codex")]
fn codex_generate_impl(model: Option<&str>, prompt: &str, timeout: std::time::Duration) -> Result<String, String> {
    let home = config_home().ok_or_else(|| "Codex: не найден каталог ~/.codex".to_owned())?;
    ai_codex::generate(&home, model, prompt, timeout)
}

/// Native Codex inference: the official Responses request, never its agent.
/// Without the `codex` feature the HTTP client is not linked, so the backend
/// reports how to get it instead of silently degrading to the CLI's agent.
pub(super) fn codex_generate(
    model: Option<&str>,
    prompt: &str,
    timeout: std::time::Duration,
) -> Result<String, String> {
    #[cfg(feature = "codex")]
    {
        codex_generate_impl(model, prompt, timeout)
    }
    #[cfg(not(feature = "codex"))]
    {
        let _ = (model, prompt, timeout);
        Err("Codex: эта сборка ANVIL без бэкенда Codex (меньше размер и зависимости). Соберите с `--features codex` или выберите другой AI-бэкенд".to_owned())
    }
}

/// The shape a Codex request has: URL, headers, body, and the token carried in
/// the `Authorization` header.
#[cfg(test)]
pub(super) type CodexRequest = (String, Vec<(String, String)>, serde_json::Value, String);

#[cfg(test)]
pub(super) fn codex_request_for_tests(home: &Path, model: Option<&str>, prompt: &str) -> Result<CodexRequest, String> {
    let request = ai_codex::request(home, model, prompt)?;
    Ok((
        request.url,
        request.headers.into_iter().map(|(name, value)| (name.to_owned(), value)).collect(),
        request.body,
        request.token,
    ))
}

#[cfg(feature = "codex")]
fn config_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| home().map(|home| home.join(".codex")))
}

fn wrapper_hides_agent(arg: &str) -> bool {
    let path = arg.replace('\\', "/").to_ascii_lowercase();
    if ["@anthropic-ai/claude-code/", "@openai/codex/", "@google/gemini-cli/", "opencode-ai/"]
        .iter()
        .any(|name| path.contains(name))
    {
        return true;
    }
    // `cmd /c claude ...`, `sh -c "gemini -p ..."` and similar shell text.
    // cmd accepts /c and /k without a separating space; words() has already
    // removed surrounding quotes from the user's command specification.
    let path = path.strip_prefix("/c").or_else(|| path.strip_prefix("/k")).unwrap_or(&path);
    path.split_whitespace()
        .next()
        .is_some_and(|first| classify_backend(first.trim_matches(['\'', '"'])) != Backend::Custom)
}

#[cfg(test)]
#[test]
fn command_words_count_backslashes_before_quotes() {
    assert_eq!(words(r#""C:\Tools\App\\" --model "a\\\"b""#).unwrap(), [r"C:\Tools\App\", r#"--model"#, r#"a\"b"#]);
    assert_eq!(words(r#""C:\Program Files\agent.exe" """#).unwrap(), [r"C:\Program Files\agent.exe", ""]);
    assert!(words(r#""unterminated\""#).is_err());
    assert!(wrapper_hides_agent("/cclaude"));
    assert!(wrapper_hides_agent("/k\"codex\""));
    assert!(!wrapper_hides_agent("/cecho test"));
}

/// Environment values Claude Code reads: credentials and provider routing.
/// They reach the child through its environment, never through `--settings` —
/// a command line is visible to every process of the same user and lands in
/// audit/EDR logs (Sysmon 4688).
const CLAUDE_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
    "AWS_REGION",
];

/// The `--settings` document and the environment it must not carry: inert model
/// preferences plus `disableAllHooks`, and the projected env pairs separately.
fn claude_settings(source: &serde_json::Value, with_user_settings: bool) -> (serde_json::Value, Vec<(String, String)>) {
    let mut settings = serde_json::json!({ "disableAllHooks": true });
    if with_user_settings {
        merge(&mut settings, project(source, &["model", "effortLevel", "modelSettings"]));
    }
    let mut environment = Vec::new();
    if let Some(values) = source.get("env").and_then(serde_json::Value::as_object) {
        for key in CLAUDE_ENV_KEYS {
            if let Some(text) = values.get(*key).and_then(serde_json::Value::as_str) {
                environment.push(((*key).to_owned(), text.to_owned()));
            }
        }
    }
    (settings, environment)
}

/// Credential copies (Gemini OAuth files, aider API keys) live inside
/// `anvil-ai-*` directories. Normal completion removes them, but a crash, a
/// kill or a child still holding a file leaves one behind, so directories older
/// than this are swept before the next generation starts — and once at startup,
/// since nothing else would ever clean them. A generation that is running now is
/// younger than the threshold and stays.
const STALE_AI_STATE_AGE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Sweeps what a dead generation left behind: its credential copies must not
/// survive it, whether the run crashed or the machine went down.
pub(super) fn sweep_stale_ai_state() {
    sweep_stale_directories(STALE_AI_STATE_AGE);
}

fn sweep_stale_directories(max_age: std::time::Duration) {
    sweep_stale_directories_in(&std::env::temp_dir(), max_age);
}

fn sweep_stale_directories_in(root: &Path, max_age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("anvil-ai-") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        let stale = meta.modified().ok().and_then(|modified| modified.elapsed().ok()).is_some_and(|age| age >= max_age);
        if stale {
            if let Err(error) = std::fs::remove_dir_all(entry.path()) {
                log::info!("cannot remove stale AI directory {}: {error}", entry.path().display());
            }
        }
    }
}

/// Applies the fixed Claude flags, the credential-free `--settings` document and
/// the projected environment. The split exists so a test can hold the invariant
/// that secrets travel in the environment and never on the command line.
fn apply_claude_session(command: &mut Command, settings: &serde_json::Value, environment: &[(String, String)]) {
    for (key, value) in environment {
        command.env(key, value);
    }
    command
        .args([
            "--print",
            "--restricted",
            "--setting-sources",
            "",
            "--tools",
            "",
            "--disallowedTools",
            "*",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--disable-slash-commands",
            "--no-chrome",
            "--no-session-persistence",
            "--output-format",
            "json",
            "--settings",
        ])
        .arg(settings.to_string());
    command.env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
}

fn claude(command: &mut Command) -> Result<(), String> {
    // OAuth remains in the original auth directory, but executable helpers and
    // env injection from settings are not inherited. Only inert model/auth data.
    let root = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| home().map(|h| h.join(".claude")));
    let source = match root.as_ref() {
        Some(root) => json_config(&root.join("settings.json"))?,
        None => serde_json::json!({}),
    };
    let (settings, environment) = claude_settings(&source, root.is_some());
    let static_auth = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some() || environment.iter().any(|(name, _)| name == key));
    if static_auth {
        if let Some(root) = root.as_deref() {
            check_claude_managed_policy(root, false)?;
        }
        command.arg("--bare");
    } else {
        let root = root.as_deref().ok_or_else(|| "AI: невозможно проверить политику Claude".to_owned())?;
        check_claude_managed_policy(root, true)?;
        // Personal pro/max accounts do not fetch organization-managed hooks.
        // Enterprise/token-only accounts cannot prove this contract before launch.
        let credentials = json_config(&root.join(".credentials.json"))?;
        let subscription = credentials.pointer("/claudeAiOauth/subscriptionType").and_then(serde_json::Value::as_str);
        if !matches!(subscription, Some("pro" | "max")) {
            return Err("AI: административные hooks Claude не позволяют гарантировать генерацию без выполнения команд. Используйте API-ключ (режим --bare), другого AI-бэкенда или личную подписку pro/max".to_owned());
        }
        command.arg("--safe-mode");
    }
    apply_claude_session(command, &settings, &environment);
    Ok(())
}

fn opencode(command: &mut Command, dir: &Path, models: bool) -> Result<(), String> {
    let config_root =
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".config")));
    let mut config = serde_json::json!({});
    if let Some(root) = config_root {
        for name in ["config.json", "opencode.json", "opencode.jsonc"] {
            let safe = opencode_provider_config(&json_config(&root.join("opencode").join(name))?)?;
            merge(&mut config, safe);
        }
    }
    if let Some(path) = std::env::var_os("OPENCODE_CONFIG") {
        merge(&mut config, opencode_provider_config(&json_config(Path::new(&path))?)?);
    }
    if let Ok(text) = std::env::var("OPENCODE_CONFIG_CONTENT") {
        let value = json5::from_str(&text).map_err(|e| format!("OPENCODE_CONFIG_CONTENT: {e}"))?;
        merge(&mut config, opencode_provider_config(&value)?);
    }
    let config = super::opencode_commit_config(Some(&config.to_string()))?;
    // Credentials are injected as data (OPENCODE_AUTH_CONTENT), so the real data
    // directory is never read: no plugins, sessions or caches from the user.
    let data_root =
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".local/share")));
    let mut auth = None;
    if let Some(root) = data_root {
        if let Some(text) = read_config(&root.join("opencode/auth.json"))? {
            if let Ok(serde_json::Value::Object(entries)) = serde_json::from_str::<serde_json::Value>(&text) {
                // An OAuth session is refreshed on use and the rotated tokens are
                // written under the isolated data root, which would invalidate the
                // user's real login. Only static credentials are projected.
                let static_entries: serde_json::Map<String, serde_json::Value> = entries
                    .into_iter()
                    .filter(|(_, value)| value.get("type").and_then(serde_json::Value::as_str) != Some("oauth"))
                    .collect();
                if !static_entries.is_empty() {
                    auth = Some(serde_json::Value::Object(static_entries).to_string());
                }
            }
        }
    }
    let private = dir.join("home");
    for name in ["data", "cache", "state", "config"] {
        std::fs::create_dir_all(private.join(name)).map_err(|e| e.to_string())?;
    }
    // The last chosen model is inert selection data, not a session or plugin.
    let state_root =
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".local/state")));
    if let Some(root) = state_root {
        if let Some(text) = read_config(&root.join("opencode/model.json"))? {
            if serde_json::from_str::<serde_json::Value>(&text).is_ok_and(|value| value.is_object()) {
                let target = private.join("state/opencode");
                std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
                std::fs::write(target.join("model.json"), text).map_err(|e| e.to_string())?;
            }
        }
    }
    if models {
        command.args(["models", "--pure"]);
    } else {
        command.args([
            "run",
            "--pure",
            "--agent",
            "anvil-commit",
            "--format",
            "json",
            "--title",
            "ANVIL commit message",
        ]);
    }
    command
        .env("XDG_CONFIG_HOME", private.join("config"))
        .env("XDG_DATA_HOME", private.join("data"))
        .env("XDG_CACHE_HOME", private.join("cache"))
        .env("XDG_STATE_HOME", private.join("state"))
        .env("OPENCODE_TEST_HOME", &private)
        .env("OPENCODE_CONFIG_DIR", private.join("config/opencode"))
        .env_remove("OPENCODE_CONFIG")
        .env("OPENCODE_CONFIG_CONTENT", config)
        .env("OPENCODE_DISABLE_PROJECT_CONFIG", "true")
        .env("OPENCODE_DISABLE_AUTOUPDATE", "true");
    if let Some(auth) = auth {
        command.env("OPENCODE_AUTH_CONTENT", auth);
    }
    Ok(())
}

fn merge(target: &mut serde_json::Value, source: serde_json::Value) {
    if let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) {
        for (key, value) in source {
            if value.is_object() && target.get(key).is_some_and(serde_json::Value::is_object) {
                merge(target.get_mut(key).unwrap(), value.clone());
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

fn opencode_provider_config(value: &serde_json::Value) -> Result<serde_json::Value, String> {
    let mut safe = project(value, &["model", "small_model", "enabled_providers", "disabled_providers"]);
    if let Some(providers) = value.get("provider").and_then(serde_json::Value::as_object) {
        let mut result = serde_json::Map::new();
        for (id, provider) in providers {
            // Model-level provider overrides can also import an executable SDK.
            check_bundled_sdks(provider, id)?;
            result.insert(
                id.clone(),
                project(provider, &["npm", "name", "api", "env", "models", "options", "whitelist", "blacklist"]),
            );
        }
        safe["provider"] = result.into();
    }
    Ok(safe)
}

fn check_bundled_sdks(value: &serde_json::Value, provider: &str) -> Result<(), String> {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                if key == "npm" {
                    let npm = value.as_str().ok_or_else(|| "AI: OpenCode npm должен быть строкой".to_owned())?;
                    if !matches!(
                        npm,
                        "@ai-sdk/openai"
                            | "@ai-sdk/openai-compatible"
                            | "@ai-sdk/anthropic"
                            | "@ai-sdk/google"
                            | "@ai-sdk/google-vertex"
                            | "@ai-sdk/google-vertex/anthropic"
                            | "@ai-sdk/amazon-bedrock"
                            | "@ai-sdk/amazon-bedrock/mantle"
                            | "@ai-sdk/azure"
                            | "@openrouter/ai-sdk-provider"
                            | "@ai-sdk/xai"
                            | "@ai-sdk/mistral"
                            | "@ai-sdk/groq"
                            | "@ai-sdk/deepinfra"
                            | "@ai-sdk/cerebras"
                            | "@ai-sdk/cohere"
                            | "@ai-sdk/gateway"
                            | "@ai-sdk/togetherai"
                            | "@ai-sdk/perplexity"
                            | "@ai-sdk/vercel"
                            | "@ai-sdk/alibaba"
                            | "gitlab-ai-provider"
                            | "@ai-sdk/github-copilot"
                            | "venice-ai-sdk-provider"
                    ) {
                        return Err(format!("AI: OpenCode provider {provider} использует исполняемый SDK {npm}, запрещённый для генерации коммита"));
                    }
                } else {
                    check_bundled_sdks(value, provider)?;
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                check_bundled_sdks(value, provider)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn aider(command: &mut Command, dir: &Path) -> Result<(), String> {
    let python = aider_python(command)?;
    *command = Command::new(python);
    command.arg("-I");
    strip_runtime_injection(command);
    // `verify-ssl` is deliberately not projected: turning certificate
    // verification off would put the provider key on the wire to whoever is
    // in the middle, and a commit-message generator gains nothing from it.
    const KEYS: &[&str] = &[
        "model",
        "openai-api-key",
        "anthropic-api-key",
        "openai-api-base",
        "openai-api-version",
        "openai-api-deployment-id",
        "openai-organization-id",
        "api-key",
        "reasoning-effort",
        "thinking-tokens",
        "timeout",
        "alias",
    ];
    let mut config = serde_json::json!({});
    if let Some(home) = home() {
        if let Some(text) = read_config(&home.join(".aider.conf.yml"))? {
            let value: serde_json::Value =
                serde_yaml_ng::from_str(&text).map_err(|e| format!(".aider.conf.yml: {e}"))?;
            config = project(&value, KEYS);
        }
    }
    for (key, value) in std::env::vars_os() {
        let upper = key.to_string_lossy().to_ascii_uppercase();
        if let Some(name) = upper.strip_prefix("AIDER_") {
            let name = name.to_ascii_lowercase().replace('_', "-");
            if KEYS.contains(&name.as_str()) {
                config[&name] = if matches!(name.as_str(), "api-key" | "alias") {
                    serde_json::json!([value.to_string_lossy().into_owned()])
                } else {
                    value.to_string_lossy().into_owned().into()
                };
            }
            command.env_remove(key);
        }
    }
    // The projected config carries API keys. It travels in the child's
    // environment, never as a file: a plain write into %TEMP% leaves a copy
    // readable by anything running as this user until the directory is swept,
    // with whatever DACL the temp folder happens to carry.
    let config = serde_json::to_string(&config).map_err(|e| e.to_string())?;
    let driver = dir.join("inference.py");
    std::fs::write(&driver, include_str!("ai_inference_aider.py")).map_err(|e| e.to_string())?;
    command.arg(driver).env("ANVIL_AIDER_CONFIG", config).env("HOME", dir).env("USERPROFILE", dir);
    Ok(())
}

fn aider_python(command: &Command) -> Result<PathBuf, String> {
    let program = PathBuf::from(command.get_program());
    let executable = if program.is_absolute() {
        Some(program)
    } else {
        #[cfg(windows)]
        {
            super::find_on_path("aider.exe")
        }
        #[cfg(not(windows))]
        {
            std::env::var_os("PATH")
                .and_then(|path| std::env::split_paths(&path).map(|dir| dir.join(&program)).find(|p| p.is_file()))
        }
    }
    .ok_or_else(|| "AI: не найден установленный Aider".to_owned())?;
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(&executable)
        .map_err(|e| e.to_string())?
        .take(2 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    // pip/distlib and uv Windows launchers carry an interpreter shebang. Read
    // it as data; never execute the launcher/CLI to discover an interpreter.
    for start in bytes.windows(2).enumerate().filter_map(|(i, pair)| (pair == b"#!").then_some(i + 2)) {
        let line = &bytes[start..];
        let end = line.iter().position(|b| matches!(b, b'\n' | 0)).unwrap_or(line.len());
        if let Ok(line) = std::str::from_utf8(&line[..end]) {
            let path = PathBuf::from(line.trim().trim_matches('"'));
            if path.is_absolute()
                && path.is_file()
                && path.file_stem().is_some_and(|name| name.to_string_lossy().starts_with("python"))
            {
                return Ok(path);
            }
        }
    }
    let adjacent = executable.parent().unwrap_or(Path::new("")).join("python.exe");
    if adjacent.is_file() {
        return Ok(adjacent);
    }
    Err("AI: не найден Python установленного Aider; требуется pip/uv установка с доступным интерпретатором".to_owned())
}

pub(super) fn prepare_models() -> Result<Invocation, String> {
    let directory = tempfile::Builder::new().prefix("anvil-ai-").tempdir().map_err(|e| e.to_string())?;
    let mut command = super::ai_cli_command("opencode");
    strip_runtime_injection(&mut command);
    opencode(&mut command, directory.path(), true)?;
    command.current_dir(directory.path());
    Ok(Invocation { program: Some(command), directory, backend: Backend::OpenCode, model: None })
}

pub(super) fn response(backend: Backend, output: &str) -> Result<String, String> {
    match backend {
        Backend::OpenCode => super::opencode_message(output),
        Backend::Claude | Backend::Gemini | Backend::Aider => {
            let value: serde_json::Value = serde_json::from_str(output).map_err(|e| format!("{backend:?}: {e}"))?;
            if value.get("is_error").and_then(serde_json::Value::as_bool) == Some(true) || value.get("error").is_some()
            {
                return Err(value
                    .get("error")
                    .or_else(|| value.get("result"))
                    .map(ToString::to_string)
                    .unwrap_or_else(|| output.to_owned()));
            }
            value
                .get(if backend == Backend::Gemini { "response" } else { "result" })
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("{backend:?}: ответ не содержит текст сообщения"))
        }
        Backend::Codex | Backend::Custom => Ok(output.to_owned()),
    }
}

fn gemini(command: &mut Command, dir: &Path) -> Result<(), String> {
    let original_home = std::env::var_os("GEMINI_CLI_HOME")
        .map(PathBuf::from)
        .or_else(home)
        .ok_or_else(|| "AI: не найден каталог авторизации Gemini".to_owned())?;
    let original = original_home.join(".gemini");
    let source = json_config(&original.join("settings.json"))?;
    let mut settings = serde_json::json!({
        "tools": { "core": [] }, "hooksConfig": { "enabled": false }
    });
    if let Some(model) = source.get("model") {
        settings["model"] = project(model, &["name"]);
    }
    if let Some(auth) = source.pointer("/security/auth") {
        settings["security"] = serde_json::json!({ "auth": project(auth, &["selectedType", "useExternal"]) });
    }
    let private = dir.join("home/.gemini");
    save(&private.join("settings.json"), &settings)?;
    for file in ["oauth_creds.json", "google_accounts.json"] {
        if let Some(text) = read_config(&original.join(file))? {
            std::fs::write(private.join(file), text).map_err(|e| e.to_string())?;
        }
    }
    let system = dir.join("system-settings.json");
    save(&system, &settings)?;
    // -e none keeps extensions (and their tools/MCP/hooks) out; the allowlist
    // sentinel cannot match a server name, blocking every configured MCP server
    // before it connects; --skip-trust trusts only this private directory.
    command
        .args([
            "-e",
            "none",
            "--skip-trust",
            "--allowed-mcp-server-names",
            "anvil-block-all",
            "--output-format",
            "json",
        ])
        .env("GEMINI_CLI_HOME", dir.join("home"))
        .env("GEMINI_CLI_SYSTEM_SETTINGS_PATH", system)
        .env("GEMINI_CLI_SYSTEM_DEFAULTS_PATH", dir.join("absent-defaults.json"))
        .env("GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES", "0");
    Ok(())
}

pub(super) fn policy_is_executable(value: &serde_json::Value, hooks_active: bool) -> bool {
    let mcp = ["managedMcpServers"].iter().any(|key| {
        value.get(*key).is_some_and(|v| !v.is_null() && v != &serde_json::json!({}) && v != &serde_json::json!([]))
    });
    // Helpers below are read and executed by Claude Code itself, whatever the
    // hooks setting says; the hook machinery only matters in the safe-mode case.
    let helper = ["apiKeyHelper", "awsAuthRefresh", "awsCredentialExport", "otelHeadersHelper"].iter().any(|key| {
        value.get(*key).is_some_and(|v| !v.is_null() && v != &serde_json::json!({}) && v != &serde_json::json!([]))
    });
    let hooks = hooks_active
        && [
            "hooks",
            "policyHelper",
            "statusLine",
            "fileSuggestion",
            "subagentStatusLine",
            "enabledPlugins",
            "extraKnownMarketplaces",
        ]
        .iter()
        .any(|key| {
            value.get(*key).is_some_and(|v| !v.is_null() && v != &serde_json::json!({}) && v != &serde_json::json!([]))
        });
    mcp || helper
        || hooks
        || value.get("env").and_then(serde_json::Value::as_object).is_some_and(|env| {
            env.keys().any(|key| {
                matches!(
                    key.to_ascii_uppercase().as_str(),
                    "NODE_OPTIONS"
                        | "NODE_PATH"
                        | "BUN_OPTIONS"
                        | "BUN_PRELOAD"
                        | "PYTHONPATH"
                        | "PYTHONSTARTUP"
                        | "CLAUDE_CONFIG_DIR"
                        | "CLAUDE_CODE_MANAGED_SETTINGS_PATH"
                        | "CLAUDE_CODE_REMOTE_SETTINGS_PATH"
                )
            })
        })
}

fn check_claude_managed_policy(root: &Path, hooks_active: bool) -> Result<(), String> {
    let error = "AI: управляемая политика Claude содержит hooks/MCP/исполняемые настройки; генерация без выполнения команд невозможна. Используйте API-ключ (--bare), другого AI-бэкенда или обратитесь к администратору";
    let mut paths = vec![root.join("remote-settings.json")];
    for key in ["CLAUDE_CODE_MANAGED_SETTINGS_PATH", "CLAUDE_CODE_REMOTE_SETTINGS_PATH"] {
        if let Some(path) = std::env::var_os(key) {
            paths.push(path.into());
        }
    }
    #[cfg(windows)]
    let system = PathBuf::from("C:\\Program Files\\ClaudeCode");
    #[cfg(not(windows))]
    let system = PathBuf::from("/etc/claude-code");
    paths.push(system.join("managed-settings.json"));
    paths.push(system.join("managed-mcp.json"));
    match std::fs::read_dir(system.join("managed-settings.d")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|e| format!("AI: не удалось проверить managed settings: {e}"))?;
                let name = entry.file_name();
                if !name.to_string_lossy().starts_with('.') && entry.path().extension().is_some_and(|e| e == "json") {
                    paths.push(entry.path());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("AI: не удалось проверить managed settings: {e}")),
    }
    for path in paths {
        let is_mcp = path.file_name().is_some_and(|n| n == "managed-mcp.json");
        let value = json_config(&path)?;
        if policy_is_executable(&value, hooks_active) || (is_mcp && value != serde_json::json!({})) {
            return Err(error.to_owned());
        }
    }
    #[cfg(windows)]
    for value in claude_registry_policies()? {
        if policy_is_executable(&value, hooks_active) {
            return Err(error.to_owned());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn claude_registry_policies() -> Result<Vec<serde_json::Value>, String> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY,
        KEY_WOW64_64KEY, REG_EXPAND_SZ, REG_SZ,
    };
    let name: Vec<u16> = "SOFTWARE\\Policies\\ClaudeCode\0".encode_utf16().collect();
    let setting: Vec<u16> = "Settings\0".encode_utf16().collect();
    let mut policies = Vec::new();
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            let mut key = std::ptr::null_mut();
            // SAFETY: NUL-terminated names and valid out parameters; every opened key is closed.
            let status = unsafe { RegOpenKeyExW(root, name.as_ptr(), 0, KEY_READ | view, &mut key) };
            if status == ERROR_FILE_NOT_FOUND {
                continue;
            }
            if status != ERROR_SUCCESS {
                return Err(format!("AI: не удалось проверить registry policy Claude ({status})"));
            }
            let result = (|| {
                let mut kind = 0;
                let mut bytes = 0;
                // SAFETY: query size without an output buffer.
                let status = unsafe {
                    RegQueryValueExW(
                        key,
                        setting.as_ptr(),
                        std::ptr::null(),
                        &mut kind,
                        std::ptr::null_mut(),
                        &mut bytes,
                    )
                };
                if status == ERROR_FILE_NOT_FOUND {
                    return Ok(None);
                }
                if status != ERROR_SUCCESS
                    || !matches!(kind, REG_SZ | REG_EXPAND_SZ)
                    || bytes > 2 * 1024 * 1024
                    || bytes % 2 != 0
                {
                    return Err(format!("AI: не удалось прочитать registry policy Claude ({status})"));
                }
                let mut utf16 = vec![0u16; bytes as usize / 2];
                // SAFETY: allocated buffer is bytes long, returned size is bounded again below.
                let status = unsafe {
                    RegQueryValueExW(
                        key,
                        setting.as_ptr(),
                        std::ptr::null(),
                        &mut kind,
                        utf16.as_mut_ptr().cast(),
                        &mut bytes,
                    )
                };
                if status != ERROR_SUCCESS {
                    return Err(format!("AI: не удалось прочитать registry policy Claude ({status})"));
                }
                let text = String::from_utf16(&utf16).map_err(|e| e.to_string())?;
                let value = serde_json::from_str(text.trim_end_matches('\0'))
                    .map_err(|e| format!("AI: registry policy Claude: {e}"))?;
                Ok(Some(value))
            })();
            // SAFETY: key was successfully opened above.
            unsafe {
                RegCloseKey(key);
            }
            if let Some(value) = result? {
                policies.push(value);
            }
        }
    }
    Ok(policies)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider token must never reach the command line: `--settings` is
    /// visible to every process of the user and to audit/EDR logs, while the
    /// environment is not. The JSON the CLI receives carries no credentials.
    #[test]
    fn claude_settings_carry_no_credentials() {
        let source = serde_json::json!({
            "model": "opus",
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "secret-token",
                "ANTHROPIC_BASE_URL": "https://proxy.example",
                "EVIL_TYPO": "not-projected"
            }
        });
        let (settings, environment) = claude_settings(&source, true);
        let serialized = settings.to_string();
        assert!(!serialized.contains("secret-token"), "the token must not be serialized: {serialized}");
        assert_eq!(settings.get("disableAllHooks"), Some(&serde_json::json!(true)));
        assert_eq!(settings.get("model").and_then(serde_json::Value::as_str), Some("opus"));
        assert!(settings.get("env").is_none(), "settings must never carry an env object");
        assert!(environment.iter().any(|(key, value)| key == "ANTHROPIC_AUTH_TOKEN" && value == "secret-token"));
        assert!(environment.iter().any(|(key, _)| key == "ANTHROPIC_BASE_URL"));
        assert!(!environment.iter().any(|(key, _)| key == "EVIL_TYPO"));
    }

    /// The invariant end to end: what `claude()` hands to the process.
    #[test]
    fn claude_command_keeps_credentials_out_of_argv() {
        let source = serde_json::json!({
            "model": "sonnet",
            "env": { "ANTHROPIC_AUTH_TOKEN": "sk-ant-secret", "ANTHROPIC_BASE_URL": "https://proxy.example" }
        });
        let (settings, environment) = claude_settings(&source, true);
        let mut command = Command::new("claude.exe");
        apply_claude_session(&mut command, &settings, &environment);
        let args: Vec<String> = command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
        assert!(
            !args.iter().any(|arg| arg.contains("sk-ant-secret")),
            "a credential reached the command line: {args:?}"
        );
        let document = args
            .iter()
            .position(|arg| arg == "--settings")
            .and_then(|index| args.get(index + 1))
            .expect("--settings carries a document");
        let document: serde_json::Value = serde_json::from_str(document).unwrap();
        assert!(document.get("env").is_none(), "--settings must not carry env: {document}");
        let environment: Vec<String> = command
            .get_envs()
            .filter_map(|(key, value)| Some(format!("{}={}", key.to_string_lossy(), value?.to_string_lossy())))
            .collect();
        assert!(
            environment.iter().any(|pair| pair == "ANTHROPIC_AUTH_TOKEN=sk-ant-secret"),
            "the token must travel in the environment: {environment:?}"
        );
    }

    /// A CLI that prints its own `Authorization` header on failure must not put
    /// the key in the panel. Only environment entries that look like secrets
    /// are scrubbed, so a model name in the same message survives for the user.
    #[test]
    fn child_diagnostics_are_scrubbed_of_the_credentials_we_handed_over() {
        let mut command = Command::new("echo");
        command.env("ANTHROPIC_AUTH_TOKEN", "sk-ant-very-secret-value");
        command.env("ANTHROPIC_MODEL", "claude-opus-5");
        command.env("HOME", "/tmp/x");
        let secrets = command_secrets(&command);
        assert_eq!(secrets, vec!["sk-ant-very-secret-value".to_owned()], "only real secrets are collected");

        let stderr = "request failed\nAuthorization: Bearer sk-ant-very-secret-value\nmodel=claude-opus-5\n";
        let scrubbed = redact(stderr, &secrets);
        assert!(!scrubbed.contains("sk-ant-very-secret-value"), "{scrubbed}");
        assert!(scrubbed.contains("claude-opus-5"), "diagnostics stay useful: {scrubbed}");
    }

    /// A short value is far more likely to be an ordinary setting that happens
    /// to sit in a secret-sounding name than a credential.
    #[test]
    fn short_environment_values_are_not_treated_as_secrets() {
        let mut command = Command::new("echo");
        command.env("ANTHROPIC_API_KEY", "short");
        assert!(command_secrets(&command).is_empty());
    }

    /// Credential copies must not survive a crash: only our own prefix is swept.
    #[test]
    fn stale_generation_directories_are_swept_by_prefix_only() {
        let root = tempfile::tempdir().unwrap();
        let stale = root.path().join("anvil-ai-dead1234");
        std::fs::create_dir_all(stale.join("home/.gemini")).unwrap();
        std::fs::write(stale.join("home/.gemini/oauth_creds.json"), "secret").unwrap();
        let foreign = root.path().join("anvil-other-work");
        std::fs::create_dir_all(&foreign).unwrap();
        sweep_stale_directories_in(root.path(), std::time::Duration::ZERO);
        assert!(!stale.exists(), "an abandoned generation directory keeps credentials");
        assert!(foreign.exists(), "directories that are not ours are untouched");
    }

    /// A provider entry can pull executable code in through `npm`; only the
    /// inert SDK names are projected, and no field that becomes a process.
    #[test]
    fn provider_projection_refuses_executable_sdks_and_drops_command_fields() {
        let evil = serde_json::json!({"provider":{"evil":{"npm":"evil-sdk","options":{"baseURL":"https://ok"}}}});
        assert!(opencode_provider_config(&evil).is_err(), "an arbitrary npm package executes user code");

        let safe = serde_json::json!({"provider":{"custom":{
            "npm":"@ai-sdk/openai-compatible",
            "options":{"baseURL":"https://example.com"},
            "headers":{"Authorization":"secret"},
            "fetch":"/bin/evil"
        }}});
        let projected = opencode_provider_config(&safe).unwrap();
        assert_eq!(projected.pointer("/provider/custom/npm").unwrap(), "@ai-sdk/openai-compatible");
        assert_eq!(projected.pointer("/provider/custom/options/baseURL").unwrap(), "https://example.com");
        assert!(
            projected.pointer("/provider/custom/headers").is_none(),
            "arbitrary headers are not inert provider data"
        );
        assert!(projected.pointer("/provider/custom/fetch").is_none(), "a custom fetch is executable user code");
    }
}
