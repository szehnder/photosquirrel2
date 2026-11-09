use std::io;

/// Why a check, download or install did not happen. The messages are shown to the user as they are.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("updates are off in developer builds")]
    DeveloperBuild,
    #[error("could not reach the update server: {0}")]
    Network(String),
    #[error("the update server answered {0}")]
    Status(u16),
    #[error("redirect to an untrusted host ({0})")]
    UntrustedRedirect(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("the update address must use https ({0})")]
    Insecure(String),
    #[error("the response is larger than the expected {0} bytes")]
    TooLarge(u64),
    #[error("update manifest signature is not valid")]
    BadSignature,
    #[error("the update manifest cannot be read: {0}")]
    BadManifest(String),
    #[error("downloaded update does not match its signature")]
    Mismatch,
    #[error("not enough disk space for the update: {needed} bytes needed, {available} free")]
    NoSpace { needed: u64, available: u64 },
    #[error("{0} is not a valid test server address (only http://127.0.0.1:<port> is accepted)")]
    BadServerOverride(String),
    #[error("{0}")]
    Io(#[from] io::Error),
}
