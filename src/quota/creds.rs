//! Where quota logins come from. Everything here only reads: CLI login files,
//! the OMP and OpenCode 2 databases, environment variables, the `env` block of
//! Claude Code's user settings and the keys typed into ANVIL. Tokens are never
//! refreshed and refresh tokens are never read into memory. Project-level
//! configuration from repositories is never consulted.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use super::model::ProviderId;
use super::providers::{chatgpt, claude};
use super::sqlite::Sqlite;
use super::time::epoch_from_json;
use crate::strings;

/// Login files larger than this are not login files.
const MAX_FILE: u64 = 2 * 1024 * 1024;
/// A token this close to expiry is treated as expired: the request would race it.
const EXPIRY_MARGIN: i64 = 60;

/// A token or key. `Debug` never shows it.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    ClaudeCode,
    CodexCli,
    Omp,
    OpenCode,
    AnvilKey,
    Env(&'static str),
    ClaudeSettings,
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::ClaudeCode => strings::QUOTA_SOURCE_CLAUDE_CODE.to_owned(),
            Source::CodexCli => strings::QUOTA_SOURCE_CODEX.to_owned(),
            Source::Omp => strings::QUOTA_SOURCE_OMP.to_owned(),
            Source::OpenCode => strings::QUOTA_SOURCE_OPENCODE.to_owned(),
            Source::AnvilKey => strings::QUOTA_SOURCE_ANVIL_KEY.to_owned(),
            Source::Env(name) => format!("{} {name}", strings::QUOTA_SOURCE_ENV),
            Source::ClaudeSettings => strings::QUOTA_SOURCE_CLAUDE_SETTINGS.to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Credential {
    pub secret: Secret,
    pub source: Source,
    pub plan: Option<String>,
    /// ChatGPT account id for the request header; never stored or shown.
    pub account: Option<String>,
    /// OpenCode Console organisation (`x-org-id`); never stored or shown.
    pub org: Option<String>,
    /// OpenCode Console server the login belongs to, as the login says.
    pub server: Option<String>,
    pub expires_at: Option<i64>,
    /// Changes whenever the login does; kept in memory only.
    pub marker: u64,
}

#[derive(Clone, Debug)]
pub enum Detection {
    Missing,
    Found(Credential),
    /// Only expired logins were found: nothing may be requested with them.
    Expired {
        source: Source,
        marker: u64,
    },
}

/// Environment variables discovery reads; nothing else is copied.
const VARS: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "XDG_DATA_HOME",
    "PI_CODING_AGENT_DIR",
    "PI_CONFIG_DIR",
    "OMP_PROFILE",
    "PI_PROFILE",
    "OPENCODE_DB",
    "ZAI_API_KEY",
    "ZAI_CODING_PLAN_API_KEY",
    "ZHIPU_API_KEY",
    "ZHIPU_CODING_PLAN_API_KEY",
    "KIMI_API_KEY",
    "KIMI_CODE_API_KEY",
    "KIMI_CN_API_KEY",
    "KIMI_GLOBAL_API_KEY",
    "MINIMAX_CODING_PLAN_API_KEY",
    "MINIMAX_API_KEY",
    "MINIMAX_CHINA_CODING_PLAN_API_KEY",
    "OPENCODE_API_KEY",
    "SYNTHETIC_API_KEY",
    "OLLAMA_API_KEY",
    "CHUTES_API_KEY",
    "COMMAND_CODE_API_KEY",
    "DEEPSEEK_API_KEY",
    "OPENROUTER_API_KEY",
    "KILO_API_KEY",
    "CHARM_HYPER_API_KEY",
];

/// Everything discovery depends on, so tests can point it at temp folders.
pub struct CredEnv {
    pub home: Option<PathBuf>,
    pub vars: HashMap<String, String>,
    /// Keys typed into ANVIL, by provider.
    pub own_keys: HashMap<ProviderId, String>,
    pub sqlite: Option<Arc<Sqlite>>,
    pub now: i64,
}

