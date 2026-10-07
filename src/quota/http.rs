//! HTTPS over WinHTTP for the quota worker (GET) and the Codex backend (POST
//! with a streamed answer): the system's TLS, certificate store and proxy, no
//! redirects (an `Authorization` header must never reach another host), fixed
//! timeouts and a cap on the body. Only background threads call it; nothing
//! here may run on the UI thread.

use std::ffi::c_void;
use std::io;
use std::marker::PhantomData;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER};
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    WinHttpSetTimeouts, ERROR_WINHTTP_TIMEOUT, WINHTTP_ACCESS_TYPE, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_DISABLE_COOKIES, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_DISABLE_FEATURE,
    WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT, WINHTTP_OPTION_RECEIVE_TIMEOUT, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_RETRY_AFTER,
    WINHTTP_QUERY_STATUS_CODE, WINHTTP_QUERY_STATUS_TEXT,
};

/// Larger answers are refused: every quota response is a few kilobytes.
pub const BODY_LIMIT: usize = 256 * 1024;
const TIMEOUT_MS: i32 = 10_000;
/// The body gets this many timeouts in all. WinHTTP's receive timeout restarts
/// with every read, so a peer that trickles a byte at a time would otherwise
/// hold the worker for as long as it likes inside the size cap.
const BODY_TIMEOUTS: u32 = 3;

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
    /// How long reading one answer's body may take in all.
    body_deadline: Duration,
}

// SAFETY: a WinHTTP session handle may be used from any thread; WinHttp owns it
// and is moved into the worker thread, never shared.
unsafe impl Send for WinHttp {}

impl WinHttp {
    pub fn new(user_agent: &str) -> Result<WinHttp, HttpError> {
        WinHttp::open(user_agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, TIMEOUT_MS)
    }

    /// A session for a call that may outlast the quota worker's ten seconds.
    /// Each native wait is bounded by `timeout`; POST also shortens those
    /// waits to its remaining request budget.
    pub fn with_timeout(user_agent: &str, timeout: Duration) -> Result<WinHttp, HttpError> {
        let millis = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX).max(1);
        WinHttp::open(user_agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, millis)
    }

    /// Loopback tests must not depend on the machine's proxy settings.
    #[cfg(test)]
    pub(crate) fn for_tests(timeout_ms: i32) -> WinHttp {
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
        Ok(WinHttp { session, body_deadline: Duration::from_millis(timeout_ms as u64) * BODY_TIMEOUTS })
    }
}

impl Http for WinHttp {
    fn get(&self, endpoint: &Endpoint, query: Option<&str>, headers: &[(&str, &str)]) -> Result<Response, HttpError> {
        let target = match query {
            Some(query) if !valid_query(query) => return Err(HttpError::BadQuery),
            Some(query) => format!("{}?{query}", endpoint.path),
            None => endpoint.path.to_owned(),
        };
        let exchange = self.exchange("GET", endpoint, &target, headers, &[], None)?;
        let retry_after = query_text(&exchange.request, WINHTTP_QUERY_RETRY_AFTER);
        let body = read_body(&exchange.request, self.body_deadline, self.body_deadline / BODY_TIMEOUTS)?;
        Ok(Response { status: exchange.status, retry_after, body })
    }
}

/// A request that has been answered up to the end of the response headers. The
/// request handle comes first so it is closed before the connection it uses.
struct Exchange {
    request: Handle,
    _connect: Handle,
    status: u16,
}

