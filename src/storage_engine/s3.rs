//! An S3 client with no I/O in it, and an S3 to test it against.
//!
//! [`S3ObjectStore`] is an [`ObjectStore`] over S3 and anything that
//! speaks its protocol. It builds requests, signs them, reads responses
//! and decides what to do when one does not come. It does not open a
//! socket: a request goes out through a [`Transport`], which is given the
//! whole request and gives back the whole response, or says that none
//! came.
//!
//! That split is for testing. An SDK that owns its own threads, sockets
//! and retries cannot be run inside a simulation, and what goes wrong
//! with object storage goes wrong in exactly those parts: a request that
//! was applied and whose answer was lost, an answer cut short, a retry
//! that arrives after the first attempt took effect. [`SimulatedS3`] is a
//! transport that is an S3 in memory and does those things on purpose,
//! from a seed. The crash harness runs the store through this client
//! against it.
//!
//! What a put needs to survive is an answer that never came. A put that
//! must happen only if the object is as last seen cannot simply be tried
//! again: if the first attempt took effect, the second is refused, and
//! the caller would conclude that someone else had written the object.
//! So every put carries a token of its own, kept with the object. After a
//! lost answer, a refusal is checked against the object: if it holds this
//! put's token, the put happened.
//!
//! An object's mark is its ETag, which is made from its bytes. Two puts
//! of the same bytes have the same mark. Nothing here depends on telling
//! those apart: a part is the same part, and a pointer names its writer
//! and its publication, which only ever go up.

use super::object_store::ObjectStore;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// One request, whole. The path and the query are as meant, not yet
/// encoded for the wire: [`Request::target`] gives them encoded.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: &'static str,
    pub host: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The path and query as they go on the request line.
    pub fn target(&self) -> String {
        let query = canonical_query(&self.query);
        match query.is_empty() {
            true => encoded(&self.path, false),
            false => format!("{}?{query}", encoded(&self.path, false)),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    fn param(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(other, _)| other == name).map(|(_, value)| value.as_str())
    }
}

/// One response, whole.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(other, _)| other.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
}

/// What carries a request to S3 and brings the response back.
///
/// An error means no response came. The request may have been applied
/// all the same.
pub trait Transport: Send + Sync {
    fn send(&self, request: &Request) -> io::Result<Response>;
}

/// Where the objects are and who is asking.
#[derive(Debug, Clone)]
pub struct S3Config {
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// The host to send to, with its port if it has one: the bucket's own
    /// host, or with `path_style` the service's.
    pub endpoint: String,
    /// The bucket is the first part of the path and not part of the host.
    /// S3-compatible servers are usually reached this way.
    pub path_style: bool,
    /// What every key begins with in the bucket, so that one bucket can
    /// hold more than this store's objects.
    pub prefix: String,
    /// An object longer than this is put in parts of this size, so that
    /// no more than this is held in memory. S3 takes no part smaller than
    /// 5 MiB but the last.
    pub part_bytes: usize,
    /// How many times a request is sent before giving up.
    pub attempts: u32,
}

impl S3Config {
    /// A bucket reached at its own host on AWS.
    pub fn aws(bucket: &str, region: &str, access_key: &str, secret_key: &str) -> Self {
        Self {
            bucket: bucket.to_string(),
            region: region.to_string(),
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            endpoint: format!("{bucket}.s3.{region}.amazonaws.com"),
            path_style: false,
            prefix: String::new(),
            part_bytes: 8 << 20,
            attempts: 6,
        }
    }
}

/// The metadata a put's token is kept in.
const TOKEN: &str = "x-amz-meta-owl-put";

/// An object store that is an S3 bucket.
pub struct S3ObjectStore {
    config: S3Config,
    transport: Arc<dyn Transport>,
    /// The time, in seconds since 1970, which a request is signed with.
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// What this store's put tokens begin with: different for every store
    /// ever made, so that no two puts anywhere share a token.
    token: String,
    next_put: AtomicU64,
}

/// What a put may depend on.
enum Condition<'a> {
    None,
    Absent,
    Is(&'a str),
}

/// What is known of an object without reading it.
struct Head {
    len: u64,
    mark: String,
    token: Option<String>,
}

fn failed(what: &str, response: &Response) -> io::Error {
    let body = String::from_utf8_lossy(&response.body);
    let reason = text_of(&body, "Message").or_else(|| text_of(&body, "Code")).unwrap_or_default();
    io::Error::other(format!("S3 would not {what}: status {} {reason}", response.status))
}

impl S3ObjectStore {
    /// A store over `transport`, signing with the time `clock` gives and
    /// naming its puts from `seed`, which must differ between any two
    /// stores that write the same bucket.
    pub fn new(
        config: S3Config,
        transport: Arc<dyn Transport>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
        seed: u128,
    ) -> Self {
        Self {
            config,
            transport,
            clock,
            token: format!("{seed:032x}"),
            next_put: AtomicU64::new(0),
        }
    }

