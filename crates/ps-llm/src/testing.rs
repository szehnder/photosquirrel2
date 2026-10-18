//! Test support (feature `testing`): a tiny in-process HTTP/1.1 server on `std::net::TcpListener` that replays
//! recorded request/response fixtures, asserts request shapes, and records every request. No real network.
//!
//! Fixture files (`tests/fixtures/<provider>/<name>.json`):
//! ```json
//! { "request":  { "method": "POST", "path": "/v1/chat/completions",
//!                 "headers": { "x-api-key": "test-key" }, "absent_headers": ["authorization"],
//!                 "body": { ... with every base64 JPEG replaced by "<jpeg>" ... } },
//!   "response": { "status": 200, "headers": { "retry-after": "3" }, "body": { ... } } }
//! ```
//! Set `PHOTOSQUIRREL_UPDATE_FIXTURES=1` to rewrite the expected request bodies from what the code sends now
//! (after a deliberate prompt or request change; review the diff).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A request the server received.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// Lower-case names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub at: Instant,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Every base64 JPEG in the body, decoded (the uploaded images).
    pub fn images(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        collect_images(&self.json(), &mut out);
        out
    }
}

fn collect_images(v: &Value, out: &mut Vec<Vec<u8>>) {
    use base64::Engine as _;
    match v {
        Value::String(s) => {
            let b64 = s.strip_prefix("data:image/jpeg;base64,").unwrap_or(s);
            if b64.starts_with("/9j/")
                && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64)
            {
                out.push(bytes);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_images(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_images(x, out)),
        _ => {}
    }
}

/// Replace every base64 JPEG string with `"<jpeg>"` (keeping a `data:` prefix), so shapes compare stably.
pub fn normalize_images(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            if let Some(rest) = s.strip_prefix("data:image/jpeg;base64,")
                && rest.starts_with("/9j/")
            {
                return Value::String("data:image/jpeg;base64,<jpeg>".into());
            }
            if s.starts_with("/9j/") && s.len() > 64 {
                return Value::String("<jpeg>".into());
            }
            v.clone()
        }
        Value::Array(a) => Value::Array(a.iter().map(normalize_images).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), normalize_images(x)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The answer to one request.
#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub fn json(status: u16, body: &Value) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }

    pub fn with_header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.to_owned(), v.to_owned()));
        self
    }

    pub fn with_delay(mut self, d: Duration) -> Self {
        self.delay = d;
        self
    }
}

/// One scripted exchange: an optional request expectation and the reply.
#[derive(Clone, Debug)]
pub struct Exchange {
    pub method: Option<String>,
    pub path: Option<String>,
    pub headers: Vec<(String, String)>,
    pub absent_headers: Vec<String>,
    /// Expected JSON body after [`normalize_images`].
    pub body: Option<Value>,
    pub reply: Reply,
    /// The fixture file this came from (for update mode).
    pub fixture: Option<PathBuf>,
}

impl Exchange {
    /// Reply without checking the request.
    pub fn reply(reply: Reply) -> Self {
        Self {
            method: None,
            path: None,
            headers: Vec::new(),
            absent_headers: Vec::new(),
            body: None,
            reply,
            fixture: None,
        }
    }

