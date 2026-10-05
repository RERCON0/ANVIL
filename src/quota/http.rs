//! HTTPS GET over WinHTTP for the quota worker: the system's TLS, certificate
//! store and proxy, no redirects (an `Authorization` header must never reach
//! another host), fixed timeouts and a cap on the body. Only the worker thread
//! calls it; nothing here may run on the UI thread.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER};
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts, ERROR_WINHTTP_TIMEOUT,
    WINHTTP_ACCESS_TYPE, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_DISABLE_COOKIES, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_RETRY_AFTER,
    WINHTTP_QUERY_STATUS_CODE,
};

/// Larger answers are refused: every quota response is a few kilobytes.
pub const BODY_LIMIT: usize = 256 * 1024;
const TIMEOUT_MS: i32 = 10_000;

/// Where a request goes. Production endpoints are `const` HTTPS values written
/// in each provider; plain HTTP exists only for the loopback test server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub host: &'static str,
    pub port: u16,
    pub path: &'static str,
    pub secure: bool,
}

impl Endpoint {
    pub const fn https(host: &'static str, path: &'static str) -> Endpoint {
        Endpoint { host, port: 443, path, secure: true }
    }

    #[cfg(test)]
    pub const fn loopback(port: u16, path: &'static str) -> Endpoint {
        Endpoint { host: "127.0.0.1", port, path, secure: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Raw `Retry-After` header, if any.
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpError {
    Timeout,
    TooLarge,
    /// A 3xx answer: redirects are never followed.
    Redirect(u16),
    /// Plain HTTP outside tests.
    Insecure,
    /// A query string with characters that could change the address.
    BadQuery,
    /// A header name or value that could inject a header of its own.
    Header,
    /// A WinHTTP call failed with this `GetLastError` code.
    Os(u32),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Timeout => f.write_str("timeout"),
            HttpError::TooLarge => f.write_str("response too large"),
            HttpError::Redirect(code) => write!(f, "unexpected redirect {code}"),
            HttpError::Insecure => f.write_str("plain HTTP refused"),
            HttpError::BadQuery => f.write_str("invalid query string"),
            HttpError::Header => f.write_str("invalid header"),
            HttpError::Os(code) => write!(f, "WinHTTP error {code}"),
        }
    }
}

/// The transport a provider fetch goes through; tests substitute a fake.
/// `query` is appended to the endpoint's fixed path after `?`; it must pass
/// [`valid_query`], so it can never change the host or the path.
pub trait Http {
    fn get(&self, endpoint: &Endpoint, query: Option<&str>, headers: &[(&str, &str)]) -> Result<Response, HttpError>;
}

/// Characters of a percent-encoded query only: no `/`, `?`, `#`, `@`, `:`,
/// spaces or controls, and a bounded length.
pub fn valid_query(query: &str) -> bool {
    !query.is_empty()
        && query.len() <= 512
        && query.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.~=&%".contains(&b))
}

struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void) -> Result<Handle, HttpError> {
        if raw.is_null() {
            Err(last_error())
        } else {
            Ok(Handle(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from WinHttpOpen/Connect/OpenRequest and is
        // closed exactly once, children before parents (reverse drop order).
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn last_error() -> HttpError {
    // SAFETY: reads the calling thread's last-error value.
    match unsafe { GetLastError() } {
        ERROR_WINHTTP_TIMEOUT => HttpError::Timeout,
        code => HttpError::Os(code),
    }
}

fn check(ok: i32) -> Result<(), HttpError> {
    if ok == 0 {
        Err(last_error())
    } else {
        Ok(())
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct WinHttp {
    session: Handle,
}

// SAFETY: a WinHTTP session handle may be used from any thread; WinHttp owns it
// and is moved into the worker thread, never shared.
unsafe impl Send for WinHttp {}

impl WinHttp {
    pub fn new(user_agent: &str) -> Result<WinHttp, HttpError> {
        WinHttp::open(user_agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, TIMEOUT_MS)
    }

    /// Loopback tests must not depend on the machine's proxy settings.
    #[cfg(test)]
    fn for_tests(timeout_ms: i32) -> WinHttp {
        use windows_sys::Win32::Networking::WinHttp::WINHTTP_ACCESS_TYPE_NO_PROXY;
        WinHttp::open("ANVIL-test", WINHTTP_ACCESS_TYPE_NO_PROXY, timeout_ms).expect("WinHTTP session")
    }

    fn open(user_agent: &str, access: WINHTTP_ACCESS_TYPE, timeout_ms: i32) -> Result<WinHttp, HttpError> {
        let agent = wide(user_agent);
        // SAFETY: `agent` is a NUL-terminated UTF-16 string alive for the call;
        // null proxy/bypass pointers are the documented "none" values.
        let session = Handle::new(unsafe { WinHttpOpen(agent.as_ptr(), access, null(), null(), 0) })?;
        // SAFETY: valid session handle; timeouts are plain integers.
        check(unsafe { WinHttpSetTimeouts(session.0, timeout_ms, timeout_ms, timeout_ms, timeout_ms) })?;
        // The wait for the response headers has its own limit (90 s by
        // default); without it a silent server would hold the worker.
        let headers_timeout = timeout_ms as u32;
        // SAFETY: the option takes a DWORD; the pointer and size describe it.
        check(unsafe {
            WinHttpSetOption(
                session.0,
                WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT,
                (&headers_timeout as *const u32).cast(),
                4,
            )
        })?;
        let never: u32 = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        // SAFETY: the option takes a DWORD; the pointer and size describe `never`.
        check(unsafe {
            WinHttpSetOption(session.0, WINHTTP_OPTION_REDIRECT_POLICY, (&never as *const u32).cast(), 4)
        })?;
        Ok(WinHttp { session })
    }
}

impl Http for WinHttp {
    fn get(&self, endpoint: &Endpoint, query: Option<&str>, headers: &[(&str, &str)]) -> Result<Response, HttpError> {
        if !endpoint.secure && !cfg!(test) {
            return Err(HttpError::Insecure);
        }
        let target = match query {
            Some(query) if !valid_query(query) => return Err(HttpError::BadQuery),
            Some(query) => format!("{}?{query}", endpoint.path),
            None => endpoint.path.to_owned(),
        };
        let host = wide(endpoint.host);
        // SAFETY: valid session handle and NUL-terminated host string.
        let connect = Handle::new(unsafe { WinHttpConnect(self.session.0, host.as_ptr(), endpoint.port, 0) })?;
        let verb = wide("GET");
        let path = wide(&target);
        let flags = if endpoint.secure { WINHTTP_FLAG_SECURE } else { 0 };
        // SAFETY: valid connect handle; null version/referrer/accept-types are
        // the documented defaults.
        let request = Handle::new(unsafe {
            WinHttpOpenRequest(connect.0, verb.as_ptr(), path.as_ptr(), null(), null(), null(), flags)
        })?;
        let no_cookies: u32 = WINHTTP_DISABLE_COOKIES;
        // SAFETY: DWORD option on a request handle.
        check(unsafe {
            WinHttpSetOption(request.0, WINHTTP_OPTION_DISABLE_FEATURE, (&no_cookies as *const u32).cast(), 4)
        })?;
        let mut block = String::new();
        for (name, value) in headers {
            // The block is a raw header list: a value carrying CR/LF would end
            // its header and start another one, so nothing control-bearing may
            // reach it. Callers already filter their sources; this is the last
            // gate before the bytes go on the wire.
            if name.is_empty() || name.contains(':') || value.chars().any(char::is_control) {
                return Err(HttpError::Header);
            }
            block.push_str(name);
            block.push_str(": ");
            block.push_str(value);
            block.push_str("\r\n");
        }
        let block = wide(&block);
        // SAFETY: `block` is NUL-terminated (length u32::MAX = "until NUL");
        // no request body.
        check(unsafe { WinHttpSendRequest(request.0, block.as_ptr(), u32::MAX, null(), 0, 0, 0) })?;
        // SAFETY: valid request handle; the reserved pointer must be null.
        check(unsafe { WinHttpReceiveResponse(request.0, null_mut()) })?;
        let status = query_status(&request)?;
        if (300..400).contains(&status) {
            return Err(HttpError::Redirect(status));
        }
        let retry_after = query_text(&request, WINHTTP_QUERY_RETRY_AFTER);
        let body = read_body(&request)?;
        Ok(Response { status, retry_after, body })
    }
}

fn query_status(request: &Handle) -> Result<u16, HttpError> {
    let mut code: u32 = 0;
    let mut size: u32 = 4;
    // SAFETY: numeric query into a DWORD of the stated size; null name and
    // index pointers select "by info level" and "first instance".
    check(unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut code as *mut u32).cast(),
            &mut size,
            null_mut(),
        )
    })?;
    u16::try_from(code).map_err(|_| HttpError::Os(0))
}

fn query_text(request: &Handle, level: u32) -> Option<String> {
    let mut size: u32 = 0;
    // SAFETY: a null buffer asks for the required size in bytes.
    let ok = unsafe { WinHttpQueryHeaders(request.0, level, null(), null_mut(), &mut size, null_mut()) };
    // SAFETY: reads the calling thread's last-error value.
    if ok != 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || size == 0 || size > 4096 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: the buffer holds `size` bytes as the first call reported.
    let ok =
        unsafe { WinHttpQueryHeaders(request.0, level, null(), buffer.as_mut_ptr().cast(), &mut size, null_mut()) };
    if ok == 0 {
        return None;
    }
    buffer.truncate(size as usize / 2);
    String::from_utf16(&buffer).ok().map(|text| text.trim().to_owned())
}

fn read_body(request: &Handle) -> Result<Vec<u8>, HttpError> {
    let mut body = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        let mut read: u32 = 0;
        // SAFETY: `chunk` is writable for its full length.
        check(unsafe { WinHttpReadData(request.0, chunk.as_mut_ptr().cast(), chunk.len() as u32, &mut read) })?;
        if read == 0 {
            return Ok(body);
        }
        if body.len() + read as usize > BODY_LIMIT {
            return Err(HttpError::TooLarge);
        }
        body.extend_from_slice(&chunk[..read as usize]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    /// One-shot HTTP server on 127.0.0.1: answers the first request with
    /// `reply` (after `delay`) and hands the raw request text back.
    fn serve(reply: Vec<u8>, delay: Duration) -> (u16, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            let _ = tx.send(String::from_utf8_lossy(&request).into_owned());
            std::thread::sleep(delay);
            let _ = stream.write_all(&reply);
        });
        (port, rx)
    }

    fn reply(status: &str, extra_headers: &str, body: &[u8]) -> Vec<u8> {
        let mut out =
            format!("HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                .into_bytes();
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn sends_headers_and_reads_the_body() {
        let (port, request) = serve(reply("200 OK", "", b"{\"ok\":true}"), Duration::ZERO);
        let http = WinHttp::for_tests(5_000);
        let response = http
            .get(&Endpoint::loopback(port, "/quota"), None, &[("Authorization", "Bearer t0k"), ("X-Test", "1")])
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{\"ok\":true}");
        assert_eq!(response.retry_after, None);
        let request = request.recv().unwrap();
        assert!(request.starts_with("GET /quota HTTP/1.1\r\n"), "{request}");
        assert!(request.contains("Authorization: Bearer t0k\r\n"), "{request}");
        assert!(request.contains("User-Agent: ANVIL-test\r\n"), "{request}");
        assert!(!request.to_ascii_lowercase().contains("cookie"), "{request}");
    }

    #[test]
    fn redirects_are_not_followed() {
        let (port, _request) =
            serve(reply("302 Found", "Location: http://127.0.0.1:9/elsewhere\r\n", b""), Duration::ZERO);
        let result =
            WinHttp::for_tests(5_000).get(&Endpoint::loopback(port, "/"), None, &[("Authorization", "Bearer t")]);
        assert_eq!(result, Err(HttpError::Redirect(302)));
    }

    #[test]
    fn oversized_bodies_are_refused() {
        let big = vec![b'x'; BODY_LIMIT + 1];
        let (port, _request) = serve(reply("200 OK", "", &big), Duration::ZERO);
        assert_eq!(WinHttp::for_tests(5_000).get(&Endpoint::loopback(port, "/"), None, &[]), Err(HttpError::TooLarge));
    }

    #[test]
    fn retry_after_is_reported() {
        let (port, _request) =
            serve(reply("429 Too Many Requests", "Retry-After: 120\r\n", b"slow down"), Duration::ZERO);
        let response = WinHttp::for_tests(5_000).get(&Endpoint::loopback(port, "/"), None, &[]).unwrap();
        assert_eq!((response.status, response.retry_after.as_deref()), (429, Some("120")));
    }

    #[test]
    fn a_query_is_appended_to_the_fixed_path() {
        let (port, request) = serve(reply("200 OK", "", b"{}"), Duration::ZERO);
        let response =
            WinHttp::for_tests(5_000).get(&Endpoint::loopback(port, "/credits"), Some("orgId=a%2Fb&x=1"), &[]).unwrap();
        assert_eq!(response.status, 200);
        let request = request.recv().unwrap();
        assert!(request.starts_with("GET /credits?orgId=a%2Fb&x=1 HTTP/1.1\r\n"), "{request}");
    }

    #[test]
    fn queries_that_could_change_the_address_are_refused() {
        for bad in ["", "a/b", "x=1#frag", "@evil", "a?b", "a b", "x=\r\n", "host:80", &"a".repeat(513)] {
            assert!(!valid_query(bad), "{bad:?}");
            let result = WinHttp::for_tests(1_000).get(&Endpoint::loopback(1, "/"), Some(bad), &[]);
            assert_eq!(result, Err(HttpError::BadQuery), "{bad:?}");
        }
        assert!(valid_query("batch=1&input=%7B%220%22%3Anull%7D"));
    }

    /// WinHTTP checks its timers in steps of about four seconds, so the limit
    /// is asserted against a server that stays silent for much longer.
    #[test]
    fn a_silent_server_times_out() {
        let (port, _request) = serve(reply("200 OK", "", b"late"), Duration::from_secs(8));
        let started = std::time::Instant::now();
        assert_eq!(WinHttp::for_tests(1_000).get(&Endpoint::loopback(port, "/"), None, &[]), Err(HttpError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(7), "{:?}", started.elapsed());
    }
}