    /// A store that signs with the machine's clock and names its puts at
    /// random.
    pub fn over(config: S3Config, transport: Arc<dyn Transport>) -> Self {
        let clock = || {
            web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |since| since.as_secs())
        };
        Self::new(config, transport, Arc::new(clock), rand::random())
    }

    fn request(&self, method: &'static str, key: Option<&str>) -> Request {
        let key = key.map(|key| format!("{}{key}", self.config.prefix));
        let path = match (self.config.path_style, key) {
            (true, Some(key)) => format!("/{}/{key}", self.config.bucket),
            (true, None) => format!("/{}", self.config.bucket),
            (false, Some(key)) => format!("/{key}"),
            (false, None) => "/".to_string(),
        };
        Request {
            method,
            host: self.config.endpoint.clone(),
            path,
            query: Vec::new(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Send a request until an answer comes that is an answer: not a
    /// failure of the service, and whole. Says also whether an attempt
    /// before it went unanswered, and so may have been applied.
    fn send(&self, request: &Request) -> io::Result<(Response, bool)> {
        let mut unanswered = false;
        let mut last = String::from("no answer");
        for _ in 0..self.config.attempts.max(1) {
            let mut signed = request.clone();
            sign(&mut signed, &self.config, &amz_date((self.clock)()));
            let response = match self.transport.send(&signed) {
                Ok(response) => response,
                Err(error) => {
                    unanswered = true;
                    last = error.to_string();
                    continue;
                }
            };
            // The service failed, or two conditional writes met. Either
            // may have been applied; both are tried again.
            if matches!(response.status, 409 | 429 | 500 | 502 | 503 | 504) {
                unanswered = true;
                last = format!("status {}", response.status);
                continue;
            }
            // S3 can answer 200 and put the failure in the body.
            if response.status == 200 && response.body.starts_with(b"<?xml") {
                let body = String::from_utf8_lossy(&response.body);
                if body.contains("<Error>") {
                    unanswered = true;
                    last = text_of(&body, "Code").unwrap_or_default();
                    continue;
                }
            }
            // An answer shorter than it says it is was cut off.
            if request.method == "GET" {
                let whole = response
                    .header("content-length")
                    .and_then(|len| len.parse::<usize>().ok())
                    .is_none_or(|len| len == response.body.len());
                if !whole {
                    last = "an answer cut short".to_string();
                    continue;
                }
            }
            return Ok((response, unanswered));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("S3 did not answer {} {} ({last})", request.method, request.path),
        ))
    }

    fn head(&self, key: &str) -> io::Result<Option<Head>> {
        let (response, _) = self.send(&self.request("HEAD", Some(key)))?;
        match response.status {
            200 => Ok(Some(Head {
                len: response.header("content-length").and_then(|len| len.parse().ok()).unwrap_or(0),
                mark: response.header("etag").unwrap_or_default().to_string(),
                token: response.header(TOKEN).map(str::to_string),
            })),
            404 => Ok(None),
            _ => Err(failed("describe an object", &response)),
        }
    }

    /// After an answer was lost: whether the object is the one this put
    /// was making, and if so its mark.
    fn put_happened(&self, key: &str, token: &str) -> io::Result<Option<String>> {
        Ok(self.head(key)?.filter(|head| head.token.as_deref() == Some(token)).map(|head| head.mark))
    }

    fn condition(request: &mut Request, condition: &Condition) {
        match condition {
            Condition::None => {}
            Condition::Absent => request.headers.push(("if-none-match".into(), "*".into())),
            Condition::Is(mark) => request.headers.push(("if-match".into(), mark.to_string())),
        }
    }

    /// Put an object. Gives its mark, or `None` when the condition did
    /// not hold.
    fn put_object(
        &self,
        key: &str,
        source: &mut dyn Read,
        len: u64,
        condition: Condition,
    ) -> io::Result<Option<String>> {
        let token = format!("{}-{}", self.token, self.next_put.fetch_add(1, Ordering::Relaxed));
        if len as usize > self.config.part_bytes {
            return self.put_in_parts(key, source, len, condition, &token);
        }
        let mut body = Vec::with_capacity(len as usize);
        source.take(len).read_to_end(&mut body)?;
        if body.len() as u64 != len {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a put was given too few bytes"));
        }
        let mut request = self.request("PUT", Some(key));
        request.headers.push((TOKEN.into(), token.clone()));
        Self::condition(&mut request, &condition);
        request.body = body;
        let (response, unanswered) = self.send(&request)?;
        self.put_outcome(key, &token, &condition, response, unanswered, |response| {
            response.header("etag").map(str::to_string)
        })
    }

    /// What a put's answer means.
    fn put_outcome(
        &self,
        key: &str,
        token: &str,
        condition: &Condition,
        response: Response,
        unanswered: bool,
        mark: impl Fn(&Response) -> Option<String>,
    ) -> io::Result<Option<String>> {
        match response.status {
            200 => match mark(&response) {
                Some(mark) => Ok(Some(mark)),
                // The put happened; only its mark is missing.
                None => Ok(self.head(key)?.map(|head| head.mark)),
            },
            // Refused, or the thing it was to change is gone. If an
            // earlier attempt went unanswered, that attempt may be why.
            404 | 412 if unanswered => self.put_happened(key, token),
            412 => Ok(None),
            404 if !matches!(condition, Condition::None) => Ok(None),
            _ => Err(failed("put an object", &response)),
        }
    }

    /// Put an object too long to hold in memory, a part at a time. It
    /// becomes the object only when the last request completes it, and
    /// the condition is on that request.
    fn put_in_parts(
        &self,
        key: &str,
        source: &mut dyn Read,
        len: u64,
        condition: Condition,
        token: &str,
    ) -> io::Result<Option<String>> {
        let mut begin = self.request("POST", Some(key));
        begin.query.push(("uploads".into(), String::new()));
        begin.headers.push((TOKEN.into(), token.to_string()));
        let (response, _) = self.send(&begin)?;
        if response.status != 200 {
            return Err(failed("begin a put in parts", &response));
        }
        let upload = text_of(&String::from_utf8_lossy(&response.body), "UploadId")
            .ok_or_else(|| io::Error::other("S3 began a put in parts and did not name it"))?;

        let outcome = (|| {
            let mut listed = String::from("<CompleteMultipartUpload>");
            let mut left = len;
            let mut number = 0u32;
            while left > 0 {
                number += 1;
                let size = left.min(self.config.part_bytes as u64);
                let mut body = Vec::with_capacity(size as usize);
                source.take(size).read_to_end(&mut body)?;
                if body.len() as u64 != size {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a put was given too few bytes"));
                }
                let mut part = self.request("PUT", Some(key));
                part.query.push(("partNumber".into(), number.to_string()));
                part.query.push(("uploadId".into(), upload.clone()));
                part.body = body;
                // The same part sent twice is the same part.
                let (response, _) = self.send(&part)?;
                if response.status != 200 {
                    return Err(failed("put a part", &response));
                }
                let mark = response.header("etag").unwrap_or_default();
                listed.push_str(&format!("<Part><PartNumber>{number}</PartNumber><ETag>{mark}</ETag></Part>"));
                left -= size;
            }
            listed.push_str("</CompleteMultipartUpload>");

            let mut complete = self.request("POST", Some(key));
            complete.query.push(("uploadId".into(), upload.clone()));
            Self::condition(&mut complete, &condition);
            complete.body = listed.into_bytes();
            let (response, unanswered) = self.send(&complete)?;
            self.put_outcome(key, token, &condition, response, unanswered, |response| {
                text_of(&String::from_utf8_lossy(&response.body), "ETag")
            })
        })();
        if !matches!(outcome, Ok(Some(_))) {
            // The parts are of no use. Left alone they would be kept, and
            // paid for, until someone noticed.
            let mut abort = self.request("DELETE", Some(key));
            abort.query.push(("uploadId".into(), upload));
            let _ = self.send(&abort);
        }
        outcome
    }
}

impl ObjectStore for S3ObjectStore {
    fn put(&self, key: &str, source: &mut dyn Read, len: u64) -> io::Result<()> {
        self.put_object(key, source, len, Condition::None).map(|_| ())
    }

    fn read_at(&self, key: &str, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut request = self.request("GET", Some(key));
        request.headers.push(("range".into(), format!("bytes={offset}-{}", offset + buffer.len() as u64 - 1)));
        let (response, _) = self.send(&request)?;
        match response.status {
            200 | 206 => {
                let count = buffer.len().min(response.body.len());
                buffer[..count].copy_from_slice(&response.body[..count]);
                Ok(count)
            }
            404 => Err(io::Error::new(io::ErrorKind::NotFound, format!("no object named '{key}'"))),
            // The range begins at or past the object's end.
            416 => Ok(0),
            _ => Err(failed("read an object", &response)),
        }
    }

    fn len(&self, key: &str) -> io::Result<Option<u64>> {
        Ok(self.head(key)?.map(|head| head.len))
    }

    fn delete(&self, key: &str) -> io::Result<()> {
        let (response, _) = self.send(&self.request("DELETE", Some(key)))?;
        match response.status {
            200 | 204 | 404 => Ok(()),
            _ => Err(failed("delete an object", &response)),
        }
    }

    fn list(&self, prefix: &str) -> io::Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut request = self.request("GET", None);
            request.query.push(("list-type".into(), "2".into()));
            request.query.push(("prefix".into(), format!("{}{prefix}", self.config.prefix)));
            if let Some(next) = &next {
                request.query.push(("continuation-token".into(), next.clone()));
            }
            let (response, _) = self.send(&request)?;
            if response.status != 200 {
                return Err(failed("list objects", &response));
            }
            let body = String::from_utf8_lossy(&response.body).into_owned();
            for key in texts_of(&body, "Key") {
                if let Some(key) = key.strip_prefix(&self.config.prefix) {
                    keys.push(key.to_string());
                }
            }
            next = match text_of(&body, "IsTruncated").as_deref() {
                Some("true") => text_of(&body, "NextContinuationToken"),
                _ => None,
            };
            if next.is_none() {
                break;
            }
        }
        keys.sort();
        Ok(keys)
    }

    fn version(&self, key: &str) -> io::Result<Option<String>> {
        Ok(self.head(key)?.map(|head| head.mark))
    }

    fn put_if(
        &self,
        key: &str,
        source: &mut dyn Read,
        len: u64,
        expected: Option<&str>,
    ) -> io::Result<Option<String>> {
        let condition = match expected {
            None => Condition::Absent,
            Some(mark) => Condition::Is(mark),
        };
        self.put_object(key, source, len, condition)
    }

    /// Not every server that speaks S3 honours a condition on a delete:
    /// some remove the object whatever it is. So the object is looked at
    /// first, and the delete is only sent if it is the one expected. Where
    /// the condition is honoured the two are one step; where it is not,
    /// an object put between the look and the delete would be removed.
    /// Nothing in the store depends on that gap being closed: a writer
    /// deletes only objects under names that are its own line's.
    fn delete_if(&self, key: &str, expected: &str) -> io::Result<bool> {
        if self.head(key)?.is_none_or(|head| head.mark != expected) {
            return Ok(false);
        }
        let mut request = self.request("DELETE", Some(key));
        request.headers.push(("if-match".into(), expected.to_string()));
        let (response, unanswered) = self.send(&request)?;
        match response.status {
            200 | 204 => Ok(true),
            // Gone, after an attempt that went unanswered: that attempt
            // removed it.
            404 if unanswered => Ok(true),
            404 | 412 => Ok(false),
            _ => Err(failed("delete an object", &response)),
        }
    }
}