    /// Load a fixture file.
    pub fn from_fixture(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        let f: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
        let req = &f["request"];
        let resp = &f["response"];
        let map = |v: &Value| -> Vec<(String, String)> {
            v.as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| {
                    (
                        k.to_ascii_lowercase(),
                        v.as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        };
        let body = match &resp["body"] {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        Self {
            method: req["method"].as_str().map(str::to_owned),
            path: req["path"].as_str().map(str::to_owned),
            headers: map(&req["headers"]),
            absent_headers: req["absent_headers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|h| h.as_str().map(str::to_ascii_lowercase))
                .collect(),
            body: req.get("body").filter(|b| !b.is_null()).cloned(),
            reply: Reply {
                status: resp["status"].as_u64().unwrap_or(200) as u16,
                headers: map(&resp["headers"]),
                body,
                delay: Duration::ZERO,
            },
            fixture: Some(path.to_path_buf()),
        }
    }

    /// Mismatches between this expectation and a request (empty when it matches).
    pub fn check(&self, r: &Recorded) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(m) = &self.method
            && !m.eq_ignore_ascii_case(&r.method)
        {
            out.push(format!("method: expected {m}, got {}", r.method));
        }
        if let Some(p) = &self.path
            && *p != r.path
        {
            out.push(format!("path: expected {p}, got {}", r.path));
        }
        for (k, v) in &self.headers {
            match r.header(k) {
                Some(got) if v == "*" || got == v => {}
                Some(_) => out.push(format!("header {k}: unexpected value")),
                None => out.push(format!("header {k}: missing")),
            }
        }
        for k in &self.absent_headers {
            if r.header(k).is_some() {
                out.push(format!("header {k}: must be absent"));
            }
        }
        if let Some(expected) = &self.body {
            let got = normalize_images(&r.json());
            if let Some(d) = first_difference(expected, &got, "$") {
                out.push(format!("body: {d}"));
            }
        }
        out
    }
}

/// The JSON path of the first difference between two values.
pub fn first_difference(expected: &Value, got: &Value, path: &str) -> Option<String> {
    match (expected, got) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, va) in a {
                match b.get(k) {
                    Some(vb) => {
                        if let Some(d) = first_difference(va, vb, &format!("{path}.{k}")) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{path}.{k}: missing")),
                }
            }
            b.keys()
                .find(|k| !a.contains_key(*k))
                .map(|k| format!("{path}.{k}: unexpected"))
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!(
                    "{path}: expected {} items, got {}",
                    a.len(),
                    b.len()
                ));
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(i, (x, y))| first_difference(x, y, &format!("{path}[{i}]")))
        }
        (a, b) if a == b => None,
        (Value::Number(a), Value::Number(b)) if a.as_f64() == b.as_f64() => None,
        (a, b) => {
            let s = |v: &Value| {
                let t = v.to_string();
                if t.len() > 120 {
                    format!("{}…", &t[..120])
                } else {
                    t
                }
            };
            Some(format!("{path}: expected {}, got {}", s(a), s(b)))
        }
    }
}

type Handler = Arc<dyn Fn(&Recorded) -> Reply + Send + Sync>;

struct State {
    script: VecDeque<Exchange>,
    handler: Option<Handler>,
    recorded: Vec<Recorded>,
    mismatches: Vec<String>,
}

/// The fixture server. Dropping it stops accepting connections.
pub struct MockServer {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for MockServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MockServer({})", self.addr)
    }
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl MockServer {
    /// Serve `script` in order; requests beyond it get HTTP 422 (not retried).
    pub fn start(script: Vec<Exchange>) -> Self {
        Self::spawn(script.into(), None)
    }

    /// Serve fixture files in order.
    pub fn from_fixtures(paths: &[PathBuf]) -> Self {
        Self::start(paths.iter().map(Exchange::from_fixture).collect())
    }

    /// Answer every request with `handler` (queue tests).
    pub fn with_handler(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        Self::spawn(VecDeque::new(), Some(Arc::new(handler)))
    }

    fn spawn(script: VecDeque<Exchange>, handler: Option<Handler>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("bind: {e}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("addr: {e}"));
        let state = Arc::new(Mutex::new(State {
            script,
            handler,
            recorded: Vec::new(),
            mismatches: Vec::new(),
        }));
        let server = Self {
            addr,
            state,
            active: Arc::default(),
            max_active: Arc::default(),
            stop: Arc::default(),
        };
        let (state, active, max_active, stop) = (
            server.state.clone(),
            server.active.clone(),
            server.max_active.clone(),
            server.stop.clone(),
        );
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                let (state, active, max_active) =
                    (state.clone(), active.clone(), max_active.clone());
                std::thread::spawn(move || serve(conn, &state, &active, &max_active));
            }
        });
        server
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        lock(&self.state).recorded.clone()
    }

    pub fn request_count(&self) -> usize {
        lock(&self.state).recorded.len()
    }

    pub fn mismatches(&self) -> Vec<String> {
        lock(&self.state).mismatches.clone()
    }

    /// Most requests ever in flight at once.
    pub fn max_in_flight(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }

    /// Panic unless every scripted exchange was used and every request matched.
    pub fn assert_all_matched(&self) {
        let s = lock(&self.state);
        assert!(
            s.mismatches.is_empty(),
            "request shape mismatches:\n{}",
            s.mismatches.join("\n")
        );
        assert!(
            s.script.is_empty(),
            "{} scripted exchanges were not requested",
            s.script.len()
        );
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the accept loop.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
    }
}

