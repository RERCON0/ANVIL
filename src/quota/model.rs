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
}

impl ProviderId {
    /// Display and settings order.
    pub const ALL: [ProviderId; 8] = [
        ProviderId::Claude,
        ProviderId::ChatGpt,
        ProviderId::Zai,
        ProviderId::Zhipu,
        ProviderId::Kimi,
        ProviderId::KimiAi,
        ProviderId::MiniMax,
        ProviderId::MiniMaxCn,
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
        }
    }

    /// Subscriptions are reached through a CLI's OAuth login only: an API key
    /// has no subscription quota, so ANVIL offers no key field for them.
    pub fn is_subscription(self) -> bool {
        matches!(self, ProviderId::Claude | ProviderId::ChatGpt)
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
    let minutes = (secs + 30) / 60;
    (minutes > 0).then(|| (format!("{minutes}m"), format!("{minutes}{}", strings::QUOTA_UNIT_MINUTE)))
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