/// The transport that opens a socket: HTTP/1.1, over TLS or not.
///
/// It does nothing clever. It sends the request as it was built, and
/// gives back whatever status came with its headers and body. It does not
/// follow redirects, decode bodies, or try again: the client above
/// decides those. A response that does not arrive whole is no response.
#[cfg(feature = "s3")]
pub struct HttpTransport {
    agent: ureq::Agent,
    scheme: &'static str,
}

#[cfg(feature = "s3")]
impl HttpTransport {
    /// Over TLS when `secure`. `timeout` is for a whole request, from
    /// connecting to the last byte of the answer.
    pub fn new(secure: bool, timeout: std::time::Duration) -> Self {
        Self {
            agent: ureq::AgentBuilder::new().timeout(timeout).redirects(0).build(),
            scheme: if secure { "https" } else { "http" },
        }
    }
}

#[cfg(feature = "s3")]
impl Transport for HttpTransport {
    fn send(&self, request: &Request) -> io::Result<Response> {
        let url = format!("{}://{}{}", self.scheme, request.host, request.target());
        let mut call = self.agent.request(request.method, &url);
        for (name, value) in &request.headers {
            // The host and the length are the transport's to write, from
            // the same URL and body.
            if !name.eq_ignore_ascii_case("host") && !name.eq_ignore_ascii_case("content-length") {
                call = call.set(name, value);
            }
        }
        let sent = match request.method {
            "PUT" | "POST" => call.send_bytes(&request.body),
            _ => call.call(),
        };
        let response = match sent {
            // A status that is a refusal is still an answer.
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(error)) => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, format!("no answer came: {error}")));
            }
        };
        let status = response.status();
        let headers: Vec<(String, String)> = response
            .headers_names()
            .into_iter()
            .filter_map(|name| response.header(&name).map(|value| (name.clone(), value.to_string())))
            .collect();
        let mut body = Vec::new();
        if request.method != "HEAD" {
            response.into_reader().read_to_end(&mut body)?;
        }
        Ok(Response { status, headers, body })
    }
}

// Signing: AWS Signature Version 4.

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    match key.len() > 64 {
        true => block[..32].copy_from_slice(&sha256(key)),
        false => block[..key.len()].copy_from_slice(key),
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Percent-encode as S3 signs: everything but letters, digits and
/// `-._~`, and `/` too unless it separates the parts of a path.
fn encoded(text: &str, slashes: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let plain = byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || (byte == b'/' && !slashes);
        match plain {
            true => out.push(byte as char),
            false => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn canonical_query(query: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> =
        query.iter().map(|(name, value)| (encoded(name, true), encoded(value, true))).collect();
    pairs.sort();
    pairs.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("&")
}

/// Seconds since 1970 as `YYYYMMDDTHHMMSSZ`.
fn amz_date(seconds: u64) -> String {
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    // Days to a date in the calendar that has always had leap years as
    // they are now, counted from the first of March of year zero.
    let shifted = days as i64 + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        of_day / 3_600,
        of_day % 3_600 / 60,
        of_day % 60
    )
}

/// Add the headers that sign a request as made at `at`.
fn sign(request: &mut Request, config: &S3Config, at: &str) {
    let payload = hex(&sha256(&request.body));
    request.headers.retain(|(name, _)| {
        !["host", "x-amz-date", "x-amz-content-sha256", "authorization"].contains(&name.to_ascii_lowercase().as_str())
    });
    request.headers.push(("host".into(), request.host.clone()));
    request.headers.push(("x-amz-content-sha256".into(), payload.clone()));
    request.headers.push(("x-amz-date".into(), at.to_string()));

    let mut headers: Vec<(String, String)> = request
        .headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.split_whitespace().collect::<Vec<_>>().join(" ")))
        .collect();
    headers.sort();
    let signed: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
    let signed = signed.join(";");
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{signed}\n{payload}",
        request.method,
        encoded(&request.path, false),
        canonical_query(&request.query),
        headers.iter().map(|(name, value)| format!("{name}:{value}\n")).collect::<String>(),
    );
    let date = &at[..8];
    let scope = format!("{date}/{}/s3/aws4_request", config.region);
    let to_sign = format!("AWS4-HMAC-SHA256\n{at}\n{scope}\n{}", hex(&sha256(canonical.as_bytes())));
    let mut key = hmac(format!("AWS4{}", config.secret_key).as_bytes(), date.as_bytes());
    for part in [config.region.as_str(), "s3", "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    request.headers.push((
        "authorization".into(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed},Signature={signature}",
            config.access_key
        ),
    ));
}

