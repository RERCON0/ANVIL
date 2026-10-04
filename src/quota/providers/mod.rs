//! One module per provider: its fixed endpoint and a pure parser of its
//! answer. `fetch` is the only place a credential meets the network, and the
//! address it goes to always comes from `endpoint(id)`, never from a config.

pub mod chatgpt;
pub mod claude;
pub mod glm;
pub mod kimi;
pub mod minimax;

use serde_json::Value;

use super::creds::Credential;
use super::http::{Endpoint, Http};
use super::model::{FetchError, Fetched, ProviderId};
use super::time;

pub fn endpoint(id: ProviderId) -> Endpoint {
    match id {
        ProviderId::Claude => claude::ENDPOINT,
        ProviderId::ChatGpt => chatgpt::ENDPOINT,
        ProviderId::Zai => glm::ZAI,
        ProviderId::Zhipu => glm::ZHIPU,
        ProviderId::Kimi => kimi::KIMI,
        ProviderId::KimiAi => kimi::KIMI_AI,
        ProviderId::MiniMax => minimax::INTERNATIONAL,
        ProviderId::MiniMaxCn => minimax::CHINA,
    }
}

fn headers(id: ProviderId, credential: &Credential) -> Vec<(&'static str, String)> {
    let secret = credential.secret.expose();
    let bearer = format!("Bearer {secret}");
    let mut headers = vec![("Accept", "application/json".to_owned())];
    match id {
        ProviderId::Claude => {
            headers.push(("Authorization", bearer));
            headers.push(("anthropic-beta", claude::BETA.to_owned()));
        }
        ProviderId::ChatGpt => {
            headers.push(("Authorization", bearer));
            if let Some(account) = &credential.account {
                headers.push(("ChatGPT-Account-Id", account.clone()));
            }
        }
        // GLM coding plans take the bare key, without a scheme.
        ProviderId::Zai | ProviderId::Zhipu => headers.push(("Authorization", secret.to_owned())),
        ProviderId::Kimi | ProviderId::KimiAi | ProviderId::MiniMax | ProviderId::MiniMaxCn => {
            headers.push(("Authorization", bearer))
        }
    }
    headers
}

pub fn parse(id: ProviderId, body: &[u8], now: i64) -> Result<Fetched, FetchError> {
    match id {
        ProviderId::Claude => claude::parse(body),
        ProviderId::ChatGpt => chatgpt::parse(body, now),
        ProviderId::Zai => glm::parse(body, glm::Envelope::Zai),
        ProviderId::Zhipu => glm::parse(body, glm::Envelope::Zhipu),
        ProviderId::Kimi | ProviderId::KimiAi => kimi::parse(body, now),
        ProviderId::MiniMax => minimax::parse(body, now, minimax::Region::International),
        ProviderId::MiniMaxCn => minimax::parse(body, now, minimax::Region::China),
    }
}

/// One request to the provider's own endpoint, status mapping and parsing.
pub fn fetch(id: ProviderId, http: &dyn Http, credential: &Credential, now: i64) -> Result<Fetched, FetchError> {
    let headers = headers(id, credential);
    let refs: Vec<(&str, &str)> = headers.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let response = http.get(&endpoint(id), &refs).map_err(|e| FetchError::Network(e.to_string()))?;
    if !(200..300).contains(&response.status) {
        let retry_after = response.retry_after.as_deref().and_then(|value| time::retry_after_secs(value, now));
        return Err(FetchError::Status { code: response.status, retry_after });
    }
    parse(id, &response.body, now)
}

fn json(body: &[u8]) -> Result<Value, FetchError> {
    serde_json::from_slice(body).map_err(|e| FetchError::Format(format!("JSON: {e}")))
}

/// A finite number, given as a JSON number or a numeric string.
fn number(value: Option<&Value>) -> Option<f64> {
    let n = match value? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// Provider-supplied text for a message or label: one line, bounded.
fn clean(text: &str, max: usize) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(max).collect::<String>().trim().to_owned()
}

#[cfg(test)]
pub(crate) mod testing {
    use std::cell::RefCell;

    use crate::quota::creds::{Credential, Secret, Source};
    use crate::quota::http::{Endpoint, Http, HttpError, Response};

    /// One recorded request: where it went and its headers.
    pub type Seen = (Endpoint, Vec<(String, String)>);

    /// Records every request and answers with a canned response.
    pub struct FakeHttp {
        pub answer: Result<Response, HttpError>,
        pub seen: RefCell<Vec<Seen>>,
    }

