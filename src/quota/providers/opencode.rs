//! OpenCode: the Go subscription windows (API key) and the Zen balance
//! (OpenCode Console login). Zen amounts are micro-cents: one dollar is
//! 100,000,000.

use serde_json::Value;

use super::{body, get, json, number, send};
use crate::quota::creds::Credential;
use crate::quota::http::{Endpoint, Http};
use crate::quota::model::{format_amount, Balance, BalanceKind, FetchError, Fetched, Unit, Window};
use crate::quota::time::epoch_from_json;
use crate::strings;

pub const GO: Endpoint = Endpoint::https("opencode.ai", "/zen/go/v1/usage");
pub const ZEN_STATUS: Endpoint = Endpoint::https("opencode.ai", "/console/api/billing/status");
pub const ZEN_ACCOUNT: Endpoint = Endpoint::https("opencode.ai", "/console/api/billing/account");
/// The only console server a login may belong to; a login for another one is
/// refused before its token is sent anywhere.
pub const ZEN_SERVER: &str = "https://opencode.ai/console";
const MICRO_CENTS_PER_DOLLAR: f64 = 100_000_000.0;

pub fn fetch_go(http: &dyn Http, headers: &[(&'static str, String)], now: i64) -> Result<Fetched, FetchError> {
    let response = send(http, &GO, None, headers)?;
    if response.status == 403 && not_subscribed(&response.body) {
        return Err(FetchError::Rejected(strings::QUOTA_NO_SUBSCRIPTION().to_owned()));
    }
    parse_go(&body(response, now)?)
}

/// `403 {"type": "error", "error": {"type": "EntitlementError"}}`.
fn not_subscribed(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body).is_ok_and(|v| {
        v.get("type").and_then(Value::as_str) == Some("error")
            && v.pointer("/error/type").and_then(Value::as_str) == Some("EntitlementError")
    })
}

pub fn parse_go(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let usage = value.get("usage").filter(|u| u.is_object()).ok_or_else(|| FetchError::Format("no usage".into()))?;
    let spans = [
        ("rolling", "5h", format!("5{}", strings::QUOTA_UNIT_HOUR())),
        ("weekly", "7d", format!("7{}", strings::QUOTA_UNIT_DAY())),
        ("monthly", "month", strings::QUOTA_MONTH().to_owned()),
    ];
    let mut windows = Vec::new();
    for (field, key, label) in spans {
        let Some(window) = usage.get(field) else { continue };
        let used = match window.get("status").and_then(Value::as_str) {
            Some("rate-limited") => 100.0,
            Some("ok") => match number(window.get("percent")).filter(|p| (0.0..=100.0).contains(p)) {
                Some(percent) => percent,
                None => continue,
            },
            _ => continue,
        };
        windows.push(Window::new(key, label, used, window.get("resetsAt").and_then(epoch_from_json)));
    }
    if windows.is_empty() {
        return Err(FetchError::Format("no usage windows".into()));
    }
    Ok(Fetched::windows(None, windows))
}

pub fn fetch_zen(
    http: &dyn Http,
    credential: &Credential,
    headers: &[(&'static str, String)],
    now: i64,
) -> Result<Fetched, FetchError> {
    if credential.server.as_deref().is_some_and(|server| server.trim_end_matches('/') != ZEN_SERVER) {
        return Err(FetchError::Rejected(strings::QUOTA_ZEN_OTHER_SERVER().to_owned()));
    }
    let status = get(http, &ZEN_STATUS, None, headers, now)?;
    // An expired console session answers with the HTML sign-in page.
    if status.trim_ascii_start().starts_with(b"<") {
        return Err(FetchError::Status { code: 401, retry_after: None });
    }
    let mut fetched = parse_zen_balance(&status)?;
    // The credit limit only adds a line to the hover text; its failure is not one.
    if let Some(limit) = get(http, &ZEN_ACCOUNT, None, headers, now).ok().and_then(|b| parse_zen_credit_limit(&b)) {
        let detail = format!("{} {}", strings::QUOTA_CREDIT_LIMIT(), format_amount(limit, Unit::Usd));
        if let Some(balance) = fetched.balances.first_mut() {
            balance.detail = Some(detail);
        }
    }
    Ok(fetched)
}

fn micro_cents(value: Option<&Value>) -> Option<f64> {
    number(value).map(|mc| mc / MICRO_CENTS_PER_DOLLAR)
}

pub fn parse_zen_balance(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let dollars = micro_cents(value.get("balanceMicroCents")).ok_or_else(|| FetchError::Format("no balance".into()))?;
    let balance =
        Balance::new("balance", strings::QUOTA_BALANCE(), dollars.max(0.0), Unit::Usd, BalanceKind::Remaining);
    Ok(Fetched { plan: None, windows: Vec::new(), balances: vec![balance] })
}

/// The account's credit limit in dollars; `None` when it has none.
pub fn parse_zen_credit_limit(body: &[u8]) -> Option<f64> {
    let value: Value = serde_json::from_slice(body).ok()?;
    micro_cents(value.get("creditLimitMicroCents")).filter(|l| *l > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::model::ProviderId;
    use crate::quota::providers::fetch as fetch_provider;
    use crate::quota::providers::testing::{credential, response, FakeHttp};

    #[test]
    fn go_windows_and_exhaustion() {
        let body = br#"{"usage": {
            "rolling": {"status": "ok", "percent": 34, "resetsAt": "2026-10-05T14:20:00+00:00"},
            "weekly": {"status": "rate-limited", "percent": 12, "resetsAt": "2026-10-09T00:00:00Z"},
            "monthly": {"status": "weird", "percent": 1}}}"#;
        assert_eq!(
            parse_go(body).unwrap().windows,
            vec![
                Window::new("5h", "5ч", 34.0, Some(1_791_210_000)),
                Window::new("7d", "7д", 100.0, Some(1_791_504_000)),
            ]
        );
    }

    #[test]
    fn go_without_a_subscription_says_so() {
        let http = FakeHttp::status(403, None, r#"{"type": "error", "error": {"type": "EntitlementError"}}"#);
        assert_eq!(
            fetch_provider(ProviderId::OpencodeGo, &http, &credential("k"), 0),
            Err(FetchError::Rejected("нет подписки".into()))
        );
        let http = FakeHttp::status(403, None, "forbidden");
        assert_eq!(
            fetch_provider(ProviderId::OpencodeGo, &http, &credential("k"), 0),
            Err(FetchError::Status { code: 403, retry_after: None })
        );
    }

    #[test]
    fn zen_balance_with_its_credit_limit() {
        let http = FakeHttp::sequence(vec![
            response(200, None, r#"{"balanceMicroCents": "875000000"}"#),
            response(200, None, r#"{"creditLimitMicroCents": 2000000000}"#),
        ]);
        let fetched = fetch_provider(ProviderId::OpencodeZen, &http, &credential("t"), 0).unwrap();
        let balance = &fetched.balances[0];
        assert_eq!((balance.amount, balance.unit), (8.75, Unit::Usd));
        assert_eq!(balance.detail.as_deref(), Some("кредитный лимит $20.00"));
        let seen = http.seen.borrow();
        assert_eq!((seen[0].0, seen[1].0), (ZEN_STATUS, ZEN_ACCOUNT));
    }

    #[test]
    fn zen_refuses_other_servers_and_reads_html_as_an_expired_session() {
        let mut elsewhere = credential("t");
        elsewhere.server = Some("https://console.example.com".into());
        let http = FakeHttp::ok("{}");
        assert!(matches!(fetch_provider(ProviderId::OpencodeZen, &http, &elsewhere, 0), Err(FetchError::Rejected(_))));
        assert!(http.seen.borrow().is_empty(), "the token never left");
        let mut default = credential("t");
        default.server = Some("https://opencode.ai/console/".into());
        let http = FakeHttp::ok("<!doctype html><html>sign in</html>");
        assert_eq!(
            fetch_provider(ProviderId::OpencodeZen, &http, &default, 0),
            Err(FetchError::Status { code: 401, retry_after: None })
        );
    }

    #[test]
    fn zen_balance_without_a_limit_route() {
        let http = FakeHttp::sequence(vec![
            response(200, None, r#"{"balanceMicroCents": 100000000}"#),
            response(500, None, ""),
        ]);
        let fetched = fetch_provider(ProviderId::OpencodeZen, &http, &credential("t"), 0).unwrap();
        assert_eq!((fetched.balances[0].amount, fetched.balances[0].detail.as_deref()), (1.0, None));
    }
}