// The little XML that S3 answers in.

/// Text as it was before it was put in XML: the named entities, and the
/// numbered ones, which some servers use for a quotation mark.
fn unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let character = match entity {
            "quot" => Some('"'),
            "apos" => Some('\''),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            _ => entity
                .strip_prefix('#')
                .and_then(|number| match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.parse().ok(),
                })
                .and_then(char::from_u32),
        };
        match character {
            Some(character) => {
                out.push(character);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// What is between each `<tag>` and its `</tag>`.
fn texts_of(xml: &str, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find(&open) {
        rest = &rest[at + open.len()..];
        let Some(end) = rest.find(&close) else { break };
        found.push(unescaped(&rest[..end]));
        rest = &rest[end + close.len()..];
    }
    found
}

fn text_of(xml: &str, tag: &str) -> Option<String> {
    texts_of(xml, tag).into_iter().next()
}

/// What an S3 in a simulation does wrong, each as how often in a request.
#[derive(Debug, Clone, Copy, Default)]
pub struct S3Faults {
    /// The request is lost: nothing is applied and no answer comes.
    pub lost_request: f64,
    /// The service fails: nothing is applied and it says so.
    pub error: f64,
    /// The request is applied and its answer is lost.
    pub lost_answer: f64,
    /// A read's answer is cut short.
    pub cut_short: f64,
}

struct Kept {
    bytes: Arc<Vec<u8>>,
    mark: String,
    token: Option<String>,
}

struct Upload {
    key: String,
    token: Option<String>,
    parts: BTreeMap<u32, Vec<u8>>,
}

struct Simulated {
    objects: BTreeMap<String, Kept>,
    uploads: HashMap<String, Upload>,
    next_upload: u64,
    rng: StdRng,
    faults: S3Faults,
}

/// An S3 in memory, for tests: a [`Transport`] that answers as S3 does
/// for the requests [`S3ObjectStore`] makes, and that loses requests and
/// answers, fails, and cuts answers short as often as it is told to, the
/// same way every time for the same seed.
pub struct SimulatedS3 {
    bucket: String,
    path_style: bool,
    /// How many keys a listing gives at a time.
    page: usize,
    /// Deletes whatever is there, whatever the request said it must be,
    /// as some servers do.
    deletes_regardless: bool,
    state: Mutex<Simulated>,
    requests: AtomicU64,
    faulted: AtomicU64,
}

fn mark_of(bytes: &[u8]) -> String {
    format!("\"{}\"", &hex(&sha256(bytes))[..32])
}

fn answer(status: u16, headers: &[(&str, String)], body: Vec<u8>) -> Response {
    let mut headers: Vec<(String, String)> =
        headers.iter().map(|(name, value)| (name.to_string(), value.clone())).collect();
    if !headers.iter().any(|(name, _)| name == "content-length") {
        headers.push(("content-length".into(), body.len().to_string()));
    }
    Response { status, headers, body }
}

fn error(status: u16, code: &str) -> Response {
    let body = format!("<?xml version=\"1.0\"?><Error><Code>{code}</Code><Message>{code}</Message></Error>");
    answer(status, &[], body.into_bytes())
}

impl SimulatedS3 {
    pub fn new(bucket: &str, path_style: bool, seed: u64) -> Self {
        Self {
            bucket: bucket.to_string(),
            path_style,
            page: 1000,
            deletes_regardless: false,
            state: Mutex::new(Simulated {
                objects: BTreeMap::new(),
                uploads: HashMap::new(),
                next_upload: 0,
                rng: StdRng::seed_from_u64(seed),
                faults: S3Faults::default(),
            }),
            requests: AtomicU64::new(0),
            faulted: AtomicU64::new(0),
        }
    }

    /// Give listings this many keys at a time.
    pub fn with_page(mut self, page: usize) -> Self {
        self.page = page.max(1);
        self
    }

    /// Ignore the condition on a delete, as some servers that speak S3 do.
    pub fn deleting_regardless(mut self) -> Self {
        self.deletes_regardless = true;
        self
    }

    pub fn set_faults(&self, faults: S3Faults) {
        self.state.lock().unwrap().faults = faults;
    }

    /// How many requests have arrived, and how many of them went wrong
    /// on purpose.
    pub fn requests(&self) -> (u64, u64) {
        (self.requests.load(Ordering::Relaxed), self.faulted.load(Ordering::Relaxed))
    }

    /// The keys held, in order.
    pub fn keys(&self) -> Vec<String> {
        self.state.lock().unwrap().objects.keys().cloned().collect()
    }

    /// How many puts in parts have been begun and neither completed nor
    /// abandoned.
    pub fn unfinished_uploads(&self) -> usize {
        self.state.lock().unwrap().uploads.len()
    }

    fn key_of(&self, path: &str) -> Option<String> {
        let rest = path.strip_prefix('/')?;
        match self.path_style {
            false => Some(rest.to_string()),
            true => {
                let rest = rest.strip_prefix(&self.bucket)?;
                Some(rest.strip_prefix('/').unwrap_or(rest).to_string())
            }
        }
    }

    /// Whether a write's condition holds for what is under the key now.
    fn refused(request: &Request, held: Option<&Kept>) -> Option<Response> {
        if request.header("if-none-match") == Some("*") && held.is_some() {
            return Some(error(412, "PreconditionFailed"));
        }
        if let Some(mark) = request.header("if-match") {
            return match held {
                None => Some(error(404, "NoSuchKey")),
                Some(held) if held.mark != mark => Some(error(412, "PreconditionFailed")),
                Some(_) => None,
            };
        }
        None
    }

    fn apply(&self, state: &mut Simulated, request: &Request) -> Response {
        let Some(key) = self.key_of(&request.path) else {
            return error(404, "NoSuchBucket");
        };
        match request.method {
            "GET" if request.param("list-type").is_some() => {
                let prefix = request.param("prefix").unwrap_or_default();
                let after = request.param("continuation-token").unwrap_or_default();
                let mut matching = state
                    .objects
                    .keys()
                    .filter(|key| key.starts_with(prefix) && key.as_str() > after)
                    .cloned();
                let keys: Vec<String> = matching.by_ref().take(self.page).collect();
                let more = matching.next().is_some();
                let mut body = String::from("<?xml version=\"1.0\"?><ListBucketResult>");
                body.push_str(&format!("<IsTruncated>{more}</IsTruncated>"));
                if let (true, Some(last)) = (more, keys.last()) {
                    body.push_str(&format!("<NextContinuationToken>{}</NextContinuationToken>", escaped(last)));
                }
                for key in &keys {
                    body.push_str(&format!("<Contents><Key>{}</Key></Contents>", escaped(key)));
                }
                body.push_str("</ListBucketResult>");
                answer(200, &[], body.into_bytes())
            }
            "GET" | "HEAD" => {
                let Some(held) = state.objects.get(&key) else {
                    return error(404, "NoSuchKey");
                };
                let mut headers = vec![("etag", held.mark.clone())];
                if let Some(token) = &held.token {
                    headers.push((TOKEN, token.clone()));
                }
                if request.method == "HEAD" {
                    headers.push(("content-length", held.bytes.len().to_string()));
                    return answer(200, &headers, Vec::new());
                }
                let range = request.header("range").and_then(|range| {
                    let (from, to) = range.strip_prefix("bytes=")?.split_once('-')?;
                    Some((from.parse::<usize>().ok()?, to.parse::<usize>().ok()?))
                });
                match range {
                    None => answer(200, &headers, held.bytes.to_vec()),
                    Some((from, _)) if from >= held.bytes.len() => error(416, "InvalidRange"),
                    Some((from, to)) => {
                        let to = to.min(held.bytes.len() - 1);
                        answer(206, &headers, held.bytes[from..=to].to_vec())
                    }
                }
            }
            "PUT" if request.param("uploadId").is_some() => {
                let number = request.param("partNumber").and_then(|number| number.parse::<u32>().ok());
                let upload = request.param("uploadId").and_then(|upload| state.uploads.get_mut(upload));
                match (upload, number) {
                    (Some(upload), Some(number)) => {
                        let mark = mark_of(&request.body);
                        upload.parts.insert(number, request.body.clone());
                        answer(200, &[("etag", mark)], Vec::new())
                    }
                    _ => error(404, "NoSuchUpload"),
                }
            }
            "PUT" => {
                if let Some(refused) = Self::refused(request, state.objects.get(&key)) {
                    return refused;
                }
                let mark = mark_of(&request.body);
                state.objects.insert(
                    key,
                    Kept {
                        bytes: Arc::new(request.body.clone()),
                        mark: mark.clone(),
                        token: request.header(TOKEN).map(str::to_string),
                    },
                );
                answer(200, &[("etag", mark)], Vec::new())
            }
            "POST" if request.param("uploads").is_some() => {
                state.next_upload += 1;
                let upload = format!("upload-{}", state.next_upload);
                state.uploads.insert(
                    upload.clone(),
                    Upload {
                        key,
                        token: request.header(TOKEN).map(str::to_string),
                        parts: BTreeMap::new(),
                    },
                );
                let body = format!("<?xml version=\"1.0\"?><InitiateMultipartUploadResult><UploadId>{upload}</UploadId></InitiateMultipartUploadResult>");
                answer(200, &[], body.into_bytes())
            }
            "POST" => {
                let Some(name) = request.param("uploadId") else {
                    return error(400, "InvalidRequest");
                };
                if !state.uploads.contains_key(name) {
                    return error(404, "NoSuchUpload");
                }
                if let Some(refused) = Self::refused(request, state.objects.get(&key)) {
                    return refused;
                }
                let upload = state.uploads.remove(name).expect("checked above");
                // The parts the request lists, in the order it lists them.
                let listed = String::from_utf8_lossy(&request.body).into_owned();
                let mut bytes = Vec::new();
                for number in texts_of(&listed, "PartNumber") {
                    match number.parse::<u32>().ok().and_then(|number| upload.parts.get(&number)) {
                        Some(part) => bytes.extend_from_slice(part),
                        None => return error(400, "InvalidPart"),
                    }
                }
                let mark = mark_of(&bytes);
                state.objects.insert(
                    upload.key,
                    Kept {
                        bytes: Arc::new(bytes),
                        mark: mark.clone(),
                        token: upload.token,
                    },
                );
                let body = format!("<?xml version=\"1.0\"?><CompleteMultipartUploadResult><ETag>{}</ETag></CompleteMultipartUploadResult>", mark.replace('"', "&#34;"));
                answer(200, &[], body.into_bytes())
            }
            "DELETE" if request.param("uploadId").is_some() => {
                state.uploads.remove(request.param("uploadId").unwrap_or_default());
                answer(204, &[], Vec::new())
            }
            "DELETE" => {
                if !self.deletes_regardless {
                    if let Some(refused) = Self::refused(request, state.objects.get(&key)) {
                        return refused;
                    }
                }
                state.objects.remove(&key);
                answer(204, &[], Vec::new())
            }
            _ => error(405, "MethodNotAllowed"),
        }
    }
}

impl Transport for SimulatedS3 {
    fn send(&self, request: &Request) -> io::Result<Response> {
        self.requests.fetch_add(1, Ordering::Relaxed);
        // A request that is not signed, or whose body is not the one it
        // was signed with, is one S3 would turn away.
        let signed = request.header("authorization").is_some_and(|value| value.starts_with("AWS4-HMAC-SHA256 "))
            && request.header("x-amz-content-sha256") == Some(hex(&sha256(&request.body)).as_str())
            && request.header("x-amz-date").is_some()
            && request.header("host") == Some(request.host.as_str());
        if !signed {
            return Ok(error(403, "SignatureDoesNotMatch"));
        }
        let mut state = self.state.lock().unwrap();
        let faults = state.faults;
        let mut happens = |chance: f64| chance > 0.0 && state.rng.gen_bool(chance.min(1.0));
        let (lost_request, fails, lost_answer, cut_short) = (
            happens(faults.lost_request),
            happens(faults.error),
            happens(faults.lost_answer),
            happens(faults.cut_short),
        );
        let lost = || io::Error::new(io::ErrorKind::TimedOut, "no answer came");
        if lost_request {
            self.faulted.fetch_add(1, Ordering::Relaxed);
            return Err(lost());
        }
        if fails {
            self.faulted.fetch_add(1, Ordering::Relaxed);
            return Ok(error(503, "SlowDown"));
        }
        let mut response = self.apply(&mut state, request);
        if lost_answer {
            self.faulted.fetch_add(1, Ordering::Relaxed);
            return Err(lost());
        }
        if cut_short && request.method == "GET" && response.body.len() > 1 {
            self.faulted.fetch_add(1, Ordering::Relaxed);
            let keep = state.rng.gen_range(0..response.body.len());
            response.body.truncate(keep);
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> S3Config {
        S3Config {
            bucket: "owl".into(),
            region: "us-east-1".into(),
            access_key: "AKIDEXAMPLE".into(),
            secret_key: "secret".into(),
            endpoint: "s3.test".into(),
            path_style: true,
            prefix: "dbs/".into(),
            part_bytes: 64,
            // Enough that a request going wrong every time is not chance.
            attempts: 40,
        }
    }

    fn store(seed: u64, faults: S3Faults) -> (S3ObjectStore, Arc<SimulatedS3>) {
        let s3 = Arc::new(SimulatedS3::new("owl", true, seed).with_page(3));
        s3.set_faults(faults);
        let store = S3ObjectStore::new(config(), s3.clone(), Arc::new(|| 1_700_000_000), u128::from(seed));
        (store, s3)
    }

    fn harsh() -> S3Faults {
        S3Faults {
            lost_request: 0.1,
            error: 0.1,
            lost_answer: 0.15,
            cut_short: 0.15,
        }
    }

    fn whole(store: &S3ObjectStore, key: &str) -> Vec<u8> {
        let len = store.len(key).unwrap().expect("the object is there") as usize;
        let mut bytes = vec![0u8; len];
        let mut read = 0;
        while read < len {
            read += store.read_at(key, read as u64, &mut bytes[read..(read + 50).min(len)]).unwrap();
        }
        bytes
    }

    #[test]
    fn the_hash_and_the_keyed_hash_are_the_standard_ones() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        // RFC 4231, the first test case.
        assert_eq!(
            hex(&hmac(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    /// The worked example in the S3 documentation for signing a GET with
    /// a range.
    #[test]
    fn a_request_is_signed_as_the_s3_documentation_signs_it() {
        let config = S3Config {
            bucket: "examplebucket".into(),
            region: "us-east-1".into(),
            access_key: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            endpoint: "examplebucket.s3.amazonaws.com".into(),
            path_style: false,
            prefix: String::new(),
            part_bytes: 8 << 20,
            attempts: 1,
        };
        let mut request = Request {
            method: "GET",
            host: config.endpoint.clone(),
            path: "/test.txt".into(),
            query: Vec::new(),
            headers: vec![("Range".into(), "bytes=0-9".into())],
            body: Vec::new(),
        };
        sign(&mut request, &config, "20130524T000000Z");
        assert_eq!(
            request.header("authorization").unwrap(),
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,\
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,\
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn text_is_read_out_of_xml_whichever_way_it_was_put_in() {
        // Found against a real server, which numbers its quotation marks.
        let xml = "<R><ETag>&#34;ab-3&#34;</ETag><ETag>&quot;cd&quot;</ETag><Key>a&amp;b &#x41; &lt;c&gt; d&e</Key></R>";
        assert_eq!(texts_of(xml, "ETag"), ["\"ab-3\"", "\"cd\""]);
        assert_eq!(text_of(xml, "Key").unwrap(), "a&b A <c> d&e");
    }

    #[test]
    fn a_time_is_written_as_s3_reads_it() {
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(amz_date(1_369_353_600), "20130524T000000Z");
        assert_eq!(amz_date(1_709_210_096), "20240229T123456Z");
        assert_eq!(amz_date(4_102_444_799), "20991231T235959Z");
    }

    #[test]
    fn a_key_is_encoded_for_the_wire_as_it_is_signed() {
        let request = Request {
            method: "GET",
            host: "h".into(),
            path: "/owl/dbs/shop.owl.part.3~2".into(),
            query: vec![("prefix".into(), "dbs/a b@1".into()), ("list-type".into(), "2".into())],
            headers: Vec::new(),
            body: Vec::new(),
        };
        assert_eq!(request.target(), "/owl/dbs/shop.owl.part.3~2?list-type=2&prefix=dbs%2Fa%20b%401");
    }

    fn does_what_an_object_store_does(store: &S3ObjectStore) {
        assert_eq!(store.len("a").unwrap(), None);
        assert_eq!(store.version("a").unwrap(), None);
        store.put("a", &mut &b"hello, owl"[..], 10).unwrap();
        assert_eq!(whole(store, "a"), b"hello, owl");
        let mut end = [0u8; 8];
        assert_eq!(store.read_at("a", 7, &mut end).unwrap(), 3);
        assert_eq!(&end[..3], b"owl");
        assert_eq!(store.read_at("a", 10, &mut end).unwrap(), 0);
        assert_eq!(store.read_at("gone", 0, &mut end).unwrap_err().kind(), io::ErrorKind::NotFound);

        // Longer than a part, so put in parts.
        let long: Vec<u8> = (0..1000u32).map(|n| (n % 251) as u8).collect();
        store.put("long", &mut &long[..], long.len() as u64).unwrap();
        assert_eq!(whole(store, "long"), long);

        // Only if nothing is there; only if it is the object last seen.
        let first = store.put_if("b", &mut &b"one"[..], 3, None).unwrap().expect("nothing was there");
        assert_eq!(store.put_if("b", &mut &b"two"[..], 3, None).unwrap(), None);
        assert_eq!(store.version("b").unwrap().as_deref(), Some(first.as_str()));
        let second = store.put_if("b", &mut &b"two"[..], 3, Some(&first)).unwrap().expect("it was as seen");
        assert_ne!(first, second);
        assert_eq!(store.put_if("b", &mut &b"six"[..], 3, Some(&first)).unwrap(), None);
        assert_eq!(whole(store, "b"), b"two");
        // And in parts.
        let third =
            store.put_if("b", &mut &long[..], long.len() as u64, Some(&second)).unwrap().expect("it was as seen");
        assert_eq!(store.put_if("b", &mut &long[..], long.len() as u64, Some(&second)).unwrap(), None);
        assert_eq!(whole(store, "b"), long);
        assert_eq!(store.put_if("gone", &mut &b"x"[..], 1, Some(&third)).unwrap(), None);

        for n in 0..8 {
            store.put(&format!("list/{n}"), &mut &b"x"[..], 1).unwrap();
        }
        let listed = store.list("list/").unwrap();
        assert_eq!(listed, (0..8).map(|n| format!("list/{n}")).collect::<Vec<_>>());
        assert_eq!(store.list("").unwrap().len(), 11);

        assert!(!store.delete_if("b", &first).unwrap());
        assert!(store.delete_if("b", &third).unwrap());
        assert_eq!(store.version("b").unwrap(), None);
        store.delete("a").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.len("a").unwrap(), None);
    }

    #[test]
    fn the_client_does_what_an_object_store_does() {
        let (store, s3) = store(1, S3Faults::default());
        does_what_an_object_store_does(&store);
        // Every key is under the prefix, and no put in parts is left open.
        assert!(s3.keys().iter().all(|key| key.starts_with("dbs/")));
        assert_eq!(s3.unfinished_uploads(), 0);
    }

    /// The same, with one request in three going wrong: lost, failed,
    /// applied and unanswered, or answered short. For many seeds.
    #[test]
    fn it_does_the_same_when_requests_and_answers_are_lost() {
        let mut requests = (0, 0);
        for seed in 0..300 {
            let (store, s3) = store(seed, harsh());
            does_what_an_object_store_does(&store);
            let (sent, faulted) = s3.requests();
            requests = (requests.0 + sent, requests.1 + faulted);
        }
        assert!(requests.1 * 5 > requests.0, "{} of {} requests went wrong", requests.1, requests.0);
    }

    /// The case the token is for: a put that must happen once, whose
    /// answer is lost. Tried again it is refused, and that refusal must
    /// not be read as someone else's write.
    #[test]
    fn a_conditional_put_whose_answer_was_lost_is_known_to_have_happened() {
        struct LosesOne {
            s3: SimulatedS3,
            lose: Mutex<Option<&'static str>>,
        }
        impl Transport for LosesOne {
            fn send(&self, request: &Request) -> io::Result<Response> {
                let response = self.s3.send(request)?;
                let mut lose = self.lose.lock().unwrap();
                let conditional = request.header("if-none-match").is_some() || request.header("if-match").is_some();
                if *lose == Some(request.method) && conditional {
                    *lose = None;
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "the answer was lost"));
                }
                Ok(response)
            }
        }
        for (method, long) in [("PUT", false), ("POST", true)] {
            let transport = Arc::new(LosesOne {
                s3: SimulatedS3::new("owl", true, 9),
                lose: Mutex::new(Some(method)),
            });
            let store = S3ObjectStore::new(config(), transport.clone(), Arc::new(|| 1_700_000_000), 9);
            let bytes = vec![7u8; if long { 500 } else { 5 }];
            let mark = store.put_if("claim", &mut &bytes[..], bytes.len() as u64, None).unwrap();
            assert!(mark.is_some(), "the {method} happened, though its answer was lost");
            assert_eq!(store.version("claim").unwrap(), mark);
            // And a put that truly loses is still told so.
            assert_eq!(store.put_if("claim", &mut &bytes[..], bytes.len() as u64, None).unwrap(), None);

            *transport.lose.lock().unwrap() = Some("DELETE");
            assert!(store.delete_if("claim", mark.as_deref().unwrap()).unwrap());
        }
    }

    /// Found against a real server: it deleted an object that was not
    /// the one the request named. The client must not lean on the server
    /// for that.
    #[test]
    fn a_server_that_ignores_the_condition_on_a_delete_deletes_only_what_was_meant() {
        let s3 = Arc::new(SimulatedS3::new("owl", true, 5).deleting_regardless());
        let store = S3ObjectStore::new(config(), s3, Arc::new(|| 1_700_000_000), 5);
        does_what_an_object_store_does(&store);
        let old = store.put_if("kept", &mut &b"one"[..], 3, None).unwrap().unwrap();
        let new = store.put_if("kept", &mut &b"two"[..], 3, Some(&old)).unwrap().unwrap();
        assert!(!store.delete_if("kept", &old).unwrap());
        assert_eq!(whole(&store, "kept"), b"two");
        assert!(store.delete_if("kept", &new).unwrap());
    }

    #[test]
    fn a_service_that_never_answers_is_an_error_and_not_a_wait() {
        let (store, s3) = store(3, S3Faults { lost_request: 1.0, ..S3Faults::default() });
        let error = store.put("a", &mut &b"x"[..], 1).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(s3.requests().0, 40);
    }

    /// A small HTTP server in front of a simulated S3, so that requests
    /// cross a real socket: what the client built is written to the wire
    /// by the transport, read back here, and has to mean the same.
    #[cfg(feature = "s3")]
    fn serve(s3: Arc<SimulatedS3>) -> String {
        use std::io::{BufRead, BufReader, Write};
        fn decoded(text: &str) -> String {
            let bytes = text.as_bytes();
            let mut out = Vec::new();
            let mut at = 0;
            while at < bytes.len() {
                if bytes[at] == b'%' && at + 2 < bytes.len() + 0 {
                    let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).unwrap();
                    out.push(u8::from_str_radix(hex, 16).unwrap());
                    at += 3;
                } else {
                    out.push(bytes[at]);
                    at += 1;
                }
            }
            String::from_utf8(out).unwrap()
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut words = line.split_whitespace();
                let method = match words.next() {
                    Some("GET") => "GET",
                    Some("HEAD") => "HEAD",
                    Some("PUT") => "PUT",
                    Some("POST") => "POST",
                    Some("DELETE") => "DELETE",
                    _ => continue,
                };
                let target = words.next().unwrap_or("/").to_string();
                let mut headers = Vec::new();
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let Some((name, value)) = header.trim_end().split_once(':') else { break };
                    headers.push((name.to_string(), value.trim().to_string()));
                }
                let len = super::header(&headers, "content-length").and_then(|len| len.parse().ok()).unwrap_or(0);
                let mut body = vec![0u8; len];
                std::io::Read::read_exact(&mut reader, &mut body).unwrap();
                let (path, query) = target.split_once('?').unwrap_or((&target, ""));
                let request = Request {
                    method,
                    host: super::header(&headers, "host").unwrap_or_default().to_string(),
                    path: decoded(path),
                    query: query
                        .split('&')
                        .filter(|pair| !pair.is_empty())
                        .map(|pair| {
                            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                            (decoded(name), decoded(value))
                        })
                        .collect(),
                    headers,
                    body,
                };
                // No answer: the connection just closes.
                let Ok(response) = s3.send(&request) else { continue };
                let mut stream = stream;
                let mut head = format!("HTTP/1.1 {} X\r\nconnection: close\r\n", response.status);
                for (name, value) in &response.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                if method != "HEAD" {
                    let _ = stream.write_all(&response.body);
                }
            }
        });
        address
    }

    /// The whole client over a real socket: once with nothing wrong, then
    /// with requests lost, failed, unanswered and answered short.
    #[cfg(feature = "s3")]
    #[test]
    fn the_client_does_the_same_over_a_socket() {
        for (seed, faults) in [(1, S3Faults::default()), (2, harsh()), (3, harsh())] {
            let s3 = Arc::new(SimulatedS3::new("owl", true, seed).with_page(3));
            s3.set_faults(faults);
            let mut config = config();
            config.endpoint = serve(s3.clone());
            let transport = Arc::new(HttpTransport::new(false, std::time::Duration::from_secs(10)));
            let store = S3ObjectStore::new(config, transport, Arc::new(|| 1_700_000_000), u128::from(seed));
            does_what_an_object_store_does(&store);
            assert!(s3.keys().iter().all(|key| key.starts_with("dbs/")));
        }
    }

    /// The same against a real S3 or a server that speaks its protocol.
    /// It is told where by the environment, and left out otherwise:
    ///
    /// ```text
    /// OWL_S3_ENDPOINT=127.0.0.1:9000 OWL_S3_BUCKET=owl-test \
    /// OWL_S3_ACCESS_KEY=... OWL_S3_SECRET_KEY=... \
    /// cargo test --release --features s3 --lib against_a_real_s3 -- --ignored
    /// ```
    ///
    /// `OWL_S3_REGION` defaults to `us-east-1`, `OWL_S3_SECURE=1` uses
    /// TLS, and `OWL_S3_VIRTUAL_HOST=1` puts the bucket in the host. The
    /// bucket is made if it is not there, and only keys under a prefix of
    /// this test's own are touched.
    #[cfg(feature = "s3")]
    #[test]
    #[ignore = "needs an S3 to talk to; see the comment"]
    fn the_client_does_the_same_against_a_real_s3() {
        let variable = |name: &str| std::env::var(name).ok();
        let endpoint = variable("OWL_S3_ENDPOINT").expect("OWL_S3_ENDPOINT");
        let config = S3Config {
            bucket: variable("OWL_S3_BUCKET").expect("OWL_S3_BUCKET"),
            region: variable("OWL_S3_REGION").unwrap_or_else(|| "us-east-1".into()),
            access_key: variable("OWL_S3_ACCESS_KEY").expect("OWL_S3_ACCESS_KEY"),
            secret_key: variable("OWL_S3_SECRET_KEY").expect("OWL_S3_SECRET_KEY"),
            endpoint,
            path_style: variable("OWL_S3_VIRTUAL_HOST").is_none(),
            prefix: format!("owl-client-test-{}/", rand::random::<u64>()),
            // The smallest part S3 takes, so that a put in parts is real.
            part_bytes: 5 << 20,
            attempts: 4,
        };
        let secure = variable("OWL_S3_SECURE").is_some();
        let transport = Arc::new(HttpTransport::new(secure, std::time::Duration::from_secs(60)));
        let store = S3ObjectStore::over(config, transport);

        // Make the bucket. One that is there already is no failure.
        // (Sent once, by hand: the client reads a 409 as a write to try
        // again, and here it means the bucket exists.)
        let mut make = store.request("PUT", None);
        sign(&mut make, &store.config, &amz_date((store.clock)()));
        let made = store.transport.send(&make).unwrap();
        assert!(matches!(made.status, 200 | 409), "the bucket could not be made: {}", failed("make it", &made));

        does_what_an_object_store_does(&store);

        // A put in parts of real size, conditional, and read back.
        let long: Vec<u8> = (0..(11u32 << 20)).map(|n| (n % 251) as u8).collect();
        let first = store.put_if("big", &mut &long[..], long.len() as u64, None).unwrap().expect("nothing was there");
        assert_eq!(store.put_if("big", &mut &long[..], long.len() as u64, None).unwrap(), None);
        assert_eq!(store.len("big").unwrap(), Some(long.len() as u64));
        assert_eq!(store.version("big").unwrap().as_deref(), Some(first.as_str()));
        let mut middle = [0u8; 64];
        let at = (5 << 20) - 32;
        assert_eq!(store.read_at("big", at as u64, &mut middle).unwrap(), 64);
        assert_eq!(&middle[..], &long[at..at + 64]);
        let second =
            store.put_if("big", &mut &long[..1000], 1000, Some(&first)).unwrap().expect("it was as seen");
        assert_eq!(store.put_if("big", &mut &long[..1000], 1000, Some(&first)).unwrap(), None);

        assert!(store.delete_if("big", &second).unwrap());

        // And a database: written and published by one process, read by
        // another that has nothing but the bucket.
        {
            use crate::document::{Document, Fields, Value};
            use crate::simulation::filesystem::RealFileSystem;
            use crate::storage::DocumentStore;
            use crate::storage_engine::{ObjectFileSystem, RustStore};
            let store: Arc<dyn ObjectStore> = Arc::new(store);
            let codes = |database: &RustStore| {
                let mut codes: Vec<i64> = database
                    .scan_documents("events")
                    .unwrap()
                    .iter()
                    .map(|event| match event.fields.get("code") {
                        Some(Value::Integer(code)) => *code,
                        other => panic!("an event without a code: {other:?}"),
                    })
                    .collect();
                codes.sort();
                codes
            };
            let (here, there) = (tempfile::TempDir::new().unwrap(), tempfile::TempDir::new().unwrap());
            let local = Arc::new(RealFileSystem::new());
            let filesystem =
                ObjectFileSystem::writer(local.clone(), store.clone(), here.path(), "shop.owl", 8 << 20).unwrap();
            let writer = RustStore::new_with_fs(here.path().join("shop.owl"), 64 << 20, filesystem.clone()).unwrap();
            for number in 0..2000 {
                let mut event = Document::with_id(format!("e{number:05}"), "events".to_string(), Fields::default());
                event.fields.insert("code".into(), Value::Integer(number));
                event.fields.insert("note".into(), Value::String("x".repeat(300)));
                writer.put_document(&event).unwrap();
            }
            writer.checkpoint().unwrap();
            writer.publish(&filesystem).unwrap();
            assert!(filesystem.drop_local_parts().unwrap() > 0);

            let (filesystem, published) =
                ObjectFileSystem::reader(local, store.clone(), there.path(), "shop.owl", 8 << 20).unwrap();
            assert_eq!(published.number, 1);
            let reader = RustStore::new_with_fs(there.path().join("shop.owl"), 64 << 20, filesystem).unwrap();
            assert_eq!(codes(&reader), (0..2000).collect::<Vec<_>>());
            assert_eq!(codes(&writer), (0..2000).collect::<Vec<_>>());

            // Leave nothing behind.
            for key in store.list("").unwrap() {
                store.delete(&key).unwrap();
            }
            assert!(store.list("").unwrap().is_empty());
        }
    }

    #[cfg(feature = "s3")]
    #[test]
    fn a_server_that_is_not_there_is_no_answer() {
        // A port nothing listens on.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);
        let mut config = config();
        config.endpoint = address;
        config.attempts = 2;
        let transport = Arc::new(HttpTransport::new(false, std::time::Duration::from_secs(2)));
        let store = S3ObjectStore::new(config, transport, Arc::new(|| 1_700_000_000), 1);
        assert_eq!(store.len("a").unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn a_request_that_is_not_signed_is_turned_away() {
        let s3 = SimulatedS3::new("owl", true, 4);
        let unsigned = Request {
            method: "GET",
            host: "s3.test".into(),
            path: "/owl/a".into(),
            query: Vec::new(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        assert_eq!(s3.send(&unsigned).unwrap().status, 403);
    }
}