impl CredEnv {
    pub fn from_process(now: i64, sqlite: Option<Arc<Sqlite>>, own_keys: HashMap<ProviderId, String>) -> CredEnv {
        let vars =
            VARS.iter().filter_map(|name| std::env::var(name).ok().map(|value| ((*name).to_owned(), value))).collect();
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
        CredEnv { home, vars, own_keys, sqlite, now }
    }

    fn var(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(|v| v.trim()).filter(|v| !v.is_empty())
    }

    fn under_home(&self, name: &str, default: &str) -> Option<PathBuf> {
        self.var(name).map(PathBuf::from).or_else(|| self.home.as_ref().map(|h| h.join(default)))
    }

    fn claude_dir(&self) -> Option<PathBuf> {
        self.under_home("CLAUDE_CONFIG_DIR", ".claude")
    }

    fn codex_home(&self) -> Option<PathBuf> {
        self.under_home("CODEX_HOME", ".codex")
    }

    fn data_home(&self) -> Option<PathBuf> {
        self.under_home("XDG_DATA_HOME", ".local/share")
    }

    /// OMP's `getAgentDbPath()` on Windows: `PI_CODING_AGENT_DIR`, else
    /// `~/<PI_CONFIG_DIR or .omp>[/profiles/<profile>]/agent/agent.db`.
    /// `OMP_PROFILE` wins over `PI_PROFILE` even when set but empty.
    fn omp_agent_db(&self) -> Option<PathBuf> {
        if let Some(dir) = self.var("PI_CODING_AGENT_DIR") {
            return Some(PathBuf::from(dir).join("agent.db"));
        }
        let root = self.home.as_ref()?.join(self.var("PI_CONFIG_DIR").unwrap_or(".omp"));
        let profile =
            self.vars.get("OMP_PROFILE").or_else(|| self.vars.get("PI_PROFILE")).map(|p| p.trim()).unwrap_or("");
        let valid = !profile.is_empty() && profile.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let agent = if valid { root.join("profiles").join(profile).join("agent") } else { root.join("agent") };
        Some(agent.join("agent.db"))
    }

    /// OpenCode 2: `OPENCODE_DB` (absolute or relative to its data folder),
    /// else `<data>/opencode/opencode.db`.
    fn opencode_db(&self) -> Option<PathBuf> {
        let dir = self.data_home()?.join("opencode");
        match self.var("OPENCODE_DB") {
            Some(":memory:") => None,
            Some(path) if Path::new(path).is_absolute() => Some(PathBuf::from(path)),
            Some(path) => Some(dir.join(path)),
            None => Some(dir.join("opencode.db")),
        }
    }
}

fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// A CLI may be rewriting its login file at the very moment it is read; one
/// short retry keeps the provider from vanishing for a whole cycle.
fn read_json(path: &Path) -> Option<Value> {
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        if let Some(value) = read_small(path).and_then(|text| serde_json::from_str(&text).ok()) {
            return Some(value);
        }
        if !path.is_file() {
            return None;
        }
    }
    None
}

