//! Ollama Cloud: session, weekly and monthly usage as fractions. The key goes
//! in `Authorization` bare. One reference (the OpenCode plugin) uses this
//! endpoint, another (OMP) says Ollama has no usage API; a changed or missing
//! endpoint is an error for this provider only.

use serde_json::Value;

use super::{json, number};
use crate::quota::http::Endpoint;
use crate::quota::model::{FetchError, Fetched, Window};
use crate::quota::time::epoch_from_json;
use crate::strings;

pub const ENDPOINT: Endpoint = Endpoint::https("ollama.com", "/api/usage");

fn window(value: Option<&Value>) -> Option<(f64, Option<i64>)> {
    let value = value?;
    let usage = number(value.get("usage")).filter(|u| (0.0..=1.0).contains(u))?;
    let resets_at = ["resets_at", "resetsAt", "reset_at"].iter().find_map(|k| value.get(*k).and_then(epoch_from_json));
    Some((usage * 100.0, resets_at))
}

pub fn parse(body: &[u8]) -> Result<Fetched, FetchError> {
    let value = json(body)?;
    let limits = value.get("limits").ok_or_else(|| FetchError::Format("no limits".into()))?;
    let mut windows = Vec::new();
    let spans = [
        ("session", "session", strings::QUOTA_SESSION.to_owned()),
        ("weekly", "7d", format!("7{}", strings::QUOTA_UNIT_DAY)),
        ("monthly", "month", strings::QUOTA_MONTH.to_owned()),
    ];
    for (field, key, label) in spans {
        if let Some((used, resets_at)) = window(limits.get(field)) {
            windows.push(Window::new(key, label, used, resets_at));
        }
    }
    if windows.is_empty() {
        return Err(FetchError::Format("no usable usage data".into()));
    }
    Ok(Fetched::windows(None, windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions_become_windows() {
        let body = br#"{"limits": {"session": {"usage": 0.25}, "weekly": {"usage": 0.5, "resets_at": 1791504000},
                                   "monthly": {"usage": 1.5}}}"#;
        let fetched = parse(body).unwrap();
        assert_eq!(
            fetched.windows,
            vec![Window::new("session", "сессия", 25.0, None), Window::new("7d", "7д", 50.0, Some(1_791_504_000)),],
            "a fraction above 1 is ignored"
        );
        assert!(matches!(parse(br#"{"limits": {}}"#), Err(FetchError::Format(_))));
    }
}