impl WinHttp {
    /// Sends one request and waits for the response headers. A 3xx answer is an
    /// error, never followed. `deadline` bounds the wait for the headers when
    /// the caller has an overall limit.
    fn exchange(
        &self,
        verb: &str,
        endpoint: &Endpoint,
        target: &str,
        headers: &[(&str, &str)],
        body: &[u8],
        deadline: Option<Instant>,
    ) -> Result<Exchange, HttpError> {
        if !endpoint.secure && !cfg!(test) {
            return Err(HttpError::Insecure);
        }
        let block = header_block(headers)?;
        let length = u32::try_from(body.len()).map_err(|_| HttpError::TooLarge)?;
        let host = wide(endpoint.host);
        // SAFETY: valid session handle and NUL-terminated host string.
        let connect = Handle::new(unsafe { WinHttpConnect(self.session.0, host.as_ptr(), endpoint.port, 0) })?;
        let verb = wide(verb);
        let path = wide(target);
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
        let data = if body.is_empty() { null() } else { body.as_ptr().cast() };
        // The limit is set before the request goes out: WinHTTP starts waiting
        // for the answer inside the send, with the values the handle has then.
        if let Some(deadline) = deadline {
            let timeout = remaining(deadline)?.min(self.body_deadline / BODY_TIMEOUTS);
            let millis = timeout_millis(timeout) as i32;
            // SAFETY: valid request handle and positive timeout values. Resolve,
            // connect and send are separate waits inside WinHttpSendRequest.
            check(unsafe { WinHttpSetTimeouts(request.0, millis, millis, millis, millis) })?;
            limit_wait(&request, deadline, timeout)?;
        }
        // SAFETY: `block` is NUL-terminated (length u32::MAX = "until NUL");
        // `data` is null or readable for `length` bytes. WinHTTP may use the
        // body until `WinHttpReceiveResponse` completes, and the borrow of
        // `data` spans this whole function, which makes that call.
        check(unsafe { WinHttpSendRequest(request.0, block.as_ptr(), u32::MAX, data, length, length, 0) })?;
        if let Some(deadline) = deadline {
            limit_wait(&request, deadline, self.body_deadline / BODY_TIMEOUTS)?;
        }
        // SAFETY: valid request handle; the reserved pointer must be null.
        check(unsafe { WinHttpReceiveResponse(request.0, null_mut()) })?;
        if let Some(deadline) = deadline {
            remaining(deadline)?;
        }
        let status = query_status(&request)?;
        if (300..400).contains(&status) {
            return Err(HttpError::Redirect(status));
        }
        Ok(Exchange { request, _connect: connect, status })
    }

    /// POST of `body` (the caller names its content type) whose answer is read
    /// as it arrives, for server-sent events that may run for minutes. Every
    /// wait uses the remaining `timeout`, and an answer received after it is
    /// rejected. Native synchronous timers have coarse granularity; resolve,
    /// connect and send within SendRequest each have their own bounded wait,
    /// so this is not hard wall-clock cancellation of that combined call.
    pub fn post(
        &self,
        endpoint: &Endpoint,
        headers: &[(&str, &str)],
        body: &[u8],
        timeout: Duration,
    ) -> Result<BodyStream<'_>, HttpError> {
        let deadline = Instant::now() + timeout;
        let exchange = self.exchange("POST", endpoint, endpoint.path, headers, body, Some(deadline))?;
        let status_text = query_text(&exchange.request, WINHTTP_QUERY_STATUS_TEXT).unwrap_or_default();
        Ok(BodyStream {
            exchange,
            status_text,
            deadline,
            receive_timeout: self.body_deadline / BODY_TIMEOUTS,
            finished: false,
            _session: PhantomData,
        })
    }
}

/// The answer to [`WinHttp::post`], read through `Read`: whatever has arrived
/// is returned at once, and a read fails once the request's time is used up.
/// It carries no size limit of its own; the reader applies its own.
pub struct BodyStream<'a> {
    exchange: Exchange,
    status_text: String,
    deadline: Instant,
    receive_timeout: Duration,
    finished: bool,
    /// The session must outlive the handles opened on it.
    _session: PhantomData<&'a WinHttp>,
}

impl BodyStream<'_> {
    pub fn status(&self) -> u16 {
        self.exchange.status
    }

    /// The server's reason phrase, empty when it sent none.
    pub fn status_text(&self) -> &str {
        &self.status_text
    }

    fn next(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        let request = &self.exchange.request;
        limit_wait(request, self.deadline, self.receive_timeout)?;
        // `WinHttpReadData` waits until its whole buffer is full, so ask what
        // has arrived first, as `read_body` does.
        let mut available: u32 = 0;
        // SAFETY: valid request handle; the DWORD is written by the call.
        check(unsafe { WinHttpQueryDataAvailable(request.0, &mut available) })?;
        remaining(self.deadline)?;
        if available == 0 {
            return Ok(0);
        }
        let want = (available as usize).min(buf.len());
        let mut read: u32 = 0;
        // SAFETY: `buf` is writable for `want <= buf.len()` bytes.
        check(unsafe { WinHttpReadData(request.0, buf.as_mut_ptr().cast(), want as u32, &mut read) })?;
        remaining(self.deadline)?;
        Ok(read as usize)
    }
}

impl io::Read for BodyStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.finished {
            return Ok(0);
        }
        match self.next(buf) {
            Ok(0) => {
                self.finished = true;
                Ok(0)
            }
            Ok(read) => Ok(read),
            Err(error) => {
                // A failed handle is not read again.
                self.finished = true;
                Err(match error {
                    HttpError::Timeout => io::Error::new(io::ErrorKind::TimedOut, error.to_string()),
                    error => io::Error::other(error.to_string()),
                })
            }
        }
    }
}

