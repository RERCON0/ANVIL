//! One module per provider: its fixed endpoints and a pure parser of its
//! answers. `fetch` is the only place a credential meets the network, and
//! every address it reaches is a constant of that provider's module, never a
//! value from a config; `hosts` lists them for the pinning test.

pub mod charm;
pub mod chatgpt;
pub mod chutes;
pub mod claude;
pub mod commandcode;
pub mod deepseek;
pub mod glm;
pub mod kilo;
pub mod kimi;
pub mod minimax;
pub mod ollama;
pub mod opencode;
pub mod openrouter;
pub mod synthetic;
pub mod umans;

use serde_json::Value;

use super::creds::Credential;
use super::http::{Endpoint, Http, Response};
use super::model::{FetchError, Fetched, ProviderId};
use super::time;

/// Every host a provider's credential may be sent to.
pub fn hosts(id: ProviderId) -> &'static [&'static str] {
    match id {
        ProviderId::Claude => &["api.anthropic.com"],
        ProviderId::ChatGpt => &["chatgpt.com"],
        ProviderId::Zai => &["api.z.ai"],
        ProviderId::Zhipu => &["bigmodel.cn"],
        ProviderId::Kimi => &["api.kimi.com"],
        ProviderId::KimiAi => &["api.kimi.ai"],
        ProviderId::MiniMax => &["api.minimax.io"],
        ProviderId::MiniMaxCn => &["api.minimaxi.com"],
        ProviderId::OpencodeGo | ProviderId::OpencodeZen => &["opencode.ai"],
        ProviderId::Synthetic => &["api.synthetic.new"],
        ProviderId::OllamaCloud => &["ollama.com"],
        ProviderId::Chutes => &["api.chutes.ai"],
        ProviderId::CommandCode => &["api.commandcode.ai"],
        ProviderId::Umans => &["api.code.umans.ai"],
        ProviderId::DeepSeek => &["api.deepseek.com"],
        ProviderId::OpenRouter => &["openrouter.ai"],
        ProviderId::Kilo => &["app.kilo.ai", "api.kilo.ai"],
        ProviderId::CharmHyper => &["hyper.charm.land"],
    }
}

/// `Accept: application/json` plus `Authorization: Bearer <secret>`.
fn bearer(secret: &str) -> Vec<(&'static str, String)> {
    vec![("Accept", "application/json".to_owned()), ("Authorization", format!("Bearer {secret}"))]
}

fn headers(id: ProviderId, credential: &Credential) -> Vec<(&'static str, String)> {
    let secret = credential.secret.expose();
    let mut headers = match id {
        // GLM coding plans and Ollama take the bare key, without a scheme.
        ProviderId::Zai | ProviderId::Zhipu | ProviderId::OllamaCloud => {
            vec![("Accept", "application/json".to_owned()), ("Authorization", secret.to_owned())]
        }
        _ => bearer(secret),
    };
    match id {
        ProviderId::Claude => headers.push(("anthropic-beta", claude::BETA.to_owned())),
        ProviderId::ChatGpt => {
            if let Some(account) = &credential.account {
                headers.push(("ChatGPT-Account-Id", account.clone()));
            }
        }
        ProviderId::OpencodeZen => {
            if let Some(org) = &credential.org {
                headers.push(("x-org-id", org.clone()));
            }
        }
        _ => {}
    }
    headers
}

