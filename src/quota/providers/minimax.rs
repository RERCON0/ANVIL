//! MiniMax Token Plan. It answers HTTP 200 even for a refused key, so
//! `base_resp.status_code` is the real success signal.

use serde_json::Value;

use super::{clean, json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{duration_window, FetchError, Fetched, Window};
use crate::quota::time::epoch_from_json;
use crate::strings;

pub const INTERNATIONAL: Endpoint = Endpoint::https("api.minimax.io", "/v1/token_plan/remains");
pub const CHINA: Endpoint = Endpoint::https("api.minimaxi.com", "/v1/token_plan/remains");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    International,
    China,
}

/// `current_*_status`: 2 = exhausted, 3 = unlimited.
const EXHAUSTED: i64 = 2;
const UNLIMITED: i64 = 3;

fn int(bucket: &Value, key: &str) -> Option<i64> {
    number(bucket.get(key)).map(|n| n as i64)
}

/// A bucket outside the plan reports zero totals and "unlimited" for both
/// windows, which would otherwise read as an untouched quota.
fn not_in_plan(bucket: &Value) -> bool {
    int(bucket, "current_interval_total_count") == Some(0)
        && int(bucket, "current_weekly_total_count") == Some(0)
        && int(bucket, "current_interval_status") == Some(UNLIMITED)
        && int(bucket, "current_weekly_status") == Some(UNLIMITED)
}

fn used(bucket: &Value, prefix: &str, region: Region) -> Option<f64> {
    if int(bucket, &format!("{prefix}_status")) == Some(EXHAUSTED) {
        return Some(100.0);
    }
    if let Some(remaining) = number(bucket.get(format!("{prefix}_remaining_percent"))) {
        return Some(100.0 - remaining);
    }
    let total = number(bucket.get(format!("{prefix}_total_count"))).filter(|t| *t > 0.0)?;
    let count = number(bucket.get(format!("{prefix}_usage_count")))?;
    // The international API reports what is left in `usage_count`; China, what was used.
    let used = match region {
        Region::International => total - count.clamp(0.0, total),
        Region::China => count.max(0.0),
    };
    Some(used / total * 100.0)
}

fn reset(bucket: &Value, end_key: &str, remains_key: &str, now: i64) -> Option<i64> {
    bucket
        .get(end_key)
        .and_then(epoch_from_json)
        .or_else(|| number(bucket.get(remains_key)).filter(|ms| *ms > 0.0).map(|ms| now + (ms / 1000.0) as i64))
}

pub fn parse(body: &[u8], now: i64, region: Region) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let status = value.pointer("/base_resp/status_code").and_then(Value::as_i64);
    if status != Some(0) {
        let message =
            value.pointer("/base_resp/status_msg").and_then(Value::as_str).map(|m| clean(m, 120)).unwrap_or_default();
        let message = if message.is_empty() {
            format!("status {}", status.map_or("?".to_owned(), |s| s.to_string()))
        } else {
            message
        };
        return Err(FetchError::Rejected(message));
    }
    let buckets: Vec<&Value> = value
        .get("model_remains")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Format("no model_remains".into()))?
        .iter()
        .filter(|b| !not_in_plan(b))
        .collect();
    let name = |bucket: &Value| bucket.get("model_name").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
    let bucket = buckets
        .iter()
        .find(|b| name(b) == "general")
        .or_else(|| buckets.iter().find(|b| name(b).starts_with("minimax-m")))
        .ok_or_else(|| FetchError::Format("no text-model quota".into()))?;
    let mut windows = Vec::new();
    if let Some(used) = used(bucket, "current_interval", region) {
        let span = match (
            bucket.get("start_time").and_then(epoch_from_json),
            bucket.get("end_time").and_then(epoch_from_json),
        ) {
            (Some(start), Some(end)) if end > start => duration_window(end - start),
            _ => None,
        };
        let (key, label) = span.unwrap_or_else(|| ("5h".to_owned(), format!("5{}", strings::QUOTA_UNIT_HOUR)));
        windows.push(Window::new(key, label, used, reset(bucket, "end_time", "remains_time", now)));
    }
    if let Some(used) = used(bucket, "current_weekly", region) {
        if !windows.iter().any(|w: &Window| w.key == "7d") {
            let resets_at = reset(bucket, "weekly_end_time", "weekly_remains_time", now);
            windows.push(Window::new("7d", format!("7{}", strings::QUOTA_UNIT_DAY), used, resets_at));
        }
    }
    if windows.is_empty() {
        return Err(FetchError::Format("no usable windows".into()));
    }
    Ok(Fetched { plan: None, windows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_bucket_by_percent_and_status() {
        let body = br#"{"base_resp": {"status_code": 0, "status_msg": "success"}, "model_remains": [
            {"model_name": "video", "current_interval_remaining_percent": 1},
            {"model_name": "general", "start_time": 1791192000000, "end_time": 1791210000000,
             "current_interval_remaining_percent": 80, "current_interval_status": 1,
             "weekly_end_time": 1791504000, "current_weekly_remaining_percent": 95, "current_weekly_status": 2}]}"#;
        let fetched = parse(body, 0, Region::International).unwrap();
        assert_eq!(
            fetched.windows,
            vec![
                Window::new("5h", "5ч", 20.0, Some(1_791_210_000)),
                Window::new("7d", "7д", 100.0, Some(1_791_504_000)),
            ]
        );
    }

    #[test]
    fn counts_mean_remaining_abroad_and_used_in_china() {
        let body = br#"{"base_resp": {"status_code": 0}, "model_remains": [
            {"model_name": "MiniMax-M2", "current_interval_total_count": 100, "current_interval_usage_count": 30,
             "remains_time": 60000}]}"#;
        let abroad = parse(body, 1_000, Region::International).unwrap();
        assert_eq!(abroad.windows, vec![Window::new("5h", "5ч", 70.0, Some(1_060))]);
        let china = parse(body, 1_000, Region::China).unwrap();
        assert_eq!(china.windows, vec![Window::new("5h", "5ч", 30.0, Some(1_060))]);
    }

    #[test]
    fn buckets_outside_the_plan_are_skipped() {
        let body = br#"{"base_resp": {"status_code": 0}, "model_remains": [
            {"model_name": "general", "current_interval_total_count": 0, "current_weekly_total_count": 0,
             "current_interval_status": 3, "current_weekly_status": 3, "current_interval_remaining_percent": 100}]}"#;
        assert!(matches!(parse(body, 0, Region::International), Err(FetchError::Format(_))));
    }

    #[test]
    fn refusals_carry_the_message() {
        let body = br#"{"base_resp": {"status_code": 1004, "status_msg": "login fail"}}"#;
        assert_eq!(parse(body, 0, Region::China), Err(FetchError::Rejected("login fail".into())));
        assert_eq!(parse(b"{}", 0, Region::China), Err(FetchError::Rejected("status ?".into())));
    }
}
