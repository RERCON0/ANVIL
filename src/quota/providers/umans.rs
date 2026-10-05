//! Umans AI Coding Plan: weighted requests in a rolling window against the
//! soft cap.

use serde_json::Value;

use super::{json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{duration_window, FetchError, Fetched, Window};
use crate::quota::time::epoch_from_json;

pub const ENDPOINT: Endpoint = Endpoint::https("api.code.umans.ai", "/v1/usage");

pub fn parse(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let requests = value.pointer("/limits/requests").unwrap_or(&Value::Null);
    let limit = number(requests.get("limit"))
        .filter(|l| *l > 0.0)
        .ok_or_else(|| FetchError::Format("no request limit".into()))?;
    let usage = value.get("usage").unwrap_or(&Value::Null);
    let used = number(usage.get("weighted_in_window"))
        .or_else(|| number(usage.get("requests_in_window")))
        .ok_or_else(|| FetchError::Format("no usage".into()))?;
    let span = number(requests.get("window_seconds")).filter(|s| *s > 0.0).map(|s| s as i64).unwrap_or(18_000);
    let (key, label) = duration_window(span).ok_or_else(|| FetchError::Format("bad window".into()))?;
    let resets_at = value.pointer("/window/resets_at").and_then(epoch_from_json);
    Ok(Fetched::windows(None, vec![Window::new(key, label, used / limit * 100.0, resets_at)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_requests_against_the_soft_cap() {
        let body = br#"{"limits": {"requests": {"limit": 400, "hard_cap": 600, "window_seconds": 18000}},
                        "usage": {"requests_in_window": 120, "weighted_in_window": 100},
                        "window": {"resets_at": "2026-10-05T14:20:00Z"}}"#;
        assert_eq!(parse(body).unwrap().windows, vec![Window::new("5h", "5ч", 25.0, Some(1_791_210_000))]);
        let raw = parse(br#"{"limits": {"requests": {"limit": 10}}, "usage": {"requests_in_window": 5}}"#).unwrap();
        assert_eq!(raw.windows[0].used_pct, 50.0, "unweighted count and the default 5-hour window");
        assert!(matches!(parse(br#"{"usage": {"requests_in_window": 5}}"#), Err(FetchError::Format(_))));
    }
}
