//! Claude subscriptions (Pro/Max): Anthropic's OAuth usage endpoint, the one
//! Claude Code's `/usage` reads. Undocumented — a changed shape is a
//! `FormatError` for this provider only.

use serde_json::Value;

use super::{clean, json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{FetchError, Fetched, Window};
use crate::quota::time::epoch_from_json;
use crate::strings;

pub const ENDPOINT: Endpoint = Endpoint::https("api.anthropic.com", "/api/oauth/usage");
pub const BETA: &str = "oauth-2025-04-20";

fn window(value: Option<&Value>) -> Option<(f64, Option<i64>)> {
    let value = value?;
    let used = number(value.get("utilization"))?;
    Some((used, value.get("resets_at").and_then(epoch_from_json)))
}

pub fn parse(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let roots = [Some(&value), value.get("usage"), value.get("rate_limits")];
    for root in roots.into_iter().flatten() {
        let (Some(five), Some(seven)) = (window(root.get("five_hour")), window(root.get("seven_day"))) else {
            continue;
        };
        let mut windows = vec![
            Window::new("5h", format!("5{}", strings::QUOTA_UNIT_HOUR()), five.0, five.1),
            Window::new("7d", format!("7{}", strings::QUOTA_UNIT_DAY()), seven.0, seven.1),
        ];
        let limits = root.get("limits").or_else(|| value.get("limits")).and_then(Value::as_array);
        for limit in limits.into_iter().flatten() {
            if limit.get("kind").and_then(Value::as_str) != Some("weekly_scoped") {
                continue;
            }
            let Some(name) = limit.pointer("/scope/model/display_name").and_then(Value::as_str).map(|n| clean(n, 24))
            else {
                continue;
            };
            let Some(used) = number(limit.get("percent")) else { continue };
            let key = format!("model:{}", name.to_lowercase());
            if name.is_empty() || windows.iter().any(|w| w.key == key) {
                continue;
            }
            let label = format!("{name}·7{}", strings::QUOTA_UNIT_DAY());
            windows.push(Window::new(key, label, used, limit.get("resets_at").and_then(epoch_from_json)));
        }
        return Ok(Fetched::windows(None, windows));
    }
    Err(FetchError::Format("no five_hour/seven_day windows".into()))
}

/// `subscriptionType` from Claude Code's credentials: `max` → «Max».
pub fn plan_label(raw: &str) -> Option<String> {
    let raw = clean(raw, 20);
    let mut chars = raw.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_hour_seven_day_and_model_limits() {
        let body = br#"{
            "five_hour": {"utilization": 17.2, "resets_at": "2026-10-05T14:20:00Z"},
            "seven_day": {"utilization": "63.8", "resets_at": 1791210000},
            "extra_usage": {"is_enabled": false},
            "limits": [
                {"kind": "weekly_scoped", "percent": 41, "resets_at": "2026-10-09T00:00:00Z",
                 "scope": {"model": {"display_name": "Opus"}}},
                {"kind": "weekly_scoped", "percent": 5, "scope": {"model": {"display_name": "Opus"}}},
                {"kind": "daily", "percent": 99, "scope": {"model": {"display_name": "Haiku"}}},
                {"kind": "weekly_scoped", "scope": {"model": {"display_name": "Fable"}}}
            ]}"#;
        let fetched = parse(body).unwrap();
        let keys: Vec<(&str, &str, f64, Option<i64>)> =
            fetched.windows.iter().map(|w| (w.key.as_str(), w.label.as_str(), w.used_pct, w.resets_at)).collect();
        assert_eq!(
            keys,
            vec![
                ("5h", "5ч", 17.2, Some(1_791_210_000)),
                ("7d", "7д", 63.8, Some(1_791_210_000)),
                ("model:opus", "Opus·7д", 41.0, Some(1_791_504_000)),
            ]
        );
    }

    #[test]
    fn nested_roots_are_searched() {
        let body = br#"{"rate_limits": {"five_hour": {"utilization": 1}, "seven_day": {"utilization": 2}}}"#;
        assert_eq!(parse(body).unwrap().windows.len(), 2);
    }

    #[test]
    fn missing_windows_are_a_format_error() {
        assert!(matches!(parse(br#"{"five_hour": {"utilization": 1}}"#), Err(FetchError::Format(_))));
        assert!(matches!(parse(br#"{"five_hour": {}, "seven_day": {}}"#), Err(FetchError::Format(_))));
        assert!(matches!(parse(b"[]"), Err(FetchError::Format(_))));
        assert!(matches!(parse(b""), Err(FetchError::Format(_))));
    }

    #[test]
    fn plans() {
        assert_eq!(plan_label("max").as_deref(), Some("Max"));
        assert_eq!(plan_label(" pro ").as_deref(), Some("Pro"));
        assert_eq!(plan_label(""), None);
    }
}
