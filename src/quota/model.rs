//! What the quota worker knows about each provider. The same structures are
//! written to `quota.json` and drawn by the block, so they carry percentages,
//! instants and labels only: never a token, key, email or account id.

use serde::{Deserialize, Serialize};

use crate::strings;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProviderId {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "chatgpt")]
    ChatGpt,
    #[serde(rename = "zai")]
    Zai,
    #[serde(rename = "zhipu")]
    Zhipu,
    #[serde(rename = "kimi")]
    Kimi,
    #[serde(rename = "kimi-ai")]
    KimiAi,
    #[serde(rename = "minimax")]
    MiniMax,
    #[serde(rename = "minimax-cn")]
    MiniMaxCn,
    #[serde(rename = "opencode-go")]
    OpencodeGo,
    #[serde(rename = "synthetic")]
    Synthetic,
    #[serde(rename = "ollama-cloud")]
    OllamaCloud,
    #[serde(rename = "chutes")]
    Chutes,
    #[serde(rename = "commandcode")]
    CommandCode,
    #[serde(rename = "umans")]
    Umans,
    #[serde(rename = "deepseek")]
    DeepSeek,
    #[serde(rename = "openrouter")]
    OpenRouter,
    #[serde(rename = "kilo")]
    Kilo,
    #[serde(rename = "opencode-zen")]
    OpencodeZen,
    #[serde(rename = "charm-hyper")]
    CharmHyper,
}

impl ProviderId {
    /// Display and settings order: subscriptions, coding plans, balances.
    pub const ALL: [ProviderId; 19] = [
        ProviderId::Claude,
        ProviderId::ChatGpt,
        ProviderId::Zai,
        ProviderId::Zhipu,
        ProviderId::Kimi,
        ProviderId::KimiAi,
        ProviderId::MiniMax,
        ProviderId::MiniMaxCn,
        ProviderId::OpencodeGo,
        ProviderId::Synthetic,
        ProviderId::OllamaCloud,
        ProviderId::Chutes,
        ProviderId::CommandCode,
        ProviderId::Umans,
        ProviderId::DeepSeek,
        ProviderId::OpenRouter,
        ProviderId::Kilo,
        ProviderId::OpencodeZen,
        ProviderId::CharmHyper,
    ];

    /// Stable key used in config.json and the credential target name.
    pub fn key(self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::ChatGpt => "chatgpt",
            ProviderId::Zai => "zai",
            ProviderId::Zhipu => "zhipu",
            ProviderId::Kimi => "kimi",
            ProviderId::KimiAi => "kimi-ai",
            ProviderId::MiniMax => "minimax",
            ProviderId::MiniMaxCn => "minimax-cn",
            ProviderId::OpencodeGo => "opencode-go",
            ProviderId::Synthetic => "synthetic",
            ProviderId::OllamaCloud => "ollama-cloud",
            ProviderId::Chutes => "chutes",
            ProviderId::CommandCode => "commandcode",
            ProviderId::Umans => "umans",
            ProviderId::DeepSeek => "deepseek",
            ProviderId::OpenRouter => "openrouter",
            ProviderId::Kilo => "kilo",
            ProviderId::OpencodeZen => "opencode-zen",
            ProviderId::CharmHyper => "charm-hyper",
        }
    }

    /// Product names: not translated.
    pub fn label(self) -> &'static str {
        match self {
            ProviderId::Claude => "Claude",
            ProviderId::ChatGpt => "ChatGPT",
            ProviderId::Zai => "Z.ai",
            ProviderId::Zhipu => "Zhipu",
            ProviderId::Kimi => "Kimi",
            ProviderId::KimiAi => "Kimi (kimi.ai)",
            ProviderId::MiniMax => "MiniMax",
            ProviderId::MiniMaxCn => "MiniMax CN",
            ProviderId::OpencodeGo => "OpenCode Go",
            ProviderId::Synthetic => "Synthetic",
            ProviderId::OllamaCloud => "Ollama Cloud",
            ProviderId::Chutes => "Chutes",
            ProviderId::CommandCode => "Command Code",
            ProviderId::Umans => "Umans",
            ProviderId::DeepSeek => "DeepSeek",
            ProviderId::OpenRouter => "OpenRouter",
            ProviderId::Kilo => "Kilo",
            ProviderId::OpencodeZen => "OpenCode Zen",
            ProviderId::CharmHyper => "Charm Hyper",
        }
    }

    /// Subscriptions are reached through a CLI's OAuth login only: an API key
    /// has no subscription quota.
    pub fn is_subscription(self) -> bool {
        matches!(self, ProviderId::Claude | ProviderId::ChatGpt)
    }

    /// Whether the settings page offers a key field. Subscriptions and
    /// OpenCode Zen (its billing needs the OpenCode Console login) do not.
    pub fn accepts_own_key(self) -> bool {
        !self.is_subscription() && self != ProviderId::OpencodeZen
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    /// Stable key the visibility setting is stored under: `5h`, `7d`,
    /// `month`, `mcp`, `review`, `model:<name>`, or a span such as `3h`.
    pub key: String,
    pub label: String,
    /// Used, 0..=100 — never the remaining share.
    pub used_pct: f64,
    /// Unix seconds.
    pub resets_at: Option<i64>,
}

