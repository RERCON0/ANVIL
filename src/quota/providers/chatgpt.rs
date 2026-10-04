//! ChatGPT subscriptions through Codex: the usage endpoint the Codex client
//! itself reads. Undocumented — a changed shape is a `FormatError` here only.

use serde_json::Value;

use super::{clean, json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{duration_window, FetchError, Fetched, Window};
use crate::quota::time::epoch_from_json;
use crate::strings;

pub const ENDPOINT: Endpoint = Endpoint::https("chatgpt.com", "/backend-api/wham/usage");

fn reset(window: &Value, now: i64) -> Option<i64> {
    window
        .get("reset_at")
        .and_then(epoch_from_json)
        .or_else(|| number(window.get("reset_after_seconds")).filter(|s| *s > 0.0).map(|s| now + s as i64))
}

pub fn parse(body: &[u8], now: i64) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let mut windows: Vec<Window> = Vec::new();
    for name in ["primary_window", "secondary_window"] {
        let Some(window) = value.get("rate_limit").and_then(|r| r.get(name)) else { continue };
        let Some(used) = number(window.get("used_percent")) else { continue };
        let (key, label) = match window.get("limit_window_seconds").and_then(Value::as_i64) {
            Some(secs @ (18_000 | 604_800)) => duration_window(secs).expect("whole hours and days"),
            Some(2_628_000) => ("month".to_owned(), strings::QUOTA_MONTH.to_owned()),
            _ => continue,
        };
        if !windows.iter().any(|w| w.key == key) {
            windows.push(Window::new(key, label, used, reset(window, now)));
        }
    }
    if let Some(window) = value.pointer("/code_review_rate_limit/primary_window") {
        if let Some(used) = number(window.get("used_percent")) {
            windows.push(Window::new("review", strings::QUOTA_REVIEW, used, reset(window, now)));
        }
    }
    if windows.is_empty() {
        return Err(FetchError::Format("no rate_limit windows".into()));
    }
    Ok(Fetched { plan: value.get("plan_type").and_then(Value::as_str).and_then(plan_label), windows })
}

/// `plan_type` (or the `chatgpt_plan_type` claim) for display.
pub fn plan_label(raw: &str) -> Option<String> {
    let lower = raw.trim().to_lowercase();
    Some(match lower.as_str() {
        "" => return None,
        "team" | "business" => "Business".to_owned(),
        "enterprise" | "edu" => clean(raw, 20),
        _ if lower.contains("pro") => "Pro".to_owned(),
        _ if lower.contains("plus") => "Plus".to_owned(),
        "free" => "Free".to_owned(),
        _ => clean(raw, 20),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_secondary_and_review_windows() {
        let now = 1_791_210_000;
        let body = br#"{"plan_type": "pro",
            "rate_limit": {
                "primary_window": {"used_percent": 42, "limit_window_seconds": 18000, "reset_after_seconds": 3900},
                "secondary_window": {"used_percent": 12.5, "limit_window_seconds": 604800, "reset_at": 1791500000}},
            "code_review_rate_limit": {"primary_window": {"used_percent": 3, "limit_window_seconds": 604800}}}"#;
        let fetched = parse(body, now).unwrap();
        assert_eq!(fetched.plan.as_deref(), Some("Pro"));
        assert_eq!(
            fetched.windows,
            vec![
                Window::new("5h", "5ч", 42.0, Some(now + 3_900)),
                Window::new("7d", "7д", 12.5, Some(1_791_500_000)),
                Window::new("review", "ревью", 3.0, None),
            ]
        );
    }

    #[test]
    fn monthly_and_unknown_lengths() {
        let body = br#"{"rate_limit": {
            "primary_window": {"used_percent": 7, "limit_window_seconds": 2628000},
            "secondary_window": {"used_percent": 9, "limit_window_seconds": 777}}}"#;
        let fetched = parse(body, 0).unwrap();
        assert_eq!(fetched.windows, vec![Window::new("month", "мес", 7.0, None)]);
        assert!(matches!(parse(br#"{"rate_limit": {}}"#, 0), Err(FetchError::Format(_))));
    }

    #[test]
    fn plan_names() {
        assert_eq!(plan_label("plus").as_deref(), Some("Plus"));
        assert_eq!(plan_label("prolite").as_deref(), Some("Pro"));
        assert_eq!(plan_label("team").as_deref(), Some("Business"));
        assert_eq!(plan_label("  "), None);
    }
}
