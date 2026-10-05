//! Synthetic: a rolling five-hour request limit and weekly credits.

use serde_json::Value;

use super::{json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{FetchError, Fetched, Window};
use crate::quota::time::epoch_from_json;
use crate::strings;

pub const ENDPOINT: Endpoint = Endpoint::https("api.synthetic.new", "/v2/quotas");

/// `"$12.34"` (or a plain number) as dollars.
fn dollars(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::String(s) => s.trim().strip_prefix('$').unwrap_or(s.trim()).parse().ok().filter(|n: &f64| n.is_finite()),
        other => number(Some(other)),
    }
}

pub fn parse(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let mut windows = Vec::new();
    if let Some(rolling) = value.get("rollingFiveHourLimit") {
        if let (Some(max), Some(remaining)) =
            (number(rolling.get("max")).filter(|m| *m > 0.0), number(rolling.get("remaining")))
        {
            let used = (max - remaining) / max * 100.0;
            windows.push(Window::new(
                "5h",
                format!("5{}", strings::QUOTA_UNIT_HOUR),
                used,
                rolling.get("nextTickAt").and_then(epoch_from_json),
            ));
        }
    }
    if let Some(weekly) = value.get("weeklyTokenLimit") {
        let reported = number(weekly.get("percentRemaining")).filter(|p| (0.0..=100.0).contains(p)).map(|p| 100.0 - p);
        let computed =
            match (dollars(weekly.get("maxCredits")).filter(|m| *m > 0.0), dollars(weekly.get("remainingCredits"))) {
                (Some(max), Some(remaining)) => Some((max - remaining) / max * 100.0),
                _ => None,
            };
        if let Some(used) = reported.or(computed) {
            windows.push(Window::new(
                "7d",
                format!("7{}", strings::QUOTA_UNIT_DAY),
                used,
                weekly.get("nextRegenAt").and_then(epoch_from_json),
            ));
        }
    }
    if windows.is_empty() {
        return Err(FetchError::Format("no quota windows".into()));
    }
    Ok(Fetched::windows(None, windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_and_weekly_windows() {
        let body = br#"{
            "rollingFiveHourLimit": {"max": 200, "remaining": 150, "nextTickAt": "2026-10-05T14:20:00Z"},
            "weeklyTokenLimit": {"maxCredits": "$20.00", "remainingCredits": "$5.00", "nextRegenAt": "2026-10-09T00:00:00Z"}}"#;
        let fetched = parse(body).unwrap();
        assert_eq!(
            fetched.windows,
            vec![
                Window::new("5h", "5ч", 25.0, Some(1_791_210_000)),
                Window::new("7d", "7д", 75.0, Some(1_791_504_000)),
            ]
        );
        let reported =
            parse(br#"{"weeklyTokenLimit": {"maxCredits": "$20", "remainingCredits": "$5", "percentRemaining": 40}}"#)
                .unwrap();
        assert_eq!(reported.windows[0].used_pct, 60.0, "the reported share wins");
        assert!(matches!(parse(b"{}"), Err(FetchError::Format(_))));
    }
}