fn marker(secret: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    secret.hash(&mut hasher);
    hasher.finish()
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn credential(secret: String, source: Source) -> Credential {
    Credential {
        marker: marker(&secret),
        secret: Secret::new(secret),
        source,
        plan: None,
        account: None,
        org: None,
        server: None,
        expires_at: None,
    }
}

/// Which credential kinds a provider's quota endpoint takes.
#[derive(Clone, Copy)]
struct Accept {
    key: bool,
    oauth: bool,
}

/// One stored login entry, shaped like OpenCode's: `{"type": "api" | "key" |
/// "api_key", "key"}` or `{"type": "oauth", "access", "expires", "accountId"}`.
/// OpenCode 2 keeps extra fields under `metadata`; they are merged under the
/// top level. `refresh` is never copied.
fn from_entry(entry: &Value, source: Source, accept: Accept) -> Option<Credential> {
    let mut merged = entry.get("metadata").and_then(Value::as_object).cloned().unwrap_or_default();
    for (k, v) in entry.as_object()? {
        merged.insert(k.clone(), v.clone());
    }
    let entry = Value::Object(merged);
    match entry.get("type").and_then(Value::as_str)? {
        "api" | "key" | "api_key" if accept.key => Some(credential(text(entry.get("key"))?, source)),
        "oauth" if accept.oauth => {
            let mut found = credential(text(entry.get("access"))?, source);
            found.expires_at = entry.get("expires").and_then(epoch_from_json);
            found.account = text(entry.get("accountId"));
            found.org = text(entry.get("orgID"));
            found.server = text(entry.get("server"));
            Some(found)
        }
        _ => None,
    }
}

fn claude_code(env: &CredEnv) -> Option<Credential> {
    let value = read_json(&env.claude_dir()?.join(".credentials.json"))?;
    let oauth = value.get("claudeAiOauth")?;
    let mut found = credential(text(oauth.get("accessToken"))?, Source::ClaudeCode);
    found.expires_at = oauth.get("expiresAt").and_then(epoch_from_json);
    found.plan = oauth.get("subscriptionType").and_then(Value::as_str).and_then(claude::plan_label);
    Some(found)
}

fn codex_cli(env: &CredEnv) -> Option<Credential> {
    let value = read_json(&env.codex_home()?.join("auth.json"))?;
    if value.get("auth_mode").and_then(Value::as_str) == Some("apikey") {
        return None;
    }
    let tokens = value.get("tokens")?;
    let access = text(tokens.get("access_token"))?;
    let claims = crate::jwt::payload(&access);
    let auth_claim = claims.as_ref().and_then(|c| c.get("https://api.openai.com/auth"));
    let mut found = credential(access, Source::CodexCli);
    found.expires_at = claims.as_ref().and_then(|c| c.get("exp")).and_then(epoch_from_json);
    found.account =
        text(tokens.get("account_id")).or_else(|| text(auth_claim.and_then(|a| a.get("chatgpt_account_id"))));
    found.plan =
        auth_claim.and_then(|a| a.get("chatgpt_plan_type")).and_then(Value::as_str).and_then(chatgpt::plan_label);
    Some(found)
}

fn omp(env: &CredEnv, providers: &[&str], accept: Accept) -> Option<Credential> {
    const SQL: &str = "SELECT credential_type, data FROM auth_credentials \
                       WHERE disabled_cause IS NULL AND provider = ?1 ORDER BY updated_at DESC, id DESC";
    let sqlite = env.sqlite.as_deref()?;
    let path = env.omp_agent_db()?;
    for provider in providers {
        let rows = sqlite.query(&path, SQL, provider, 2).ok()?;
        for row in rows {
            let Ok(Value::Object(mut data)) = serde_json::from_str::<Value>(&row[1]) else { continue };
            data.insert("type".into(), Value::String(row[0].clone()));
            if let Some(found) = from_entry(&Value::Object(data), Source::Omp, accept) {
                return Some(found);
            }
        }
    }
    None
}

fn opencode2(env: &CredEnv, ids: &[&str], accept: Accept) -> Option<Credential> {
    const SQL: &str = "SELECT integration_id, value FROM credential \
                       WHERE integration_id = ?1 ORDER BY active DESC, time_updated DESC, id DESC";
    let sqlite = env.sqlite.as_deref()?;
    let path = env.opencode_db()?;
    for id in ids {
        for row in sqlite.query(&path, SQL, id, 2).ok()? {
            if let Some(found) =
                serde_json::from_str(&row[1]).ok().and_then(|v: Value| from_entry(&v, Source::OpenCode, accept))
            {
                return Some(found);
            }
        }
    }
    None
}

fn opencode1(env: &CredEnv, ids: &[&str], accept: Accept) -> Option<Credential> {
    let value = read_json(&env.data_home()?.join("opencode").join("auth.json"))?;
    ids.iter().find_map(|id| from_entry(value.get(*id)?, Source::OpenCode, accept))
}

/// Exact host of an `https://` URL, lowercased; anything else is `None`.
fn https_host(url: &str) -> Option<String> {
    let rest = url.trim().strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?.to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// A key Claude Code is configured to send to one of `hosts`, from the `env`
/// block of the user's (never a project's) settings.json.
fn claude_settings(env: &CredEnv, hosts: &[&str]) -> Option<Credential> {
    let value = read_json(&env.claude_dir()?.join("settings.json"))?;
    let vars = value.get("env")?;
    let host = https_host(vars.get("ANTHROPIC_BASE_URL")?.as_str()?)?;
    if !hosts.contains(&host.as_str()) {
        return None;
    }
    let key = text(vars.get("ANTHROPIC_AUTH_TOKEN")).or_else(|| text(vars.get("ANTHROPIC_API_KEY")))?;
    Some(credential(key, Source::ClaudeSettings))
}

#[derive(Clone, Copy)]
struct KeySpec {
    env: &'static [&'static str],
    hosts: &'static [&'static str],
    omp: &'static [&'static str],
    opencode: &'static [&'static str],
}