    impl FakeHttp {
        pub fn ok(body: &str) -> FakeHttp {
            FakeHttp::status(200, None, body)
        }

        pub fn status(status: u16, retry_after: Option<&str>, body: &str) -> FakeHttp {
            FakeHttp {
                answer: Ok(Response {
                    status,
                    retry_after: retry_after.map(str::to_owned),
                    body: body.as_bytes().to_vec(),
                }),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl Http for FakeHttp {
        fn get(&self, endpoint: &Endpoint, headers: &[(&str, &str)]) -> Result<Response, HttpError> {
            let headers = headers.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect();
            self.seen.borrow_mut().push((*endpoint, headers));
            self.answer.clone()
        }
    }

    pub fn credential(secret: &str) -> Credential {
        Credential {
            secret: Secret::new(secret),
            source: Source::AnvilKey,
            plan: None,
            account: None,
            expires_at: None,
            marker: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{credential, FakeHttp};
    use super::*;

    /// Every credential goes to its own provider's host and nowhere else.
    #[test]
    fn each_provider_talks_only_to_its_own_host() {
        let expected = [
            (ProviderId::Claude, "api.anthropic.com", "/api/oauth/usage"),
            (ProviderId::ChatGpt, "chatgpt.com", "/backend-api/wham/usage"),
            (ProviderId::Zai, "api.z.ai", "/api/monitor/usage/quota/limit"),
            (ProviderId::Zhipu, "bigmodel.cn", "/api/monitor/usage/quota/limit"),
            (ProviderId::Kimi, "api.kimi.com", "/coding/v1/usages"),
            (ProviderId::KimiAi, "api.kimi.ai", "/coding/v1/usages"),
            (ProviderId::MiniMax, "api.minimax.io", "/v1/token_plan/remains"),
            (ProviderId::MiniMaxCn, "api.minimaxi.com", "/v1/token_plan/remains"),
        ];
        for (id, host, path) in expected {
            let http = FakeHttp::ok("{}");
            let _ = fetch(id, &http, &credential("s3cret"), 0);
            let seen = http.seen.borrow();
            assert_eq!(seen.len(), 1, "{id:?}");
            let (endpoint, headers) = &seen[0];
            assert_eq!(
                (endpoint.host, endpoint.path, endpoint.port, endpoint.secure),
                (host, path, 443, true),
                "{id:?}"
            );
            let auth = headers.iter().find(|(n, _)| n == "Authorization").map(|(_, v)| v.as_str());
            let want = if matches!(id, ProviderId::Zai | ProviderId::Zhipu) { "s3cret" } else { "Bearer s3cret" };
            assert_eq!(auth, Some(want), "{id:?}");
        }
    }

    #[test]
    fn statuses_and_transport_errors_are_mapped() {
        let http = FakeHttp::status(429, Some("120"), "");
        assert_eq!(
            fetch(ProviderId::Zai, &http, &credential("k"), 0),
            Err(FetchError::Status { code: 429, retry_after: Some(120) })
        );
        let http = FakeHttp::status(401, None, "nope");
        assert_eq!(
            fetch(ProviderId::Kimi, &http, &credential("k"), 0),
            Err(FetchError::Status { code: 401, retry_after: None })
        );
        let http = FakeHttp { answer: Err(crate::quota::http::HttpError::Timeout), seen: Default::default() };
        assert_eq!(fetch(ProviderId::Claude, &http, &credential("k"), 0), Err(FetchError::Network("timeout".into())));
        let http = FakeHttp::ok("<html>");
        assert!(matches!(fetch(ProviderId::ChatGpt, &http, &credential("k"), 0), Err(FetchError::Format(_))));
    }

    #[test]
    fn chatgpt_sends_the_account_header_only_when_known() {
        let http = FakeHttp::ok("{}");
        let mut with_account = credential("t");
        with_account.account = Some("acc-1".into());
        let _ = fetch(ProviderId::ChatGpt, &http, &with_account, 0);
        let _ = fetch(ProviderId::ChatGpt, &http, &credential("t"), 0);
        let seen = http.seen.borrow();
        assert!(seen[0].1.iter().any(|(n, v)| n == "ChatGPT-Account-Id" && v == "acc-1"));
        assert!(!seen[1].1.iter().any(|(n, _)| n == "ChatGPT-Account-Id"));
    }
}