fn update_mode() -> bool {
    std::env::var("PHOTOSQUIRREL_UPDATE_FIXTURES").is_ok_and(|v| v == "1")
}

fn serve(conn: TcpStream, state: &Mutex<State>, active: &AtomicUsize, max_active: &AtomicUsize) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(match conn.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    });
    let mut conn = conn;
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let rec = Recorded {
        method,
        path,
        headers,
        body,
        at: Instant::now(),
    };
    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
    max_active.fetch_max(now, Ordering::SeqCst);
    let reply = {
        let mut s = lock(state);
        s.recorded.push(rec.clone());
        if let Some(h) = s.handler.clone() {
            drop(s);
            h(&rec)
        } else if let Some(ex) = s.script.pop_front() {
            let problems = ex.check(&rec);
            if update_mode() && ex.fixture.is_some() {
                rewrite_fixture(&ex, &rec);
            } else if !problems.is_empty() {
                let n = s.recorded.len();
                s.mismatches.extend(
                    problems
                        .iter()
                        .map(|p| format!("request {n} ({} {}): {p}", rec.method, rec.path)),
                );
                drop(s);
                active.fetch_sub(1, Ordering::SeqCst);
                write_reply(
                    &mut conn,
                    &Reply::json(422, &json!({"error": {"message": problems.join("; ")}})),
                );
                return;
            }
            ex.reply
        } else {
            let n = s.recorded.len();
            s.mismatches.push(format!(
                "request {n} ({} {}): not scripted",
                rec.method, rec.path
            ));
            Reply::json(422, &json!({"error": {"message": "unscripted request"}}))
        }
    };
    if !reply.delay.is_zero() {
        std::thread::sleep(reply.delay);
    }
    active.fetch_sub(1, Ordering::SeqCst);
    write_reply(&mut conn, &reply);
}

fn rewrite_fixture(ex: &Exchange, rec: &Recorded) {
    let Some(path) = &ex.fixture else { return };
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(mut f) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    f["request"]["method"] = Value::String(rec.method.clone());
    f["request"]["path"] = Value::String(rec.path.clone());
    f["request"]["body"] = normalize_images(&rec.json());
    if let Ok(s) = serde_json::to_string_pretty(&f) {
        let _ = std::fs::write(path, s + "\n");
        eprintln!("updated fixture {}", path.display());
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

fn write_reply(conn: &mut TcpStream, r: &Reply) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
        r.status,
        reason(r.status),
        r.body.len()
    );
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = conn.write_all(head.as_bytes());
    let _ = conn.write_all(r.body.as_bytes());
    let _ = conn.flush();
}

/// A schema-valid insight JSON (as a model would answer).
pub fn insight_json(category: &str, adults: u32, children: u32, pets: &[&str]) -> Value {
    json!({
        "subject": format!("A {category} photo."),
        "category": category,
        "also": [],
        "is_photo": true,
        "people": { "total": adults + children, "adults": adults, "children": children },
        "pets": pets,
        "tags": ["test"],
        "setting": "outdoor garden",
        "confidence": 0.9
    })
}

/// An OpenAI chat-completions answer whose message content is `content`.
pub fn openai_reply(content: &str) -> Value {
    json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 1_760_000_000,
        "model": "test-vlm",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 900, "completion_tokens": 80, "total_tokens": 980 }
    })
}

/// A small synthetic preview JPEG (never a real photo).
pub fn preview(seed: u64) -> crate::PreviewJpeg {
    let (w, h) = (64u32, 48u32);
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let v = u64::from(x * 3 + y * 5) ^ seed.wrapping_mul(0x9e37_79b9);
            data.extend_from_slice(&[
                (v & 0xff) as u8,
                ((v >> 3) & 0xff) as u8,
                ((v >> 5) & 0xff) as u8,
            ]);
        }
    }
    let img = ps_media::RgbImage {
        width: w,
        height: h,
        data,
    };
    let jpeg = ps_media::thumb::encode_jpeg(&img, 85).unwrap_or_else(|e| panic!("{e}"));
    crate::PreviewJpeg::from_jpeg(jpeg).unwrap_or_else(|e| panic!("{e}"))
}

/// A port on 127.0.0.1 where nothing listens.
pub fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("bind: {e}"));
    let port = l.local_addr().map(|a| a.port()).unwrap_or(1);
    drop(l);
    port
}