fn key_spec(id: ProviderId) -> KeySpec {
    match id {
        ProviderId::Zai => KeySpec {
            env: &["ZAI_API_KEY", "ZAI_CODING_PLAN_API_KEY"],
            hosts: &["api.z.ai"],
            omp: &["zai"],
            opencode: &["zai-coding-plan"],
        },
        ProviderId::Zhipu => KeySpec {
            env: &["ZHIPU_API_KEY", "ZHIPU_CODING_PLAN_API_KEY"],
            hosts: &["open.bigmodel.cn", "bigmodel.cn"],
            omp: &[],
            opencode: &["zhipu-coding-plan", "zhipuai-coding-plan"],
        },
        ProviderId::Kimi => KeySpec {
            env: &["KIMI_API_KEY", "KIMI_CODE_API_KEY", "KIMI_CN_API_KEY"],
            hosts: &["api.kimi.com"],
            omp: &["kimi-code"],
            opencode: &["kimi-code-plan-cn", "kimi-for-coding", "kimi-code", "kimi"],
        },
        ProviderId::KimiAi => KeySpec {
            env: &["KIMI_GLOBAL_API_KEY"],
            hosts: &["api.kimi.ai"],
            omp: &[],
            opencode: &["kimi-code-plan-global"],
        },
        ProviderId::MiniMax => KeySpec {
            env: &["MINIMAX_CODING_PLAN_API_KEY", "MINIMAX_API_KEY"],
            hosts: &["api.minimax.io"],
            omp: &["minimax-code"],
            opencode: &["minimax-coding-plan", "minimax"],
        },
        ProviderId::MiniMaxCn => KeySpec {
            env: &["MINIMAX_CHINA_CODING_PLAN_API_KEY"],
            hosts: &["api.minimaxi.com"],
            omp: &[],
            opencode: &["minimax-china-coding-plan", "minimax-cn-coding-plan", "minimax-cn", "minimax-china"],
        },
        ProviderId::OpencodeGo => KeySpec {
            env: &["OPENCODE_API_KEY"],
            hosts: &[],
            omp: &["opencode-go"],
            opencode: &["opencode-go", "opencode"],
        },
        ProviderId::Synthetic => KeySpec {
            env: &["SYNTHETIC_API_KEY"],
            hosts: &["api.synthetic.new"],
            omp: &["synthetic"],
            opencode: &["synthetic"],
        },
        ProviderId::OllamaCloud => {
            KeySpec { env: &["OLLAMA_API_KEY"], hosts: &[], omp: &["ollama-cloud"], opencode: &["ollama-cloud"] }
        }
        ProviderId::Chutes => KeySpec { env: &["CHUTES_API_KEY"], hosts: &[], omp: &[], opencode: &["chutes"] },
        ProviderId::CommandCode => {
            KeySpec { env: &["COMMAND_CODE_API_KEY"], hosts: &[], omp: &["commandcode"], opencode: &["commandcode"] }
        }
        ProviderId::Umans => KeySpec { env: &[], hosts: &["api.code.umans.ai"], omp: &["umans"], opencode: &["umans"] },
        ProviderId::DeepSeek => KeySpec {
            env: &["DEEPSEEK_API_KEY"],
            hosts: &["api.deepseek.com"],
            omp: &["deepseek"],
            opencode: &["deepseek"],
        },
        ProviderId::OpenRouter => KeySpec {
            env: &["OPENROUTER_API_KEY"],
            hosts: &["openrouter.ai"],
            omp: &["openrouter"],
            opencode: &["openrouter"],
        },
        ProviderId::Kilo => KeySpec { env: &["KILO_API_KEY"], hosts: &[], omp: &["kilo"], opencode: &["kilo"] },
        ProviderId::CharmHyper => {
            KeySpec { env: &["CHARM_HYPER_API_KEY"], hosts: &[], omp: &["charm-hyper"], opencode: &["charm-hyper"] }
        }
        ProviderId::Claude | ProviderId::ChatGpt | ProviderId::OpencodeZen => {
            KeySpec { env: &[], hosts: &[], omp: &[], opencode: &[] }
        }
    }
}

