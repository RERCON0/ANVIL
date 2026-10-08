//! Charm Hyper: the prepaid credit balance of the account.

use super::{json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{Balance, BalanceKind, FetchError, Fetched, Unit};
use crate::strings;

pub const ENDPOINT: Endpoint = Endpoint::https("hyper.charm.land", "/v1/credits");

pub fn parse(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let balance = number(value.get("balance")).ok_or_else(|| FetchError::Format("no balance".into()))?;
    let credits = Balance::new("credits", strings::QUOTA_CREDITS(), balance, Unit::Credits, BalanceKind::Remaining);
    Ok(Fetched { plan: None, windows: Vec::new(), balances: vec![credits] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_balance() {
        let fetched = parse(br#"{"balance": 1234.5}"#).unwrap();
        assert_eq!(fetched.balances[0].amount, 1234.5);
        assert_eq!(fetched.balances[0].unit, Unit::Credits);
        assert!(matches!(parse(br#"{"credits": 3}"#), Err(FetchError::Format(_))));
    }
}
