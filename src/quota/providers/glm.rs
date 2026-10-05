//! GLM Coding Plan: Z.ai (international) and Zhipu (bigmodel.cn) share one
//! monitoring API. The key goes in `Authorization` bare, without `Bearer`.

use serde_json::Value;

use super::{clean, json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{FetchError, Fetched, Window};
use crate::quota::time::epoch_from_json;
use crate::strings;

pub const ZAI: Endpoint = Endpoint::https("api.z.ai", "/api/monitor/usage/quota/limit");
pub const ZHIPU: Endpoint = Endpoint::https("bigmodel.cn", "/api/monitor/usage/quota/limit");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Envelope {
    /// Limits under `data.limits`, or at the top level.
    Zai,
    /// Limits under `data.limits` only.
    Zhipu,
}

pub fn parse(body: &[u8], envelope: Envelope) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let code = value.get("code").and_then(Value::as_i64);
    if value.get("success") == Some(&Value::Bool(false)) || code.is_some_and(|c| c >= 400) {
        let message = value.get("msg").and_then(Value::as_str).map(|m| clean(m, 120)).unwrap_or_default();
        let message = if message.is_empty() { format!("code {}", code.unwrap_or(0)) } else { message };
        return Err(FetchError::Rejected(message));
    }
    let limits = value
        .pointer("/data/limits")
        .or_else(|| (envelope == Envelope::Zai).then(|| value.get("limits")).flatten())
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Format("no limits".into()))?;
    let mut windows: Vec<Window> = Vec::new();
    for limit in limits {
        let kind = limit.get("type").and_then(Value::as_str).unwrap_or("");
        let (key, label) = match (kind, limit.get("unit").and_then(Value::as_i64)) {
            ("TOKENS_LIMIT" | "CREDIT_LIMIT", Some(3)) => ("5h", format!("5{}", strings::QUOTA_UNIT_HOUR)),
            ("TOKENS_LIMIT" | "CREDIT_LIMIT", Some(6)) => ("7d", format!("7{}", strings::QUOTA_UNIT_DAY)),
            ("TIME_LIMIT", _) => ("mcp", strings::QUOTA_MCP.to_owned()),
            _ => continue,
        };
        let credit = match (kind, number(limit.get("usage")), number(limit.get("currentValue"))) {
            ("CREDIT_LIMIT", Some(total), Some(current)) if total > 0.0 && current >= 0.0 => {
                Some(current / total * 100.0)
            }
            _ => None,
        };
        let Some(used) = credit.or_else(|| number(limit.get("percentage"))) else { continue };
        if !windows.iter().any(|w| w.key == key) {
            windows.push(Window::new(key, label, used, limit.get("nextResetTime").and_then(epoch_from_json)));
        }
    }
    if windows.is_empty() {
        return Err(FetchError::Format("no known limits".into()));
    }
    Ok(Fetched::windows(None, windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &[u8] = br#"{"code": 200, "success": true, "data": {"limits": [
        {"type": "TOKENS_LIMIT", "unit": 3, "percentage": 3, "nextResetTime": 1791210000000},
        {"type": "CREDIT_LIMIT", "unit": 6, "percentage": 99, "usage": 2000, "currentValue": 500},
        {"type": "TIME_LIMIT", "unit": 5, "percentage": 10},
        {"type": "SOMETHING_NEW", "unit": 3, "percentage": 50}]}}"#;

    #[test]
    fn five_hour_week_and_mcp() {
        let fetched = parse(BODY, Envelope::Zhipu).unwrap();
        assert_eq!(
            fetched.windows,
            vec![
                Window::new("5h", "5ч", 3.0, Some(1_791_210_000)),
                Window::new("7d", "7д", 25.0, None),
                Window::new("mcp", "MCP", 10.0, None),
            ]
        );
    }

    #[test]
    fn top_level_limits_only_for_zai() {
        let body = br#"{"limits": [{"type": "TOKENS_LIMIT", "unit": 3, "percentage": 1}]}"#;
        assert_eq!(parse(body, Envelope::Zai).unwrap().windows.len(), 1);
        assert!(matches!(parse(body, Envelope::Zhipu), Err(FetchError::Format(_))));
    }

    #[test]
    fn api_refusals() {
        assert_eq!(
            parse(br#"{"success": false, "code": 1001, "msg": "Authorization\nfailed"}"#, Envelope::Zai),
            Err(FetchError::Rejected("Authorization failed".into()))
        );
        assert_eq!(parse(br#"{"code": 500}"#, Envelope::Zai), Err(FetchError::Rejected("code 500".into())));
    }
}