/// Candidate logins in priority order, produced lazily so a valid early one
/// stops the search before a later file or database is touched.
fn candidates(id: ProviderId, env: &CredEnv) -> Vec<Box<dyn Fn() -> Option<Credential> + '_>> {
    let oauth_only = Accept { key: false, oauth: true };
    match id {
        ProviderId::Claude => vec![
            Box::new(move || claude_code(env)),
            Box::new(move || omp(env, &["anthropic"], oauth_only)),
            Box::new(move || opencode2(env, &["anthropic"], oauth_only)),
            Box::new(move || opencode1(env, &["anthropic"], oauth_only)),
        ],
        // Zen's billing answers the OpenCode Console login only; OMP's
        // `opencode-zen` entry is a model-gateway key and cannot read it.
        ProviderId::OpencodeZen => vec![
            Box::new(move || opencode2(env, &["opencode"], oauth_only)),
            Box::new(move || opencode1(env, &["opencode"], oauth_only)),
        ],
        ProviderId::ChatGpt => vec![
            Box::new(move || codex_cli(env)),
            Box::new(move || omp(env, &["openai-codex"], oauth_only)),
            Box::new(move || opencode2(env, &["openai", "codex", "chatgpt"], oauth_only)),
            Box::new(move || opencode1(env, &["openai", "codex", "chatgpt"], oauth_only)),
        ],
        _ => {
            let spec = key_spec(id);
            // Kimi Code's OMP login is OAuth; OpenRouter and Kilo log in through
            // a browser flow whose result may be stored either way.
            let oauth = matches!(id, ProviderId::Kimi | ProviderId::OpenRouter | ProviderId::Kilo);
            let accept = Accept { key: true, oauth };
            vec![
                Box::new(move || env.own_keys.get(&id).cloned().map(|key| credential(key, Source::AnvilKey))),
                Box::new(move || {
                    spec.env
                        .iter()
                        .find_map(|name| env.var(name).map(|key| credential(key.to_owned(), Source::Env(name))))
                }),
                Box::new(move || claude_settings(env, spec.hosts)),
                Box::new(move || omp(env, spec.omp, accept)),
                Box::new(move || opencode2(env, spec.opencode, accept)),
                Box::new(move || opencode1(env, spec.opencode, accept)),
            ]
        }
    }
}

