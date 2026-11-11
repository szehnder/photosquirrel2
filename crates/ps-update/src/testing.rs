//! Test support (feature `testing`): signing with the test key or a throwaway key, sample manifests, and a
//! loopback HTTP server that records requests and serves ranges, redirects and cut-off bodies.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use minisign_verify::PublicKey;
use sha2::{Digest, Sha256};

use crate::build::DOWNLOAD_PREFIX;
use crate::check::Updater;
use crate::dir::UpdateDir;
use crate::manifest::{Envelope, FORMAT, Manifest, Package};
use crate::transport::{Endpoint, Http};

/// The committed test secret key (TEST ONLY; never trusted by a release build).
const TEST_SECRET: &str = include_str!("../tests/data/test-key.sec");

/// A key that signs manifests in tests.
pub struct TestSigner {
    sk: minisign::SecretKey,
    pk: minisign::PublicKey,
}

impl TestSigner {
    /// The committed test key, which test builds trust.
    pub fn test_key() -> Self {
        let sk = minisign::SecretKeyBox::from_string(TEST_SECRET)
            .and_then(minisign::SecretKeyBox::into_unencrypted_secret_key)
            .expect("test secret key");
        let pk = minisign::PublicKey::from_secret_key(&sk).expect("test public key");
        Self { sk, pk }
    }

    /// A fresh key that nothing trusts.
    pub fn generate() -> Self {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().expect("key pair");
        Self {
            sk: kp.sk,
            pk: kp.pk,
        }
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey::from_base64(&self.pk.to_base64()).expect("public key")
    }

    /// The public key as a `.pub` file's base64 line.
    pub fn public_key_line(&self) -> String {
        self.pk.to_base64()
    }

    /// A minisign signature file's text for `text`.
    pub fn sign(&self, text: &str, trusted_comment: &str) -> String {
        minisign::sign(
            Some(&self.pk),
            &self.sk,
            text.as_bytes(),
            Some(trusted_comment),
            Some("signature from a PhotoSquirrel test key"),
        )
        .expect("sign")
        .into_string()
    }

    /// The envelope of `manifest`, signed with the comment its version needs.
    pub fn envelope(&self, manifest: &Manifest) -> String {
        self.envelope_with_comment(manifest, &Manifest::trusted_comment(&manifest.version))
    }

    pub fn envelope_with_comment(&self, manifest: &Manifest, comment: &str) -> String {
        let text = serde_json::to_string_pretty(manifest).expect("manifest json");
        let signature = self.sign(&text, comment);
        serde_json::to_string(&Envelope {
            format: FORMAT,
            manifest: text,
            signature,
        })
        .expect("envelope json")
    }
}

/// An updater for `server`, as a release build of `installed` on `linux-x86_64`, with its update folder at
/// `dir`, trusting the test key, with unlimited free space.
pub fn updater(server: &TestServer, dir: &std::path::Path, installed: &str) -> Updater {
    Updater {
        dir: UpdateDir::at(dir),
        transport: Arc::new(Http::new()),
        endpoint: Endpoint::loopback(&server.url()).expect("loopback"),
        installed: semver::Version::parse(installed).expect("version"),
        release_build: true,
        keys: crate::keys::trusted(),
        platform: "linux-x86_64".into(),
        free_space: Arc::new(|_| Some(u64::MAX)),
    }
}

/// A signature by the committed test key.
pub fn sign(text: &str, trusted_comment: &str) -> String {
    TestSigner::test_key().sign(text, trusted_comment)
}

