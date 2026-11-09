//! The HTTP side of the updater (ADR-0124, spec "Nothing about the user is sent", "Restricted transport").
//!
//! One GET at a time: `User-Agent: PhotoSquirrel` and nothing that identifies the user or the installation, no
//! cookie jar, HTTPS with the platform's certificate verifier. Redirects are followed by hand, at most
//! [`MAX_REDIRECTS`], and only to HTTPS on [`RELEASE_HOSTS`] (or, with the loopback test server, to that same
//! server). Responses are read only up to the size the caller expects.

use std::io::{Read, Write};
use std::time::Duration;

use crate::UpdateError;
use crate::build::{MANIFEST_URL, PRODUCT, RELEASE_HOSTS, SERVER_ENV};

pub const MAX_REDIRECTS: u32 = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// An open response body.
pub struct Opened {
    pub body: Box<dyn Read + Send>,
    /// The server answered a range request with the rest of the file (206); otherwise the body is the whole file.
    pub partial: bool,
}

/// Where requests go: GitHub, or the loopback test server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    GitHub,
    /// `http://127.0.0.1:<port>` (spec: the test server override; signatures still apply).
    Loopback(String),
}

impl Endpoint {
    /// GitHub, unless `PHOTOSQUIRREL_UPDATE_SERVER` names a loopback server.
    pub fn from_env() -> Result<Self, UpdateError> {
        match std::env::var(SERVER_ENV) {
            Ok(v) if !v.trim().is_empty() => Self::loopback(v.trim()),
            _ => Ok(Self::GitHub),
        }
    }

    /// A loopback server origin: `http://127.0.0.1:<port>` or `http://localhost:<port>`, nothing else.
    pub fn loopback(origin: &str) -> Result<Self, UpdateError> {
        let origin = origin.trim_end_matches('/');
        let rest = origin
            .strip_prefix("http://")
            .ok_or_else(|| UpdateError::BadServerOverride(origin.to_owned()))?;
        let (host, port) = rest
            .rsplit_once(':')
            .ok_or_else(|| UpdateError::BadServerOverride(origin.to_owned()))?;
        if !matches!(host, "127.0.0.1" | "localhost") || port.parse::<u16>().is_err() {
            return Err(UpdateError::BadServerOverride(origin.to_owned()));
        }
        Ok(Self::Loopback(origin.to_owned()))
    }

    pub fn manifest_url(&self) -> String {
        match self {
            Self::GitHub => MANIFEST_URL.to_owned(),
            Self::Loopback(o) => format!("{o}/update-manifest.json"),
        }
    }

    /// Where a package is fetched from: its manifest URL on GitHub, `<server>/<name>` on the test server.
    pub fn package_url(&self, package: &crate::Package) -> String {
        match self {
            Self::GitHub => package.url.clone(),
            Self::Loopback(o) => format!("{o}/{}", package.name),
        }
    }

    /// Whether a request may go to `url`.
    fn allows(&self, url: &str) -> Result<(), UpdateError> {
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| UpdateError::Insecure(url.to_owned()))?;
        // The authority, without any `user@`; the host is what is left before a port.
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = authority
            .rsplit('@')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match self {
            Self::Loopback(origin) => {
                if url == origin || url.starts_with(&format!("{origin}/")) {
                    Ok(())
                } else if scheme == "https" {
                    Err(UpdateError::UntrustedRedirect(host))
                } else {
                    Err(UpdateError::Insecure(url.to_owned()))
                }
            }
            Self::GitHub => {
                if scheme != "https" {
                    return Err(UpdateError::Insecure(url.to_owned()));
                }
                let bare = host.split(':').next().unwrap_or_default();
                if RELEASE_HOSTS.contains(&bare) {
                    Ok(())
                } else {
                    Err(UpdateError::UntrustedRedirect(bare.to_owned()))
                }
            }
        }
    }
}

/// Fetches update files. The real one is [`Http`]; tests may record or fake requests.
pub trait Transport: Send + Sync {
    /// GET `url` (from byte `range_from` when given) and return its body once the final response (after allowed
    /// redirects) is 200 or 206. Read it with [`copy_limited`].
    fn open(
        &self,
        endpoint: &Endpoint,
        url: &str,
        range_from: Option<u64>,
    ) -> Result<Opened, UpdateError>;