/// One GET; transport failures become `Network`, everything else is returned.
fn send(
    http: &dyn Http,
    endpoint: &Endpoint,
    query: Option<&str>,
    headers: &[(&'static str, String)],
) -> Result<Response, FetchError> {
    let refs: Vec<(&str, &str)> = headers.iter().map(|(name, value)| (*name, value.as_str())).collect();
    http.get(endpoint, query, &refs).map_err(|e| FetchError::Network(e.to_string()))
}

/// The body of a 2xx answer; any other status becomes `FetchError::Status`.
fn body(response: Response, now: i64) -> Result<Vec<u8>, FetchError> {
    if (200..300).contains(&response.status) {
        return Ok(response.body);
    }
    let retry_after = response.retry_after.as_deref().and_then(|value| time::retry_after_secs(value, now));
    Err(FetchError::Status { code: response.status, retry_after })
}

fn get(
    http: &dyn Http,
    endpoint: &Endpoint,
    query: Option<&str>,
    headers: &[(&'static str, String)],
    now: i64,
) -> Result<Vec<u8>, FetchError> {
    body(send(http, endpoint, query, headers)?, now)
}

/// The fixed endpoint of a provider answered by a single request.
fn single(id: ProviderId) -> Option<Endpoint> {
    Some(match id {
        ProviderId::Claude => claude::ENDPOINT,
        ProviderId::ChatGpt => chatgpt::ENDPOINT,
        ProviderId::Zai => glm::ZAI,
        ProviderId::Zhipu => glm::ZHIPU,
        ProviderId::Kimi => kimi::KIMI,
        ProviderId::KimiAi => kimi::KIMI_AI,
        ProviderId::MiniMax => minimax::INTERNATIONAL,
        ProviderId::MiniMaxCn => minimax::CHINA,
        ProviderId::Synthetic => synthetic::ENDPOINT,
        ProviderId::OllamaCloud => ollama::ENDPOINT,
        ProviderId::Chutes => chutes::ENDPOINT,
        ProviderId::Umans => umans::ENDPOINT,
        ProviderId::DeepSeek => deepseek::ENDPOINT,
        ProviderId::OpenRouter => openrouter::ENDPOINT,
        ProviderId::CharmHyper => charm::ENDPOINT,
        ProviderId::OpencodeGo | ProviderId::OpencodeZen | ProviderId::Kilo | ProviderId::CommandCode => return None,
    })
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
        ProviderId::Synthetic => synthetic::parse(body),
        ProviderId::OllamaCloud => ollama::parse(body),
        ProviderId::Chutes => chutes::parse(body, now),
        ProviderId::Umans => umans::parse(body),
        ProviderId::DeepSeek => deepseek::parse(body),
        ProviderId::OpenRouter => openrouter::parse(body),
        ProviderId::CharmHyper => charm::parse(body),
        ProviderId::OpencodeGo => opencode::parse_go(body),
        ProviderId::OpencodeZen => opencode::parse_zen_balance(body),
        ProviderId::Kilo => kilo::parse_balance(body),
        ProviderId::CommandCode => commandcode::parse_credits(body),
    }
}

/// The requests of one provider, status mapping and parsing.
pub fn fetch(id: ProviderId, http: &dyn Http, credential: &Credential, now: i64) -> Result<Fetched, FetchError> {
    let headers = headers(id, credential);
    match id {
        ProviderId::OpencodeGo => opencode::fetch_go(http, &headers, now),
        ProviderId::OpencodeZen => opencode::fetch_zen(http, credential, &headers, now),
        ProviderId::Kilo => kilo::fetch(http, &headers, now),
        ProviderId::CommandCode => commandcode::fetch(http, &headers, now),
        _ => {
            let endpoint = single(id).expect("every other provider answers one request");
            parse(id, &get(http, &endpoint, None, &headers, now)?, now)
        }
    }
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

/// Percent-encodes everything but the unreserved characters, so a value a
/// provider returned can only ever become one query parameter.
fn encode_query_value(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `{"data": {...}}` or the object itself.
fn data(value: &Value) -> &Value {
    value.get("data").filter(|d| d.is_object()).unwrap_or(value)
}

#[cfg(test)]
pub(crate) mod testing {
    use std::cell::RefCell;

    use crate::quota::creds::{Credential, Secret, Source};
    use crate::quota::http::{Endpoint, Http, HttpError, Response};

    /// One recorded request: where it went, its query and its headers.
    pub type Seen = (Endpoint, Option<String>, Vec<(String, String)>);

    /// Records every request and answers from a queue; the last answer
    /// repeats once the queue is down to it.
    pub struct FakeHttp {
        pub answers: RefCell<Vec<Result<Response, HttpError>>>,
        pub seen: RefCell<Vec<Seen>>,
    }

    pub fn response(status: u16, retry_after: Option<&str>, body: &str) -> Result<Response, HttpError> {
        Ok(Response { status, retry_after: retry_after.map(str::to_owned), body: body.as_bytes().to_vec() })
    }

    impl FakeHttp {
        pub fn ok(body: &str) -> FakeHttp {
            FakeHttp::sequence(vec![response(200, None, body)])
        }

        pub fn status(status: u16, retry_after: Option<&str>, body: &str) -> FakeHttp {
            FakeHttp::sequence(vec![response(status, retry_after, body)])
        }

        pub fn sequence(answers: Vec<Result<Response, HttpError>>) -> FakeHttp {
            FakeHttp { answers: RefCell::new(answers), seen: RefCell::new(Vec::new()) }
        }
    }

    impl Http for FakeHttp {
        fn get(
            &self,
            endpoint: &Endpoint,
            query: Option<&str>,
            headers: &[(&str, &str)],
        ) -> Result<Response, HttpError> {
            let headers = headers.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect();
            self.seen.borrow_mut().push((*endpoint, query.map(str::to_owned), headers));
            let mut answers = self.answers.borrow_mut();
            if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers[0].clone()
            }
        }
    }

    pub fn credential(secret: &str) -> Credential {
        Credential {
            secret: Secret::new(secret),
            source: Source::AnvilKey,
            plan: None,
            account: None,
            org: None,
            server: None,
            expires_at: None,
            marker: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{credential, FakeHttp};
    use super::*;

    /// Every credential goes to its own provider's hosts and nowhere else, on
    /// HTTPS port 443, whatever the answers are.
    #[test]
    fn each_provider_talks_only_to_its_own_hosts() {
        for id in ProviderId::ALL {
            for body in ["{}", r#"{"data": {"user": {"id": "u"}, "org": {"id": "o/1"}}}"#] {
                let http = FakeHttp::ok(body);
                let _ = fetch(id, &http, &credential("s3cret"), 0);
                let seen = http.seen.borrow();
                assert!(!seen.is_empty(), "{id:?}");
                for (endpoint, query, headers) in seen.iter() {
                    assert!(hosts(id).contains(&endpoint.host), "{id:?} reached {}", endpoint.host);
                    assert_eq!((endpoint.port, endpoint.secure), (443, true), "{id:?}");
                    assert!(query.as_deref().is_none_or(crate::quota::http::valid_query), "{id:?}: {query:?}");
                    let auth = headers.iter().find(|(n, _)| n == "Authorization").map(|(_, v)| v.as_str());
                    let bare = matches!(id, ProviderId::Zai | ProviderId::Zhipu | ProviderId::OllamaCloud);
                    assert_eq!(auth, Some(if bare { "s3cret" } else { "Bearer s3cret" }), "{id:?}");
                }
            }
        }
    }

    #[test]
    fn single_request_endpoints() {
        let expected = [
            (ProviderId::Claude, "api.anthropic.com", "/api/oauth/usage"),
            (ProviderId::ChatGpt, "chatgpt.com", "/backend-api/wham/usage"),
            (ProviderId::Zai, "api.z.ai", "/api/monitor/usage/quota/limit"),
            (ProviderId::Zhipu, "bigmodel.cn", "/api/monitor/usage/quota/limit"),
            (ProviderId::Kimi, "api.kimi.com", "/coding/v1/usages"),
            (ProviderId::KimiAi, "api.kimi.ai", "/coding/v1/usages"),
            (ProviderId::MiniMax, "api.minimax.io", "/v1/token_plan/remains"),
            (ProviderId::MiniMaxCn, "api.minimaxi.com", "/v1/token_plan/remains"),
            (ProviderId::Synthetic, "api.synthetic.new", "/v2/quotas"),
            (ProviderId::OllamaCloud, "ollama.com", "/api/usage"),
            (ProviderId::Chutes, "api.chutes.ai", "/users/me/quota_usage/me"),
            (ProviderId::Umans, "api.code.umans.ai", "/v1/usage"),
            (ProviderId::DeepSeek, "api.deepseek.com", "/user/balance"),
            (ProviderId::OpenRouter, "openrouter.ai", "/api/v1/key"),
            (ProviderId::CharmHyper, "hyper.charm.land", "/v1/credits"),
        ];
        for (id, host, path) in expected {
            let endpoint = single(id).unwrap();
            assert_eq!((endpoint.host, endpoint.path), (host, path), "{id:?}");
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
        let http = FakeHttp::sequence(vec![Err(crate::quota::http::HttpError::Timeout)]);
        assert_eq!(fetch(ProviderId::Claude, &http, &credential("k"), 0), Err(FetchError::Network("timeout".into())));
        let http = FakeHttp::ok("<html>");
        assert!(matches!(fetch(ProviderId::ChatGpt, &http, &credential("k"), 0), Err(FetchError::Format(_))));
    }

    #[test]
    fn extra_headers_only_when_known() {
        let http = FakeHttp::ok("{}");
        let mut with_account = credential("t");
        with_account.account = Some("acc-1".into());
        let _ = fetch(ProviderId::ChatGpt, &http, &with_account, 0);
        let _ = fetch(ProviderId::ChatGpt, &http, &credential("t"), 0);
        let mut with_org = credential("t");
        with_org.org = Some("org-7".into());
        let _ = fetch(ProviderId::OpencodeZen, &http, &with_org, 0);
        let seen = http.seen.borrow();
        assert!(seen[0].2.iter().any(|(n, v)| n == "ChatGPT-Account-Id" && v == "acc-1"));
        assert!(!seen[1].2.iter().any(|(n, _)| n == "ChatGPT-Account-Id"));
        assert!(seen[2].2.iter().any(|(n, v)| n == "x-org-id" && v == "org-7"));
    }

    #[test]
    fn query_values_are_encoded_into_one_parameter() {
        assert_eq!(encode_query_value("org_1-a.b~"), "org_1-a.b~");
        assert_eq!(encode_query_value("a/b?c=d&e"), "a%2Fb%3Fc%3Dd%26e");
        assert_eq!(encode_query_value("ключ"), "%D0%BA%D0%BB%D1%8E%D1%87");
    }
}
