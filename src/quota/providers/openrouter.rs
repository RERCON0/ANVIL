//! OpenRouter: what the key has spent, against its limit when it has one.

use super::{json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{Balance, BalanceKind, FetchError, Fetched, Unit};
use crate::strings;

pub const ENDPOINT: Endpoint = Endpoint::https("openrouter.ai", "/api/v1/key");

pub fn parse(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let data = value.get("data").filter(|d| d.is_object()).ok_or_else(|| FetchError::Format("no data".into()))?;
    let usage = number(data.get("usage")).filter(|u| *u >= 0.0).ok_or_else(|| FetchError::Format("no usage".into()))?;
    let limit = number(data.get("limit")).filter(|l| *l > 0.0);
    let spent = match (limit, number(data.get("limit_remaining"))) {
        (Some(limit), Some(remaining)) if remaining <= limit => limit - remaining,
        _ => usage,
    };
    let spend = Balance::new("spend", strings::QUOTA_SPEND, spent, Unit::Usd, BalanceKind::Spent).with_limit(limit);
    Ok(Fetched { plan: None, windows: Vec::new(), balances: vec![spend] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anomalous_remaining_balance_does_not_become_negative_spend() {
        let fetched = parse(br#"{"data":{"usage":3,"limit":10,"limit_remaining":12}}"#).unwrap();
        assert_eq!(fetched.balances[0].amount, 3.0);
        assert_eq!(fetched.balances[0].used_pct(), Some(30.0));
    }

    #[test]
    fn spend_with_and_without_a_limit() {
        let limited = parse(br#"{"data": {"usage": 3.5, "limit": 10, "limit_remaining": 6.9}}"#).unwrap();
        let spend = &limited.balances[0];
        assert!((spend.amount - 3.1).abs() < 1e-9 && spend.limit == Some(10.0));
        assert!((spend.used_pct().unwrap() - 31.0).abs() < 1e-9);
        let open = parse(br#"{"data": {"usage": 12.25, "limit": null, "limit_remaining": null}}"#).unwrap();
        assert_eq!((open.balances[0].amount, open.balances[0].limit), (12.25, None));
        assert!(matches!(parse(br#"{"data": {"usage": -1}}"#), Err(FetchError::Format(_))));
    }
}
