//! Kilo Gateway: the Kilo Pass credits of the current period when there is a
//! subscription, else the prepaid balance.

use serde_json::Value;

use super::{get, json, number};
use crate::quota::http::{Endpoint, Http};
use crate::quota::model::{Balance, BalanceKind, FetchError, Fetched, Unit};
use crate::quota::time::epoch_from_json;
use crate::strings;

pub const STATE: Endpoint = Endpoint::https("app.kilo.ai", "/api/trpc/kiloPass.getState");
/// `batch=1&input={"0":null}`, percent-encoded.
pub const STATE_QUERY: &str = "batch=1&input=%7B%220%22%3Anull%7D";
pub const BALANCE: Endpoint = Endpoint::https("api.kilo.ai", "/api/profile/balance");

pub fn fetch(http: &dyn Http, headers: &[(&'static str, String)], now: i64) -> Result<Fetched, FetchError> {
    let state = get(http, &STATE, Some(STATE_QUERY), headers, now)?;
    match parse_pass(&state)? {
        Some(pass) => {
            Ok(Fetched { plan: Some(strings::QUOTA_KILO_PASS.to_owned()), windows: Vec::new(), balances: vec![pass] })
        }
        None => parse_balance(&get(http, &BALANCE, None, headers, now)?),
    }
}

/// The Kilo Pass credits left this period; `None` without a subscription.
pub fn parse_pass(body: &[u8]) -> Result<Option<Balance>, FetchError> {
    let value = json(body)?;
    let item = value.as_array().and_then(|items| items.first()).unwrap_or(&value);
    let data = item
        .pointer("/result/data")
        .filter(|d| d.is_object())
        .ok_or_else(|| FetchError::Format("no result data".into()))?;
    let root = data.get("json").filter(|j| j.is_object()).unwrap_or(data);
    let subscription = match root.get("subscription") {
        None | Some(Value::Null) => return Ok(None),
        Some(subscription) if subscription.is_object() => subscription,
        Some(_) => return Err(FetchError::Format("bad subscription".into())),
    };
    let base = number(subscription.get("currentPeriodBaseCreditsUsd")).filter(|n| *n >= 0.0);
    let usage = number(subscription.get("currentPeriodUsageUsd")).filter(|n| *n >= 0.0);
    let (Some(base), Some(usage)) = (base, usage) else {
        return Err(FetchError::Format("bad Kilo Pass credits".into()));
    };
    let bonus = number(subscription.get("currentPeriodBonusCreditsUsd")).filter(|n| *n >= 0.0).unwrap_or(0.0);
    let total = base + bonus;
    let resets_at =
        ["nextBillingAt", "nextRenewalAt"].iter().find_map(|k| subscription.get(*k).and_then(epoch_from_json));
    Ok(Some(
        Balance::new("pass", strings::QUOTA_KILO_PASS, (total - usage).max(0.0), Unit::Usd, BalanceKind::Remaining)
            .with_limit(Some(total))
            .resetting(resets_at),
    ))
}

pub fn parse_balance(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let balance =
        number(value.get("balance")).filter(|b| *b >= 0.0).ok_or_else(|| FetchError::Format("no balance".into()))?;
    let balance = Balance::new("balance", strings::QUOTA_BALANCE, balance, Unit::Usd, BalanceKind::Remaining);
    Ok(Fetched { plan: None, windows: Vec::new(), balances: vec![balance] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::model::ProviderId;
    use crate::quota::providers::testing::{credential, response, FakeHttp};
    use crate::quota::providers::{fetch as fetch_provider, hosts};

    const PASS: &str = r#"[{"result": {"data": {"json": {"subscription": {
        "currentPeriodBaseCreditsUsd": 19, "currentPeriodBonusCreditsUsd": 1, "currentPeriodUsageUsd": 15.8,
        "nextBillingAt": "2026-10-09T00:00:00Z"}}}}}]"#;

    #[test]
    fn pass_credits_left_of_the_period() {
        let pass = parse_pass(PASS.as_bytes()).unwrap().unwrap();
        assert!((pass.amount - 4.2).abs() < 1e-9);
        assert_eq!((pass.limit, pass.resets_at), (Some(20.0), Some(1_791_504_000)));
        assert!((pass.used_pct().unwrap() - 79.0).abs() < 1e-9);
        assert_eq!(parse_pass(br#"[{"result": {"data": {"json": {"subscription": null}}}}]"#).unwrap(), None);
        assert!(matches!(parse_pass(br#"[{"error": {}}]"#), Err(FetchError::Format(_))));
    }

    #[test]
    fn without_a_pass_the_balance_is_asked_next() {
        let http = FakeHttp::sequence(vec![
            response(200, None, r#"[{"result": {"data": {"json": {"subscription": null}}}}]"#),
            response(200, None, r#"{"balance": 7.5}"#),
        ]);
        let fetched = fetch_provider(ProviderId::Kilo, &http, &credential("k"), 0).unwrap();
        assert_eq!((fetched.balances[0].key.as_str(), fetched.balances[0].amount), ("balance", 7.5));
        let seen = http.seen.borrow();
        assert_eq!((seen[0].0, seen[0].1.as_deref()), (STATE, Some(STATE_QUERY)));
        assert_eq!((seen[1].0, seen[1].1.as_deref()), (BALANCE, None));
        assert!(seen.iter().all(|(e, _, _)| hosts(ProviderId::Kilo).contains(&e.host)));
    }

    #[test]
    fn a_pass_answers_without_a_second_request() {
        let http = FakeHttp::ok(PASS);
        let fetched = fetch_provider(ProviderId::Kilo, &http, &credential("k"), 0).unwrap();
        assert_eq!(fetched.plan.as_deref(), Some("Kilo Pass"));
        assert_eq!(http.seen.borrow().len(), 1);
    }
}