    /// GET a small file whole, at most `limit` bytes.
    fn fetch(&self, endpoint: &Endpoint, url: &str, limit: u64) -> Result<Vec<u8>, UpdateError> {
        let opened = self.open(endpoint, url, None)?;
        let mut body = Vec::new();
        copy_limited(opened.body, &mut body, limit)?;
        Ok(body)
    }
}

/// The real transport: blocking `ureq` with rustls and the platform's verifier.
pub struct Http {
    agent: ureq::Agent,
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

impl Http {
    pub fn new() -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(PRODUCT)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_body(Some(READ_TIMEOUT))
            .timeout_recv_response(Some(READ_TIMEOUT))
            .https_only(false)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Transport for Http {
    fn open(
        &self,
        endpoint: &Endpoint,
        url: &str,
        range_from: Option<u64>,
    ) -> Result<Opened, UpdateError> {
        let mut url = url.to_owned();
        for _ in 0..=MAX_REDIRECTS {
            endpoint.allows(&url)?;
            let mut req = self
                .agent
                .get(&url)
                .header("Accept", "application/octet-stream, application/json");
            if let Some(from) = range_from.filter(|f| *f > 0) {
                req = req.header("Range", format!("bytes={from}-"));
            }
            let resp = req
                .call()
                .map_err(|e| UpdateError::Network(e.to_string()))?;
            let status = resp.status().as_u16();
            if (300..400).contains(&status) {
                let location = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or(UpdateError::Status(status))?;
                url = resolve(&url, location);
                continue;
            }
            if status != 200 && status != 206 {
                return Err(UpdateError::Status(status));
            }
            let partial = status == 206 && range_from.is_some_and(|f| f > 0);
            return Ok(Opened {
                body: Box::new(resp.into_body().into_reader()),
                partial,
            });
        }
        Err(UpdateError::TooManyRedirects)
    }
}

/// A `Location` against the URL it came from (absolute, or a path on the same origin).
fn resolve(base: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_owned();
    }
    let origin_end = base
        .find("://")
        .and_then(|i| base[i + 3..].find('/').map(|j| i + 3 + j))
        .unwrap_or(base.len());
    if location.starts_with('/') {
        format!("{}{location}", &base[..origin_end])
    } else {
        let dir_end = base
            .rfind('/')
            .filter(|i| *i >= origin_end)
            .unwrap_or(base.len());
        format!("{}/{location}", &base[..dir_end])
    }
}

/// Copy at most `limit` bytes; one byte more fails without being written.
pub fn copy_limited(
    mut reader: impl Read,
    sink: &mut dyn Write,
    limit: u64,
) -> Result<u64, UpdateError> {
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        if n == 0 {
            return Ok(total);
        }
        let room = limit - total;
        if n as u64 > room {
            sink.write_all(&buf[..room as usize])?;
            return Err(UpdateError::TooLarge(limit));
        }
        sink.write_all(&buf[..n])?;
        total += n as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Route, TestServer};

    fn get(server: &TestServer, path: &str, limit: u64) -> (Result<u64, UpdateError>, Vec<u8>) {
        let endpoint = Endpoint::loopback(&server.url()).expect("loopback");
        let mut body = Vec::new();
        let r = Http::new()
            .open(&endpoint, &format!("{}{path}", server.url()), None)
            .and_then(|o| copy_limited(o.body, &mut body, limit));
        (r, body)
    }

    #[test]
    fn scenario_app_updates_check_request_carries_no_identifiers() {
        let server = TestServer::start();
        server.route("/update-manifest.json", Route::Body(b"{}".to_vec()));
        let endpoint = Endpoint::loopback(&server.url()).expect("loopback");
        Http::new()
            .fetch(&endpoint, &endpoint.manifest_url(), 1024)
            .expect("get");
        let reqs = server.requests();
        assert_eq!(reqs.len(), 1);
        let r = &reqs[0];
        assert_eq!(r.path, "/update-manifest.json");
        assert_eq!(r.query, None);
        assert_eq!(r.header("cookie"), None);
        assert_eq!(r.header("user-agent"), Some("PhotoSquirrel"));
        let version = env!("CARGO_PKG_VERSION");
        let host = std::env::var("HOSTNAME").unwrap_or_default();
        let user = std::env::var("USER").unwrap_or_default();
        for (k, v) in &r.headers {
            assert!(!v.contains(version), "{k}: {v} carries the version");
            if !host.is_empty() {
                assert!(!v.contains(&host), "{k}: {v} carries the host name");
            }
            if user.len() > 2 {
                assert!(!v.contains(&user), "{k}: {v} carries the user name");
            }
            assert!(!v.contains('/') || k == "accept", "{k}: {v} carries a path");
        }
    }