impl Window {
    pub fn new(key: impl Into<String>, label: impl Into<String>, used_pct: f64, resets_at: Option<i64>) -> Window {
        Window { key: key.into(), label: label.into(), used_pct: clamp_pct(used_pct), resets_at }
    }
}

pub fn clamp_pct(pct: f64) -> f64 {
    if pct.is_finite() {
        pct.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// Secondary windows stay hidden until the user ticks them.
pub fn window_visible_by_default(key: &str) -> bool {
    !matches!(key, "mcp" | "review")
}

/// Key and label of a window by its length, as OMP names them: whole days
/// `7d`/«7д», whole hours `5h`/«5ч», otherwise minutes `90m`/«90мин».
pub fn duration_window(secs: i64) -> Option<(String, String)> {
    if secs <= 0 {
        return None;
    }
    if secs % 86_400 == 0 {
        let days = secs / 86_400;
        return Some((format!("{days}d"), format!("{days}{}", strings::QUOTA_UNIT_DAY)));
    }
    if secs % 3_600 == 0 {
        let hours = secs / 3_600;
        return Some((format!("{hours}h"), format!("{hours}{}", strings::QUOTA_UNIT_HOUR)));
    }
    let minutes = secs.saturating_add(30) / 60;
    (minutes > 0).then(|| (format!("{minutes}m"), format!("{minutes}{}", strings::QUOTA_UNIT_MINUTE)))
}

/// What an amount is counted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Unit {
    Usd,
    Cny,
    Credits,
}

/// Whether an amount is what is left or what has been spent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BalanceKind {
    Remaining,
    Spent,
}

/// Money or credits: a prepaid balance, a credit allowance, or spend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    /// Stable key the visibility setting is stored under: `balance`,
    /// `balance:usd`, `credits`, `spend`, `pass`.
    pub key: String,
    /// Settings and hover label: «баланс», «кредиты», «траты», «Kilo Pass».
    pub label: String,
    pub amount: f64,
    pub unit: Unit,
    pub kind: BalanceKind,
    /// The allowance `amount` belongs to, when the provider reports one.
    pub limit: Option<f64>,
    pub resets_at: Option<i64>,
    /// Extra facts for the hover text, already worded.
    pub detail: Option<String>,
}

impl Balance {
    pub fn new(
        key: impl Into<String>,
        label: impl Into<String>,
        amount: f64,
        unit: Unit,
        kind: BalanceKind,
    ) -> Balance {
        let amount = if amount.is_finite() { amount } else { 0.0 };
        Balance { key: key.into(), label: label.into(), amount, unit, kind, limit: None, resets_at: None, detail: None }
    }

    pub fn with_limit(mut self, limit: Option<f64>) -> Balance {
        self.limit = limit.filter(|l| l.is_finite() && *l > 0.0);
        self
    }

    pub fn resetting(mut self, resets_at: Option<i64>) -> Balance {
        self.resets_at = resets_at;
        self
    }

    pub fn with_detail(mut self, detail: Option<String>) -> Balance {
        self.detail = detail.filter(|d| !d.is_empty());
        self
    }