pub fn detect(id: ProviderId, env: &CredEnv) -> Detection {
    let mut expired = None;
    for candidate in candidates(id, env) {
        let Some(found) = candidate() else { continue };
        if found.expires_at.is_some_and(|at| at <= env.now + EXPIRY_MARGIN) {
            expired.get_or_insert(Detection::Expired { source: found.source, marker: found.marker });
            continue;
        }
        return Detection::Found(found);
    }
    expired.unwrap_or(Detection::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_791_210_000;

    fn env(home: &Path) -> CredEnv {
        CredEnv {
            home: Some(home.to_path_buf()),
            vars: HashMap::new(),
            own_keys: HashMap::new(),
            sqlite: None,
            now: NOW,
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn found(detection: Detection) -> Credential {
        match detection {
            Detection::Found(c) => c,
            other => panic!("expected a login, got {other:?}"),
        }
    }

    #[test]
    fn claude_code_login_with_plan_and_expiry() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".claude/.credentials.json");
        let expires_ms = (NOW + 3_600) * 1000;
        write(
            &path,
            &format!(
                r#"{{"claudeAiOauth": {{"accessToken": "sk-ant-oat", "refreshToken": "r",
            "expiresAt": {expires_ms}, "subscriptionType": "max"}}}}"#
            ),
        );
        let c = found(detect(ProviderId::Claude, &env(home.path())));
        assert_eq!(
            (c.secret.expose(), c.source.clone(), c.plan.as_deref(), c.expires_at),
            ("sk-ant-oat", Source::ClaudeCode, Some("Max"), Some(NOW + 3_600))
        );
        assert!(!format!("{c:?}").contains("sk-ant-oat"), "Debug leaks the token");

        write(&path, &format!(r#"{{"claudeAiOauth": {{"accessToken": "old", "expiresAt": {}}}}}"#, NOW * 1000));
        assert!(matches!(
            detect(ProviderId::Claude, &env(home.path())),
            Detection::Expired { source: Source::ClaudeCode, .. }
        ));
    }

    #[test]
    fn codex_login_reads_jwt_claims_and_skips_api_key_mode() {
        let home = tempfile::tempdir().unwrap();
        let token = crate::jwt::encode_for_test(&serde_json::json!({
            "exp": NOW + 600, "https://api.openai.com/auth": {"chatgpt_plan_type": "plus", "chatgpt_account_id": "acc-9"}}));
        let path = home.path().join(".codex/auth.json");
        write(&path, &format!(r#"{{"tokens": {{"access_token": "{token}", "refresh_token": "r"}}}}"#));
        let c = found(detect(ProviderId::ChatGpt, &env(home.path())));
        assert_eq!(
            (c.source.clone(), c.plan.as_deref(), c.account.as_deref(), c.expires_at),
            (Source::CodexCli, Some("Plus"), Some("acc-9"), Some(NOW + 600))
        );
        write(&path, r#"{"auth_mode": "apikey", "OPENAI_API_KEY": "sk-x"}"#);
        assert!(matches!(detect(ProviderId::ChatGpt, &env(home.path())), Detection::Missing));
    }

    #[test]
    fn key_sources_in_priority_order() {
        let home = tempfile::tempdir().unwrap();
        let mut e = env(home.path());
        write(
            &home.path().join(".local/share/opencode/auth.json"),
            r#"{"zai-coding-plan": {"type": "api", "key": "from-opencode"}}"#,
        );
        assert_eq!(found(detect(ProviderId::Zai, &e)).source, Source::OpenCode);
        write(
            &home.path().join(".claude/settings.json"),
            r#"{"env": {"ANTHROPIC_BASE_URL": "https://api.z.ai/api/anthropic", "ANTHROPIC_AUTH_TOKEN": "from-settings"}}"#,
        );
        assert_eq!(found(detect(ProviderId::Zai, &e)).secret.expose(), "from-settings");
        e.vars.insert("ZAI_API_KEY".into(), " from-env ".into());
        let c = found(detect(ProviderId::Zai, &e));
        assert_eq!((c.secret.expose(), c.source.clone()), ("from-env", Source::Env("ZAI_API_KEY")));
        e.own_keys.insert(ProviderId::Zai, "from-anvil".into());
        assert_eq!(found(detect(ProviderId::Zai, &e)).source, Source::AnvilKey);
        assert!(matches!(detect(ProviderId::Zhipu, &e), Detection::Missing), "Z.ai's key is not Zhipu's");
    }

    #[test]
    fn claude_settings_hosts_must_match_exactly() {
        let home = tempfile::tempdir().unwrap();
        let settings = home.path().join(".claude/settings.json");
        for (url, expected) in [
            ("https://api.z.ai/api/anthropic", true),
            ("https://API.Z.AI:443/api/anthropic", true),
            ("https://api.z.ai.evil.example/api", false),
            ("https://evil.example/api.z.ai", false),
            ("https://user@api.z.ai/", false),
            ("http://api.z.ai/api/anthropic", false),
        ] {
            write(&settings, &format!(r#"{{"env": {{"ANTHROPIC_BASE_URL": "{url}", "ANTHROPIC_AUTH_TOKEN": "k"}}}}"#));
            assert_eq!(matches!(detect(ProviderId::Zai, &env(home.path())), Detection::Found(_)), expected, "{url}");
        }
    }

    #[test]
    fn omp_and_opencode2_databases() {
        let Some(sqlite) = Sqlite::load() else { return };
        let home = tempfile::tempdir().unwrap();
        let omp_db = home.path().join(".omp/agent/agent.db");
        std::fs::create_dir_all(omp_db.parent().unwrap()).unwrap();
        sqlite.exec_for_test(&omp_db, &format!(
            "CREATE TABLE auth_credentials (id INTEGER PRIMARY KEY, provider TEXT, credential_type TEXT, data TEXT, disabled_cause TEXT, updated_at INTEGER);
             INSERT INTO auth_credentials VALUES (1, 'minimax-code', 'api_key', '{{\"key\":\"mm-key\"}}', NULL, 1);
             INSERT INTO auth_credentials VALUES (2, 'kimi-code', 'oauth', '{{\"access\":\"kimi-access\",\"refresh\":\"r\",\"expires\":{}}}', NULL, 1);
             INSERT INTO auth_credentials VALUES (3, 'anthropic', 'oauth', '{{\"access\":\"gone\",\"expires\":{}}}', 'logout', 1);",
            (NOW + 600) * 1000, (NOW + 600) * 1000));
        let oc_db = home.path().join(".local/share/opencode/opencode.db");
        std::fs::create_dir_all(oc_db.parent().unwrap()).unwrap();
        sqlite.exec_for_test(&oc_db,
            "CREATE TABLE credential (id TEXT, integration_id TEXT, label TEXT, active INTEGER, value TEXT, time_updated INTEGER);
             INSERT INTO credential VALUES ('a', 'zhipu-coding-plan', 'x', 0, '{\"type\":\"key\",\"key\":\"inactive\"}', 9);
             INSERT INTO credential VALUES ('b', 'zhipu-coding-plan', 'x', 1, '{\"type\":\"key\",\"key\":\"zp-key\"}', 1);");
        let mut e = env(home.path());
        e.sqlite = Some(Arc::new(sqlite));
        assert_eq!(found(detect(ProviderId::MiniMax, &e)).secret.expose(), "mm-key");
        let kimi = found(detect(ProviderId::Kimi, &e));
        assert_eq!(
            (kimi.secret.expose(), kimi.source.clone(), kimi.expires_at),
            ("kimi-access", Source::Omp, Some(NOW + 600))
        );
        assert!(matches!(detect(ProviderId::Claude, &e), Detection::Missing), "a disabled OMP row is not a login");
        assert_eq!(found(detect(ProviderId::Zhipu, &e)).secret.expose(), "zp-key", "active rows first");
    }

    #[test]
    fn zen_uses_the_console_login_and_go_a_key() {
        let home = tempfile::tempdir().unwrap();
        write(
            &home.path().join(".local/share/opencode/auth.json"),
            &format!(
                r#"{{"opencode": {{"type": "oauth", "access": "console-token", "refresh": "r", "expires": {},
                "orgID": "org-7", "server": "https://opencode.ai/console"}},
              "opencode-go": {{"type": "api", "key": "go-key"}}}}"#,
                (NOW + 600) * 1000
            ),
        );
        let zen = found(detect(ProviderId::OpencodeZen, &env(home.path())));
        assert_eq!(
            (zen.secret.expose(), zen.org.as_deref(), zen.server.as_deref()),
            ("console-token", Some("org-7"), Some("https://opencode.ai/console"))
        );
        let go = found(detect(ProviderId::OpencodeGo, &env(home.path())));
        assert_eq!(go.secret.expose(), "go-key");
        assert!(!ProviderId::OpencodeZen.accepts_own_key() && ProviderId::OpencodeGo.accepts_own_key());
    }

    #[test]
    fn new_key_providers_read_their_variables() {
        let home = tempfile::tempdir().unwrap();
        let mut e = env(home.path());
        for (id, var) in [
            (ProviderId::DeepSeek, "DEEPSEEK_API_KEY"),
            (ProviderId::OpenRouter, "OPENROUTER_API_KEY"),
            (ProviderId::Kilo, "KILO_API_KEY"),
            (ProviderId::Synthetic, "SYNTHETIC_API_KEY"),
            (ProviderId::OllamaCloud, "OLLAMA_API_KEY"),
            (ProviderId::Chutes, "CHUTES_API_KEY"),
            (ProviderId::CommandCode, "COMMAND_CODE_API_KEY"),
            (ProviderId::CharmHyper, "CHARM_HYPER_API_KEY"),
            (ProviderId::OpencodeGo, "OPENCODE_API_KEY"),
        ] {
            e.vars.insert(var.into(), format!("{var}-value"));
            let c = found(detect(id, &e));
            assert_eq!((c.secret.expose(), c.source), (format!("{var}-value").as_str(), Source::Env(var)), "{id:?}");
        }
        assert!(VARS.contains(&"COMMAND_CODE_API_KEY"), "every variable read is listed for from_process");
    }

    #[test]
    fn an_expired_first_source_does_not_hide_a_valid_later_one() {
        let home = tempfile::tempdir().unwrap();
        write(
            &home.path().join(".claude/.credentials.json"),
            &format!(r#"{{"claudeAiOauth": {{"accessToken": "old", "expiresAt": {}}}}}"#, NOW - 10),
        );
        write(
            &home.path().join(".local/share/opencode/auth.json"),
            &format!(r#"{{"anthropic": {{"type": "oauth", "access": "fresh", "expires": {}}}}}"#, (NOW + 999) * 1000),
        );
        let c = found(detect(ProviderId::Claude, &env(home.path())));
        assert_eq!((c.secret.expose(), c.source), ("fresh", Source::OpenCode));
    }

    #[test]
    fn omp_paths_follow_profiles_and_overrides() {
        let mut e = env(Path::new("C:/Users/u"));
        assert_eq!(e.omp_agent_db(), Some(PathBuf::from("C:/Users/u/.omp/agent/agent.db")));
        e.vars.insert("PI_PROFILE".into(), "work".into());
        assert_eq!(e.omp_agent_db(), Some(PathBuf::from("C:/Users/u/.omp/profiles/work/agent/agent.db")));
        e.vars.insert("OMP_PROFILE".into(), String::new());
        assert_eq!(
            e.omp_agent_db(),
            Some(PathBuf::from("C:/Users/u/.omp/agent/agent.db")),
            "empty OMP_PROFILE selects the default"
        );
        e.vars.insert("OMP_PROFILE".into(), "../evil".into());
        assert_eq!(
            e.omp_agent_db(),
            Some(PathBuf::from("C:/Users/u/.omp/agent/agent.db")),
            "invalid names are ignored"
        );
        e.vars.insert("PI_CODING_AGENT_DIR".into(), "D:/omp".into());
        assert_eq!(e.omp_agent_db(), Some(PathBuf::from("D:/omp/agent.db")));
        e.vars.insert("OPENCODE_DB".into(), ":memory:".into());
        assert_eq!(e.opencode_db(), None);
        e.vars.insert("OPENCODE_DB".into(), "alt.db".into());
        assert_eq!(e.opencode_db(), Some(PathBuf::from("C:/Users/u/.local/share/opencode/alt.db")));
    }
}