    #[test]
    fn scenario_app_updates_redirect_to_an_unknown_host_refused() {
        let server = TestServer::start();
        server.route(
            "/a",
            Route::Redirect("https://evil.example.com/update-manifest.json".into()),
        );
        server.route("/b", Route::Redirect("http://evil.example.com/x".into()));
        server.route("/c", Route::Redirect("/d".into()));
        server.route("/d", Route::Body(b"ok".to_vec()));
        let (r, _) = get(&server, "/a", 1024);
        let e = r.expect_err("refused");
        assert!(
            matches!(e, UpdateError::UntrustedRedirect(ref h) if h == "evil.example.com"),
            "{e}"
        );
        assert!(e.to_string().starts_with("redirect to an untrusted host"));
        let (r, _) = get(&server, "/b", 1024);
        assert!(matches!(r, Err(UpdateError::Insecure(_))));
        // A same-server redirect is followed.
        let (r, body) = get(&server, "/c", 1024);
        assert_eq!(r.expect("followed"), 2);
        assert_eq!(body, b"ok");
        // On GitHub, only HTTPS to the release hosts.
        assert!(
            Endpoint::GitHub
                .allows("https://objects.githubusercontent.com/x")
                .is_ok()
        );
        assert!(
            Endpoint::GitHub
                .allows("https://github.com/szehnder/photosquirrel2/releases")
                .is_ok()
        );
        assert!(matches!(
            Endpoint::GitHub.allows("https://evil.example.com/x"),
            Err(UpdateError::UntrustedRedirect(_))
        ));
        assert!(matches!(
            Endpoint::GitHub.allows("https://user@evil.example.com/x"),
            Err(UpdateError::UntrustedRedirect(_))
        ));
        assert!(matches!(
            Endpoint::GitHub.allows("http://github.com/x"),
            Err(UpdateError::Insecure(_))
        ));
    }

    #[test]
    fn too_many_redirects_fail() {
        let server = TestServer::start();
        server.route("/loop", Route::Redirect("/loop".into()));
        let (r, _) = get(&server, "/loop", 1024);
        assert!(matches!(r, Err(UpdateError::TooManyRedirects)));
        assert_eq!(server.requests().len(), MAX_REDIRECTS as usize + 1);
    }

    #[test]
    fn scenario_app_updates_oversized_response_cut_off() {
        let server = TestServer::start();
        server.route("/big", Route::Endless(70 * 1024));
        let (r, body) = get(&server, "/big", crate::manifest::MAX_MANIFEST_BYTES);
        assert!(
            matches!(r, Err(UpdateError::TooLarge(n)) if n == crate::manifest::MAX_MANIFEST_BYTES)
        );
        assert_eq!(
            body.len() as u64,
            crate::manifest::MAX_MANIFEST_BYTES,
            "nothing past the limit is written"
        );
    }

    #[test]
    fn only_loopback_servers_override_github() {
        assert!(Endpoint::loopback("http://127.0.0.1:8765").is_ok());
        assert!(Endpoint::loopback("http://localhost:8765/").is_ok());
        for bad in [
            "https://127.0.0.1:1",
            "http://10.0.0.1:80",
            "http://example.com:80",
            "http://127.0.0.1",
        ] {
            assert!(Endpoint::loopback(bad).is_err(), "{bad}");
        }
        assert_eq!(
            Endpoint::loopback("http://127.0.0.1:9/")
                .expect("ok")
                .manifest_url(),
            "http://127.0.0.1:9/update-manifest.json"
        );
    }

    #[test]
    fn locations_resolve_against_their_request() {
        assert_eq!(resolve("https://a.com/x/y", "/z"), "https://a.com/z");
        assert_eq!(resolve("https://a.com/x/y", "z"), "https://a.com/x/z");
        assert_eq!(
            resolve("https://a.com/x", "https://b.com/q"),
            "https://b.com/q"
        );
    }
}
