//! Chutes: a daily request quota that resets at midnight UTC.

use super::{json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{duration_window, FetchError, Fetched, Window};

pub const ENDPOINT: Endpoint = Endpoint::https("api.chutes.ai", "/users/me/quota_usage/me");

pub fn parse(body: &[u8], now: i64) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let quota = number(value.get("quota")).filter(|q| *q > 0.0).ok_or_else(|| FetchError::Format("no quota".into()))?;
    let used = number(value.get("used")).unwrap_or(0.0);
    let (key, label) = duration_window(86_400).expect("one day");
    let next_midnight = (now.div_euclid(86_400) + 1) * 86_400;
    Ok(Fetched::windows(None, vec![Window::new(key, label, used / quota * 100.0, Some(next_midnight))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_quota_resets_at_utc_midnight() {
        let now = 1_791_210_000; // 2026-10-05 14:20 UTC
        let fetched = parse(br#"{"used": 50, "quota": 200}"#, now).unwrap();
        assert_eq!(fetched.windows, vec![Window::new("1d", "1д", 25.0, Some(1_791_244_800))]);
        assert!(matches!(parse(br#"{"used": 5, "quota": 0}"#, now), Err(FetchError::Format(_))));
    }
}