/// The envelope of `manifest`, signed by the committed test key.
pub fn envelope(manifest: &Manifest) -> String {
    TestSigner::test_key().envelope(manifest)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A manifest of `version` listing `(platform, kind, bytes)` packages named `PhotoSquirrel-<version>-<kind>`.
pub fn sample_manifest(version: &str, packages: &[(&str, &str, &[u8])]) -> Manifest {
    let mut table: BTreeMap<String, BTreeMap<String, Package>> = BTreeMap::new();
    for (platform, kind, bytes) in packages {
        let name = format!("PhotoSquirrel-{version}-{platform}.{kind}");
        table.entry((*platform).to_owned()).or_default().insert(
            (*kind).to_owned(),
            Package {
                url: format!("{DOWNLOAD_PREFIX}v{version}/{name}"),
                name,
                size: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            },
        );
    }
    Manifest {
        product: "PhotoSquirrel".into(),
        version: version.into(),
        released: "2026-12-01T12:00:00Z".into(),
        urgent: false,
        notes: format!("What's new in {version}."),
        notes_url: format!("https://github.com/szehnder/photosquirrel2/releases/tag/v{version}"),
        packages: table,
    }
}

/// What a test server answers for a path.
#[derive(Clone, Debug)]
pub enum Route {
    /// 200 with this body; a `Range: bytes=N-` request gets 206 and the rest.
    Body(Vec<u8>),
    /// A 302 to this location.
    Redirect(String),
    /// This status with an empty body.
    Status(u16),
    /// Announces the whole body but closes the connection after this many bytes.
    CutAfter(Vec<u8>, usize),
    /// 200 with this many bytes of `x`, without announcing a length.
    Endless(u64),
}

/// A request the server saw.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub path: String,
    /// Everything after `?`, if anything.
    pub query: Option<String>,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A loopback HTTP/1.1 server on its own thread, one connection at a time, `Connection: close`.
pub struct TestServer {
    port: u16,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    seen: Arc<Mutex<Vec<Recorded>>>,
}

impl TestServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let routes: Arc<Mutex<HashMap<String, Route>>> = Arc::default();
        let seen: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let (r, s) = (Arc::clone(&routes), Arc::clone(&seen));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &r, &s);
            }
        });
        Self { port, routes, seen }
    }

    /// `http://127.0.0.1:<port>`
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn route(&self, path: &str, route: Route) {
        self.routes
            .lock()
            .expect("routes")
            .insert(path.to_owned(), route);
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.seen.lock().expect("seen").clone()
    }
}

fn serve(stream: TcpStream, routes: &Mutex<HashMap<String, Route>>, seen: &Mutex<Vec<Recorded>>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let target = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_owned(), Some(q.to_owned())),
        None => (target.clone(), None),
    };
    let range_from = headers
        .iter()
        .find(|(k, _)| k == "range")
        .and_then(|(_, v)| v.strip_prefix("bytes="))
        .and_then(|v| v.strip_suffix('-'))
        .and_then(|v| v.parse::<usize>().ok());
    seen.lock().expect("seen").push(Recorded {
        path: path.clone(),
        query,
        headers,
    });
    let route = routes.lock().expect("routes").get(&path).cloned();
    let mut out = stream;
    let _ = match route {
        None => write!(out, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        Some(Route::Status(code)) => write!(
            out,
            "HTTP/1.1 {code} Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        Some(Route::Redirect(to)) => write!(
            out,
            "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        Some(Route::Body(body)) => match range_from {
            Some(from) if from < body.len() => write!(
                out,
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {from}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len() - 1,
                body.len(),
                body.len() - from
            )
            .and_then(|()| out.write_all(&body[from..])),
            _ => write!(
                out,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .and_then(|()| out.write_all(&body)),
        },
        Some(Route::CutAfter(body, n)) => write!(
            out,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .and_then(|()| out.write_all(&body[..n.min(body.len())])),
        Some(Route::Endless(n)) => write!(out, "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n").and_then(|()| {
            let chunk = [b'x'; 8192];
            let mut left = n;
            while left > 0 {
                let k = left.min(chunk.len() as u64) as usize;
                out.write_all(&chunk[..k])?;
                left -= k as u64;
            }
            Ok(())
        }),
    };
    let _ = out.flush();
    // Drain whatever the client still sends so the close is clean.
    let _ = reader.get_mut().set_nonblocking(true);
    let mut sink = [0u8; 256];
    let _ = reader.read(&mut sink);
}