/// The header list as WinHTTP takes it. It is raw text: a name or value
/// carrying CR/LF would end its header and start another one, so nothing
/// control-bearing may reach it. Both halves are checked here, not just the
/// value: a name is data too the moment one comes from a config file instead of
/// a constant. Callers already filter their sources; this is the last gate
/// before the bytes go on the wire.
fn header_block(headers: &[(&str, &str)]) -> Result<Vec<u16>, HttpError> {
    let mut block = String::new();
    for (name, value) in headers {
        if name.is_empty()
            || name.contains(':')
            || name.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return Err(HttpError::Header);
        }
        block.push_str(name);
        block.push_str(": ");
        block.push_str(value);
        block.push_str("\r\n");
    }
    Ok(wide(&block))
}

/// Checks the deadline after native waits too: their timers can complete late,
/// and EOF must not turn a late response into a successful one.
fn remaining(deadline: Instant) -> Result<Duration, HttpError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(HttpError::Timeout)
    } else {
        Ok(left)
    }
}

fn timeout_millis(timeout: Duration) -> u32 {
    // Zero disables WinHTTP's timeout, so round a sub-millisecond budget up.
    u32::try_from(timeout.as_millis()).unwrap_or(i32::MAX as u32).clamp(1, i32::MAX as u32)
}

/// Shortens the next receive to the request budget without extending the
/// session's per-receive timeout.
fn limit_wait(request: &Handle, deadline: Instant, receive_timeout: Duration) -> Result<(), HttpError> {
    let millis = timeout_millis(remaining(deadline)?.min(receive_timeout));
    for option in [WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT, WINHTTP_OPTION_RECEIVE_TIMEOUT] {
        // SAFETY: DWORD option on a request handle.
        check(unsafe { WinHttpSetOption(request.0, option, (&millis as *const u32).cast(), 4) })?;
    }
    Ok(())
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

fn read_body(request: &Handle, body_timeout: Duration, receive_timeout: Duration) -> Result<Vec<u8>, HttpError> {
    let deadline = Instant::now() + body_timeout;
    let mut body = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        limit_wait(request, deadline, receive_timeout)?;
        // `WinHttpReadData` waits until its whole buffer is full, so a peer that
        // trickles bytes would hold it past any deadline; asking what has
        // arrived first keeps every wait down to one receive timeout.
        let mut available: u32 = 0;
        // SAFETY: valid request handle; the DWORD is written by the call.
        check(unsafe { WinHttpQueryDataAvailable(request.0, &mut available) })?;
        remaining(deadline)?;
        if available == 0 {
            return Ok(body);
        }
        let mut read: u32 = 0;
        let want = (available as usize).min(chunk.len());
        // SAFETY: `chunk` is writable for `want <= chunk.len()` bytes.
        check(unsafe { WinHttpReadData(request.0, chunk.as_mut_ptr().cast(), want as u32, &mut read) })?;
        remaining(deadline)?;
        if read == 0 {
            return Ok(body);
        }
        if body.len() + read as usize > BODY_LIMIT {
            return Err(HttpError::TooLarge);
        }
        body.extend_from_slice(&chunk[..read as usize]);
    }
}

/// A one-shot server on 127.0.0.1 for tests that need the request body or an
/// answer written in parts.
#[cfg(test)]
pub(crate) mod loopback {
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;

    /// The request as the server received it: line and headers, then the body.
    pub struct Seen {
        pub head: String,
        pub body: Vec<u8>,
    }

