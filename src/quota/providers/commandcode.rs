//! Command Code: monthly, purchased and free credits plus five-hour and
//! weekly windows. An organisation's credits need its id, which `whoami`
//! returns; it travels as one encoded query parameter on the fixed path.

use serde_json::Value;

use super::{clean, data, encode_query_value, get, json, number};
use crate::quota::http::{Endpoint, Http};
use crate::quota::model::{Balance, BalanceKind, FetchError, Fetched, Unit, Window};
use crate::quota::time::epoch_from_json;
use crate::strings;

pub const WHOAMI: Endpoint = Endpoint::https("api.commandcode.ai", "/alpha/whoami");
pub const CREDITS: Endpoint = Endpoint::https("api.commandcode.ai", "/alpha/billing/credits");

pub fn fetch(http: &dyn Http, headers: &[(&'static str, String)], now: i64) -> Result<Fetched, FetchError> {
    let whoami = json(&get(http, &WHOAMI, None, headers, now)?)?;
    let who = data(&whoami);
    if who.pointer("/user/id").and_then(Value::as_str).is_none_or(str::is_empty) {
        return Err(FetchError::Format("no user id".into()));
    }
    let query = who
        .pointer("/org/id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(|id| format!("orgId={}", encode_query_value(id)));
    parse_credits(&get(http, &CREDITS, query.as_deref(), headers, now)?)
}

fn window(raw: Option<&Value>, key: &str, label: String) -> Option<Window> {
    let raw = raw?;
    let used = number(raw.get("used"))?;
    let cap = number(raw.get("cap")).filter(|c| *c > 0.0)?;
    let pct = if raw.get("exceeded") == Some(&Value::Bool(true)) { 100.0 } else { used / cap * 100.0 };
    Some(Window::new(key, label, pct, raw.get("resetAt").and_then(epoch_from_json)))
}

pub fn parse_credits(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let root = data(&value);
    let credits =
        root.get("credits").filter(|c| c.is_object()).ok_or_else(|| FetchError::Format("no credits".into()))?;
    let parts = ["monthlyCredits", "purchasedCredits", "freeCredits"].map(|k| number(credits.get(k)));
    if parts.iter().all(Option::is_none) {
        return Err(FetchError::Format("no credit amounts".into()));
    }
    let total: f64 = parts.iter().flatten().sum();
    let balance = Balance::new("credits", strings::QUOTA_CREDITS, total, Unit::Credits, BalanceKind::Remaining);
    let limits = root.get("windowLimits");
    let windows = [
        window(limits.and_then(|l| l.get("fiveHour")), "5h", format!("5{}", strings::QUOTA_UNIT_HOUR)),
        window(limits.and_then(|l| l.get("weekly")), "7d", format!("7{}", strings::QUOTA_UNIT_DAY)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let plan = credits.get("planId").and_then(Value::as_str).map(|p| clean(p, 20)).filter(|p| !p.is_empty());
    Ok(Fetched { plan, windows, balances: vec![balance] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::model::ProviderId;
    use crate::quota::providers::fetch as fetch_provider;
    use crate::quota::providers::testing::{credential, response, FakeHttp};

    const CREDITS_BODY: &str = r#"{"data": {"credits": {"monthlyCredits": 100, "purchasedCredits": 20.5, "freeCredits": null, "planId": "pro"},
        "windowLimits": {"fiveHour": {"used": 30, "cap": 120, "resetAt": 1791210000000},
                         "weekly": {"used": 10, "cap": 100, "exceeded": true}}}}"#;

    #[test]
    fn credits_and_windows() {
        let fetched = parse_credits(CREDITS_BODY.as_bytes()).unwrap();
        assert_eq!(fetched.plan.as_deref(), Some("pro"));
        assert_eq!(fetched.balances[0].amount, 120.5);
        assert_eq!(
            fetched.windows,
            vec![Window::new("5h", "5ч", 25.0, Some(1_791_210_000)), Window::new("7d", "7д", 100.0, None),]
        );
        assert!(matches!(parse_credits(br#"{"credits": {}}"#), Err(FetchError::Format(_))));
    }

    #[test]
    fn an_organisation_id_becomes_one_encoded_parameter() {
        let http = FakeHttp::sequence(vec![
            response(200, None, r#"{"data": {"user": {"id": "u1"}, "org": {"id": "org/7&x=1"}}}"#),
            response(200, None, CREDITS_BODY),
        ]);
        fetch_provider(ProviderId::CommandCode, &http, &credential("k"), 0).unwrap();
        let seen = http.seen.borrow();
        assert_eq!((seen[1].0, seen[1].1.as_deref()), (CREDITS, Some("orgId=org%2F7%26x%3D1")));
        let personal = FakeHttp::sequence(vec![
            response(200, None, r#"{"user": {"id": "u1"}}"#),
            response(200, None, CREDITS_BODY),
        ]);
        fetch_provider(ProviderId::CommandCode, &personal, &credential("k"), 0).unwrap();
        assert_eq!(personal.seen.borrow()[1].1, None);
    }
}
