//! Kimi Code: the `usages` endpoint of the coding API, on two domains.

use serde_json::Value;

use super::{after, json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{duration_window, FetchError, Fetched, Window};
use crate::quota::time::epoch_from_json;
use crate::strings;

pub const KIMI: Endpoint = Endpoint::https("api.kimi.com", "/coding/v1/usages");
pub const KIMI_AI: Endpoint = Endpoint::https("api.kimi.ai", "/coding/v1/usages");

/// Used share of a `{limit, used | remaining}` record.
fn used_pct(record: &Value) -> Option<f64> {
    let limit = number(record.get("limit")).filter(|l| *l > 0.0)?;
    let used = number(record.get("used")).or_else(|| number(record.get("remaining")).map(|r| limit - r))?;
    Some(used / limit * 100.0)
}

fn reset(record: &Value, now: i64) -> Option<i64> {
    for key in ["reset_at", "resetAt", "reset_time", "resetTime"] {
        if let Some(at) = record.get(key).and_then(epoch_from_json) {
            return Some(at);
        }
    }
    ["reset_in", "resetIn", "ttl"]
        .iter()
        .find_map(|key| number(record.get(*key)).filter(|s| *s > 0.0))
        .map(|secs| after(now, secs))
}

fn window_secs(window: &Value) -> Option<i64> {
    let duration = number(window.get("duration")).filter(|d| *d > 0.0)?;
    let unit = window.get("timeUnit").and_then(Value::as_str)?.to_uppercase();
    let scale = [("MINUTE", 60.0), ("HOUR", 3_600.0), ("DAY", 86_400.0), ("WEEK", 604_800.0), ("SECOND", 1.0)]
        .iter()
        .find(|(name, _)| unit.contains(name))?
        .1;
    Some((duration * scale) as i64)
}

pub fn parse(body: &[u8], now: i64) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let data = value.get("data").filter(|d| d.is_object()).unwrap_or(&value);
    let mut windows: Vec<Window> = Vec::new();
    if let Some(usage) = data.get("usage").filter(|u| u.is_object()) {
        if let Some(used) = used_pct(usage) {
            windows.push(Window::new("7d", format!("7{}", strings::QUOTA_UNIT_DAY), used, reset(usage, now)));
        }
    }
    for item in data.get("limits").and_then(Value::as_array).into_iter().flatten() {
        let detail = item.get("detail").filter(|d| d.is_object()).unwrap_or(item);
        let window = item.get("window").filter(|w| w.is_object());
        let Some((key, label)) = window.and_then(window_secs).and_then(duration_window) else { continue };
        let Some(used) = used_pct(detail) else { continue };
        if !windows.iter().any(|w| w.key == key) {
            let resets_at = reset(detail, now).or_else(|| window.and_then(|w| reset(w, now)));
            windows.push(Window::new(key, label, used, resets_at));
        }
    }
    if windows.is_empty() {
        return Err(FetchError::Format("no usage windows".into()));
    }
    Ok(Fetched::windows(None, windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_usage_and_window_limits() {
        let now = 1_791_210_000;
        let body = br#"{"data": {
            "usage": {"limit": 1000, "remaining": 750, "resetAt": "2026-10-09T00:00:00Z"},
            "limits": [
                {"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
                 "detail": {"limit": "200", "used": "50", "reset_in": 3600}},
                {"window": {"duration": 7, "timeUnit": "DAY"}, "detail": {"limit": 1, "used": 1}},
                {"window": {"duration": 1, "timeUnit": "HOUR"}, "detail": {"limit": 0, "used": 0}},
                {"detail": {"limit": 10, "used": 1}}
            ]}}"#;
        let fetched = parse(body, now).unwrap();
        assert_eq!(
            fetched.windows,
            vec![Window::new("7d", "7д", 25.0, Some(1_791_504_000)), Window::new("5h", "5ч", 25.0, Some(now + 3_600)),]
        );
    }

    #[test]
    fn nothing_usable_is_a_format_error() {
        assert!(matches!(parse(br#"{"data": {"usage": {"limit": 0}}}"#, 0), Err(FetchError::Format(_))));
        assert!(matches!(parse(br#"{"error": "unauthorized"}"#, 0), Err(FetchError::Format(_))));
    }
}