    /// Accepts one connection, reads the whole request (headers and the
    /// `Content-Length` body), reports it, then lets `answer` write the reply.
    pub fn serve(answer: impl FnOnce(&mut TcpStream) + Send + 'static) -> (u16, mpsc::Receiver<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            let head_end = loop {
                if let Some(at) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                    break at + 4;
                }
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break data.len();
                }
                data.extend_from_slice(&buf[..n]);
            };
            let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
            let length = head
                .lines()
                .find_map(|line| {
                    let line = line.to_ascii_lowercase();
                    line.strip_prefix("content-length:").and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while data.len() < head_end + length {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            let _ = tx.send(Seen { head, body: data[head_end..].to_vec() });
            answer(&mut stream);
        });
        (port, rx)
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

    /// The header block is raw bytes on the wire, so a name carrying CR/LF
    /// would end its own header and open another one. Names are constants
    /// today, but this is the gate that is supposed to hold when they are not.
    #[test]
    fn a_header_name_cannot_inject_a_second_header() {
        let (port, request) = serve(reply("200 OK", "", b"{}"), Duration::ZERO);
        let http = WinHttp::for_tests(5_000);
        for bad in ["X-A\r\nX-B", "X-A\n", "X-A\r", "X-A\x00", ""] {
            let result = http.get(&Endpoint::loopback(port, "/"), None, &[(bad, "1")]);
            assert_eq!(result, Err(HttpError::Header), "{bad:?}");
        }
        // Refused before the socket is touched: the injected header never
        // reaches the wire, not even as a request of its own.
        assert!(matches!(request.try_recv(), Err(mpsc::TryRecvError::Empty)), "a rejected header must not be sent");
    }

    /// A byte every 100 ms stays inside every per-read timeout, so only the
    /// limit on the whole body can end the answer. The server gives up after
    /// 6 s, which is what an unbounded read would end on.
    #[test]
    fn a_body_that_trickles_in_is_cut_off() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n");
            let end = Instant::now() + Duration::from_secs(6);
            while Instant::now() < end && stream.write_all(b"x").is_ok() {
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let started = Instant::now();
        let result = WinHttp::for_tests(1_000).get(&Endpoint::loopback(port, "/"), None, &[]);
        assert_eq!(result, Err(HttpError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    /// The last bytes arrive before the body budget, but the close-delimited
    /// EOF arrives after it. Completion must not bypass the deadline check.
    #[test]
    fn a_quota_body_that_ends_after_its_deadline_is_rejected() {
        let (port, _seen) = loopback::serve(|stream| {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{}");
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(250));
                if stream.write_all(b" ").is_err() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(400));
        });
        let result = WinHttp::for_tests(500).get(&Endpoint::loopback(port, "/"), None, &[]);
        assert_eq!(result, Err(HttpError::Timeout));
    }

    /// A silent server times out: WinHTTP checks its timers in steps of about
    /// four seconds, so the limit is asserted against a server that stays
    /// silent for much longer.
    #[test]
    fn a_silent_server_times_out() {
        let (port, _request) = serve(reply("200 OK", "", b"late"), Duration::from_secs(8));
        let started = std::time::Instant::now();
        assert_eq!(WinHttp::for_tests(1_000).get(&Endpoint::loopback(port, "/"), None, &[]), Err(HttpError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(7), "{:?}", started.elapsed());
    }

    const SSE_HEAD: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";

    /// The server holds the second part back until the client reports it has
    /// read the first, so an answer that is only handed over at its end would
    /// never get that far.
    #[test]
    fn a_post_carries_its_body_and_its_answer_is_read_as_it_arrives() {
        let (release, released) = mpsc::channel::<()>();
        let (port, seen) = loopback::serve(move |stream| {
            let _ = stream.write_all(SSE_HEAD);
            let _ = stream.write_all(b"5\r\nfirst\r\n");
            if released.recv_timeout(Duration::from_secs(5)).is_ok() {
                let _ = stream.write_all(b"6\r\nsecond\r\n0\r\n\r\n");
            }
        });
        let http = WinHttp::for_tests(5_000);
        let mut stream = http
            .post(
                &Endpoint::loopback(port, "/responses"),
                &[("Authorization", "Bearer t0k"), ("Content-Type", "application/json")],
                b"{\"a\":1}",
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!((stream.status(), stream.status_text()), (200, "OK"));
        let mut buf = [0u8; 64];
        let n = stream.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"first");
        release.send(()).unwrap();
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, b"second");
        let seen = seen.recv().unwrap();
        assert!(seen.head.starts_with("POST /responses HTTP/1.1\r\n"), "{}", seen.head);
        assert!(seen.head.contains("Authorization: Bearer t0k\r\n"), "{}", seen.head);
        assert!(seen.head.contains("Content-Type: application/json\r\n"), "{}", seen.head);
        assert!(seen.head.contains("Content-Length: 7\r\n"), "{}", seen.head);
        assert_eq!(seen.body, b"{\"a\":1}");
    }

    /// A 307 keeps the method and the body, so following it would hand the
    /// credentials to whoever the `Location` names.
    #[test]
    fn a_post_is_not_redirected_with_its_credentials() {
        let elsewhere = TcpListener::bind("127.0.0.1:0").unwrap();
        elsewhere.set_nonblocking(true).unwrap();
        let elsewhere_port = elsewhere.local_addr().unwrap().port();
        let location = format!("Location: http://127.0.0.1:{elsewhere_port}/steal\r\n");
        let (port, _seen) = loopback::serve(move |stream| {
            let _ = stream.write_all(&reply("307 Temporary Redirect", &location, b""));
        });
        let http = WinHttp::for_tests(5_000);
        let result = http.post(
            &Endpoint::loopback(port, "/"),
            &[("Authorization", "Bearer secret-token")],
            b"{}",
            Duration::from_secs(5),
        );
        assert_eq!(result.err(), Some(HttpError::Redirect(307)));
        std::thread::sleep(Duration::from_millis(300));
        assert!(matches!(elsewhere.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    }

    #[test]
    fn a_post_header_cannot_inject_a_second_header() {
        let (port, seen) = loopback::serve(|_| {});
        let http = WinHttp::for_tests(5_000);
        for (name, value) in [("X-A\r\nX-B", "1"), ("X-A", "1\r\nX-B: 2"), ("X-A", "1\n"), ("X-A", "a\0b"), ("", "1")] {
            let result = http.post(&Endpoint::loopback(port, "/"), &[(name, value)], b"{}", Duration::from_secs(5));
            assert_eq!(result.err(), Some(HttpError::Header), "{name:?}: {value:?}");
        }
        assert!(matches!(seen.try_recv(), Err(mpsc::TryRecvError::Empty)), "a rejected header must not be sent");
    }

    /// The session's own waits are far longer than the request's: only the
    /// deadline given to `post` can end this.
    #[test]
    fn a_post_answer_that_stalls_is_cut_off_by_the_request_deadline() {
        let (port, _seen) = loopback::serve(|stream| {
            let _ = stream.write_all(SSE_HEAD);
            let _ = stream.write_all(b"5\r\nfirst\r\n");
            std::thread::sleep(Duration::from_secs(8));
        });
        let started = Instant::now();
        let http = WinHttp::for_tests(30_000);
        let mut stream = http.post(&Endpoint::loopback(port, "/"), &[], b"{}", Duration::from_secs(1)).unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(stream.read(&mut buf).unwrap(), 5);
        let error = stream.read(&mut buf).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut, "{error}");
        assert!(started.elapsed() < Duration::from_secs(7), "{:?}", started.elapsed());
    }

    /// Bytes that keep arriving inside each read's wait must still end at the
    /// deadline of the whole request, not be granted a fresh wait every time.
    #[test]
    fn a_post_answer_that_trickles_is_cut_off_by_the_request_deadline() {
        let (port, _seen) = loopback::serve(|stream| {
            let _ = stream.write_all(SSE_HEAD);
            for _ in 0..30 {
                if stream.write_all(b"1\r\nx\r\n").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        });
        let started = Instant::now();
        let http = WinHttp::for_tests(30_000);
        let mut stream = http.post(&Endpoint::loopback(port, "/"), &[], b"{}", Duration::from_secs(1)).unwrap();
        let mut buf = [0u8; 16];
        let error = loop {
            match stream.read(&mut buf) {
                Ok(0) => panic!("the answer ended before the deadline"),
                Ok(_) => {}
                Err(error) => break error,
            }
        };
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut, "{error}");
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    }

    /// Same limit before any header has arrived.
    #[test]
    fn a_post_to_a_silent_server_is_cut_off_by_the_request_deadline() {
        let (port, _seen) = loopback::serve(|_| std::thread::sleep(Duration::from_secs(8)));
        let started = Instant::now();
        let http = WinHttp::for_tests(30_000);
        let result = http.post(&Endpoint::loopback(port, "/"), &[], b"{}", Duration::from_secs(1));
        assert_eq!(result.err(), Some(HttpError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(7), "{:?}", started.elapsed());
    }

    /// WinHTTP's coarse timer can allow headers to arrive after a short
    /// timeout. A successful native call still has to satisfy our deadline.
    #[test]
    fn post_headers_received_after_the_deadline_are_rejected() {
        let (port, _seen) = loopback::serve(|stream| {
            std::thread::sleep(Duration::from_millis(1_400));
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        });
        let http = WinHttp::for_tests(30_000);
        let result = http.post(&Endpoint::loopback(port, "/"), &[], b"{}", Duration::from_secs(1));
        assert_eq!(result.err(), Some(HttpError::Timeout));
    }

    #[test]
    fn a_post_stream_that_ends_after_the_deadline_is_rejected() {
        let (port, _seen) = loopback::serve(|stream| {
            let _ = stream.write_all(SSE_HEAD);
            let _ = stream.write_all(b"5\r\nfirst\r\n");
            std::thread::sleep(Duration::from_millis(1_400));
            let _ = stream.write_all(b"0\r\n\r\n");
        });
        let http = WinHttp::for_tests(30_000);
        let mut stream = http.post(&Endpoint::loopback(port, "/"), &[], b"{}", Duration::from_secs(1)).unwrap();
        let mut body = Vec::new();
        let error = stream.read_to_end(&mut body).unwrap_err();
        assert_eq!(body, b"first");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
