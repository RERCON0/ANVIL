//! DeepSeek: the account balance per currency, with the topped-up and
//! granted parts for the hover text.

use serde_json::Value;

use super::{json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{format_amount, Balance, BalanceKind, FetchError, Fetched, Unit};
use crate::strings;

pub const ENDPOINT: Endpoint = Endpoint::https("api.deepseek.com", "/user/balance");

pub fn parse(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let infos = value
        .get("balance_infos")
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Format("no balance_infos".into()))?;
    let mut balances = Vec::new();
    for info in infos {
        let (unit, key) =
            match info.get("currency").and_then(Value::as_str).map(|c| c.trim().to_ascii_uppercase()).as_deref() {
                Some("CNY") => (Unit::Cny, "balance:cny"),
                Some("USD") => (Unit::Usd, "balance:usd"),
                _ => continue,
            };
        let Some(total) = number(info.get("total_balance")) else { continue };
        let detail = match (number(info.get("topped_up_balance")), number(info.get("granted_balance"))) {
            (Some(topped), Some(granted)) => Some(format!(
                "{} {} · {} {}",
                strings::QUOTA_TOPPED_UP(),
                format_amount(topped, unit),
                strings::QUOTA_GRANTED(),
                format_amount(granted, unit)
            )),
            _ => None,
        };
        if !balances.iter().any(|b: &Balance| b.key == key) {
            balances.push(
                Balance::new(key, strings::QUOTA_BALANCE(), total, unit, BalanceKind::Remaining).with_detail(detail),
            );
        }
    }
    if balances.is_empty() {
        return Err(FetchError::Format("no CNY or USD balance".into()));
    }
    Ok(Fetched { plan: None, windows: Vec::new(), balances })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balances_per_currency_with_parts() {
        let body = br#"{"is_available": true, "balance_infos": [
            {"currency": "CNY", "total_balance": "110.00", "granted_balance": "10.00", "topped_up_balance": "100.00"},
            {"currency": "EUR", "total_balance": "5.00"},
            {"currency": "usd", "total_balance": "0.00"}]}"#;
        let fetched = parse(body).unwrap();
        assert_eq!(fetched.balances.len(), 2);
        let cny = &fetched.balances[0];
        assert_eq!((cny.key.as_str(), cny.amount, cny.unit), ("balance:cny", 110.0, Unit::Cny));
        assert_eq!(cny.detail.as_deref(), Some("пополнено ¥100.00 · подарено ¥10.00"));
        assert!(fetched.balances[1].exhausted(), "a zero balance is an alarm");
        assert!(matches!(parse(br#"{"balance_infos": []}"#), Err(FetchError::Format(_))));
    }
}