    /// Share of the limit already spent; None without a limit.
    pub fn used_pct(&self) -> Option<f64> {
        let limit = self.limit?;
        if self.kind == BalanceKind::Remaining && self.amount > limit {
            return None; // An inconsistent allowance is not a genuine 0% spend.
        }
        let spent = match self.kind {
            BalanceKind::Spent => self.amount,
            BalanceKind::Remaining => limit - self.amount,
        };
        Some(clamp_pct(spent / limit * 100.0))
    }

    /// A prepaid balance at or below zero.
    pub fn exhausted(&self) -> bool {
        self.kind == BalanceKind::Remaining && self.amount <= 0.0
    }
}

/// `$12.40`, `-$3.00`, `¥12.40`, `120 кр.`, `4.5 кр.`.
pub fn format_amount(amount: f64, unit: Unit) -> String {
    let sign = if amount < 0.0 { "-" } else { "" };
    let abs = amount.abs();
    match unit {
        Unit::Usd => format!("{sign}${abs:.2}"),
        Unit::Cny => format!("{sign}¥{abs:.2}"),
        Unit::Credits if abs >= 10.0 => format!("{sign}{} {}", abs.round() as i64, strings::QUOTA_CREDITS_SHORT),
        Unit::Credits => format!("{sign}{abs:.1} {}", strings::QUOTA_CREDITS_SHORT),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProviderState {
    /// A login is known but nothing was fetched (provider switched off, or the
    /// first cycle has not run yet).
    Idle,
    Ok,
    /// Network or server trouble: the previous windows are kept and dimmed.
    UpdateFailed {
        reason: String,
    },
    /// The token expired or was refused; nothing is requested until it changes.
    AuthExpired,
    /// A local credential store could not be read; not a logout.
    StoreUnreadable,
    /// 429: paused until `retry_at` (unix seconds).
    RateLimited {
        retry_at: i64,
    },
    /// The answer could not be read.
    FormatError {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    pub id: ProviderId,
    /// "Max", "Pro", "Plus"…
    pub plan: Option<String>,
    /// Where the login came from, already worded for display.
    pub source: String,
    pub state: ProviderState,
    /// The last successfully fetched windows; kept through later failures.
    pub windows: Vec<Window>,
    /// The last successfully fetched balances (absent in older caches).
    #[serde(default)]
    pub balances: Vec<Balance>,
    pub fetched_at: Option<i64>,
    pub checked_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub providers: Vec<ProviderSnapshot>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot { version: Snapshot::VERSION, providers: Vec::new() }
    }
}

impl Snapshot {
    pub const VERSION: u32 = 1;

    pub fn get(&self, id: ProviderId) -> Option<&ProviderSnapshot> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Newest `checked_at`: how fresh the whole document is.
    pub fn checked_at(&self) -> Option<i64> {
        self.providers.iter().map(|p| p.checked_at).max()
    }
}

/// What a provider fetch returns.
#[derive(Clone, Debug, PartialEq)]
pub struct Fetched {
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    pub balances: Vec<Balance>,
}

impl Fetched {
    pub fn windows(plan: Option<String>, windows: Vec<Window>) -> Fetched {
        Fetched { plan, windows, balances: Vec::new() }
    }

    /// Provider text is untrusted on success too; no echoed key may reach disk.
    pub fn redacted(mut self, secret: &str) -> Self {
        if secret.is_empty() {
            return self;
        }
        let blank = |text: &mut String| {
            if text.contains(secret) {
                *text = text.replace(secret, "…");
            }
        };
        if let Some(plan) = &mut self.plan {
            blank(plan);
        }
        for window in &mut self.windows {
            blank(&mut window.key);
            blank(&mut window.label);
        }
        for balance in &mut self.balances {
            blank(&mut balance.key);
            blank(&mut balance.label);
            if let Some(detail) = &mut balance.detail {
                blank(detail);
            }
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FetchError {
    /// Transport failure or timeout.
    Network(String),
    /// A non-2xx answer; `retry_after` is a delay in seconds.
    Status { code: u16, retry_after: Option<i64> },
    /// HTTP 200 carrying an API-level refusal (Z.ai `success:false`, MiniMax `base_resp`).
    Rejected(String),
    /// The body is not what the provider documents.
    Format(String),
}

/// Longest pause after a 429.
pub const MAX_PAUSE: i64 = 900;

impl FetchError {
    /// Blanks out the secret inside a provider's own error text. That text is
    /// written to `quota.json` and drawn in the GUI, so an endpoint that echoes
    /// the `Authorization` header back would otherwise put the live key on disk.
    pub fn redacted(self, secret: &str) -> Self {
        let blank = |reason: String| {
            if secret.is_empty() || !reason.contains(secret) {
                return reason;
            }
            reason.replace(secret, "…")
        };
        match self {
            FetchError::Network(reason) => FetchError::Network(blank(reason)),
            FetchError::Rejected(reason) => FetchError::Rejected(blank(reason)),
            FetchError::Format(reason) => FetchError::Format(blank(reason)),
            other => other,
        }
    }

    /// `failures` counts the 429s in a row before this one.
    pub fn into_state(self, now: i64, failures: u32) -> ProviderState {
        match self {
            FetchError::Status { code: 401 | 403, .. } => ProviderState::AuthExpired,
            FetchError::Status { code: 429, retry_after } => {
                let backoff = 60_i64.saturating_mul(1 << failures.min(4));
                ProviderState::RateLimited { retry_at: now + retry_after.unwrap_or(backoff).clamp(1, MAX_PAUSE) }
            }
            FetchError::Status { code, .. } => ProviderState::UpdateFailed { reason: format!("HTTP {code}") },
            FetchError::Network(reason) | FetchError::Rejected(reason) => ProviderState::UpdateFailed { reason },
            FetchError::Format(reason) => ProviderState::FormatError { reason },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_allowance_smaller_than_the_balance_does_not_claim_zero_usage() {
        let balance = Balance::new("test", "test", 12.0, Unit::Usd, BalanceKind::Remaining).with_limit(Some(10.0));
        assert_eq!(balance.used_pct(), None);
        assert!(!balance.exhausted());
    }

    /// A provider that echoes the key back must not get it into `quota.json`
    /// or the GUI: 120 characters is enough to hold a key whole.
    #[test]
    fn error_text_cannot_carry_the_secret_to_disk() {
        let secret = "sk-live-abcdef0123456789";
        let echoed = FetchError::Rejected(format!("bad token {secret} for org 42"));
        let ProviderState::UpdateFailed { reason } = echoed.redacted(secret).into_state(0, 0) else {
            panic!("expected a failure state");
        };
        assert!(!reason.contains(secret), "the secret survived: {reason}");
        assert!(reason.contains('…'), "the message keeps its shape: {reason}");
        // A reason that never held the secret is untouched, and the status
        // variants carry no text to redact.
        assert_eq!(
            FetchError::Rejected("нет coding plan".into()).redacted(secret).into_state(0, 0),
            ProviderState::UpdateFailed { reason: "нет coding plan".into() }
        );
        assert_eq!(
            FetchError::Status { code: 503, retry_after: None }.redacted(secret).into_state(0, 0),
            ProviderState::UpdateFailed { reason: "HTTP 503".into() }
        );
    }

    #[test]
    fn provider_keys_round_trip_through_serde() {
        for id in ProviderId::ALL {
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, format!("\"{}\"", id.key()));
            assert_eq!(serde_json::from_str::<ProviderId>(&json).unwrap(), id);
        }
    }

    #[test]
    fn windows_by_length() {
        assert_eq!(duration_window(18_000), Some(("5h".into(), "5ч".into())));
        assert_eq!(duration_window(604_800), Some(("7d".into(), "7д".into())));
        assert_eq!(duration_window(5_400), Some(("90m".into(), "90мин".into())));
        assert_eq!(duration_window(0), None);
        assert!(
            window_visible_by_default("5h")
                && !window_visible_by_default("mcp")
                && !window_visible_by_default("review")
        );
    }

    #[test]
    fn balances_know_their_share_and_format_their_amounts() {
        let pass = Balance::new("pass", "Kilo Pass", 4.2, Unit::Usd, BalanceKind::Remaining).with_limit(Some(20.0));
        assert!((pass.used_pct().unwrap() - 79.0).abs() < 1e-9);
        let spend = Balance::new("spend", "траты", 3.1, Unit::Usd, BalanceKind::Spent).with_limit(Some(10.0));
        assert!((spend.used_pct().unwrap() - 31.0).abs() < 1e-9);
        let open = Balance::new("spend", "траты", 3.1, Unit::Usd, BalanceKind::Spent).with_limit(Some(0.0));
        assert_eq!(open.used_pct(), None, "a zero limit is no limit");
        assert!(Balance::new("balance", "баланс", 0.0, Unit::Cny, BalanceKind::Remaining).exhausted());
        assert!(!Balance::new("spend", "траты", 0.0, Unit::Usd, BalanceKind::Spent).exhausted());
        assert_eq!(format_amount(12.4, Unit::Usd), "$12.40");
        assert_eq!(format_amount(-3.0, Unit::Usd), "-$3.00");
        assert_eq!(format_amount(12.4, Unit::Cny), "¥12.40");
        assert_eq!(format_amount(120.4, Unit::Credits), "120 кр.");
        assert_eq!(format_amount(4.54, Unit::Credits), "4.5 кр.");
    }

    #[test]
    fn older_caches_without_balances_still_load() {
        let old = r#"{"id": "claude", "plan": null, "source": "Claude Code", "state": {"kind": "ok"},
                      "windows": [], "fetchedAt": 1, "checkedAt": 1}"#;
        let snapshot: ProviderSnapshot = serde_json::from_str(old).unwrap();
        assert!(snapshot.balances.is_empty());
    }

    #[test]
    fn percentages_are_clamped() {
        assert_eq!(Window::new("5h", "5ч", 140.0, None).used_pct, 100.0);
        assert_eq!(Window::new("5h", "5ч", -3.0, None).used_pct, 0.0);
        assert_eq!(Window::new("5h", "5ч", f64::NAN, None).used_pct, 0.0);
    }

    #[test]
    fn errors_become_states() {
        let now = 1_000;
        assert_eq!(FetchError::Status { code: 401, retry_after: None }.into_state(now, 0), ProviderState::AuthExpired);
        assert_eq!(FetchError::Status { code: 403, retry_after: None }.into_state(now, 0), ProviderState::AuthExpired);
        assert_eq!(
            FetchError::Status { code: 429, retry_after: Some(120) }.into_state(now, 0),
            ProviderState::RateLimited { retry_at: 1_120 }
        );
        assert_eq!(
            FetchError::Status { code: 429, retry_after: Some(99_999) }.into_state(now, 0),
            ProviderState::RateLimited { retry_at: now + MAX_PAUSE }
        );
        assert_eq!(
            FetchError::Status { code: 429, retry_after: None }.into_state(now, 0),
            ProviderState::RateLimited { retry_at: 1_060 }
        );
        assert_eq!(
            FetchError::Status { code: 429, retry_after: None }.into_state(now, 2),
            ProviderState::RateLimited { retry_at: 1_240 }
        );
        assert_eq!(
            FetchError::Status { code: 429, retry_after: None }.into_state(now, 9),
            ProviderState::RateLimited { retry_at: now + MAX_PAUSE }
        );
        assert_eq!(
            FetchError::Status { code: 503, retry_after: None }.into_state(now, 0),
            ProviderState::UpdateFailed { reason: "HTTP 503".into() }
        );
        assert_eq!(
            FetchError::Format("x".into()).into_state(now, 0),
            ProviderState::FormatError { reason: "x".into() }
        );
    }

    #[test]
    fn snapshot_round_trips_and_holds_no_secret_fields() {
        let snapshot = Snapshot {
            version: Snapshot::VERSION,
            providers: vec![ProviderSnapshot {
                id: ProviderId::Zai,
                plan: None,
                source: "ключ ANVIL".into(),
                state: ProviderState::RateLimited { retry_at: 5 },
                windows: vec![Window::new("5h", "5ч", 17.0, Some(1_791_210_000))],
                balances: vec![Balance::new("balance", "баланс", 12.4, Unit::Cny, BalanceKind::Remaining)],
                fetched_at: Some(1),
                checked_at: 2,
            }],
        };
        let text = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(serde_json::from_str::<Snapshot>(&text).unwrap(), snapshot);
        for forbidden in ["token", "secret", "email", "account", "apikey", "api_key"] {
            assert!(!text.to_lowercase().contains(forbidden), "{forbidden} in {text}");
        }
    }
}
