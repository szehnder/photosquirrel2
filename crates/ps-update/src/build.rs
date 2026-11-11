//! What this build is and where its updates come from (ADR-0124, ADR-0127).

/// Whether this binary was built by `cargo xtask package` (`PHOTOSQUIRREL_RELEASE_BUILD=1`). Other builds are
/// developer builds and never check for updates (spec "Developer builds never check").
pub const RELEASE_BUILD: bool = match option_env!("PHOTOSQUIRREL_RELEASE_BUILD") {
    Some(v) => v.len() == 1 && v.as_bytes()[0] == b'1',
    None => false,
};

/// The product name: the whole user agent of every update request (spec "Nothing about the user is sent").
pub const PRODUCT: &str = "PhotoSquirrel";

/// The fixed manifest address: GitHub's `latest` release, which skips drafts and prereleases.
pub const MANIFEST_URL: &str =
    "https://github.com/szehnder/photosquirrel2/releases/latest/download/update-manifest.json";

/// Every package a manifest lists is downloaded from under this prefix, followed by `v<version>/<name>`.
pub const DOWNLOAD_PREFIX: &str = "https://github.com/szehnder/photosquirrel2/releases/download/";

/// The release page of a version: `<RELEASE_PAGE_PREFIX>v<version>`.
pub const RELEASE_PAGE_PREFIX: &str = "https://github.com/szehnder/photosquirrel2/releases/tag/";

/// Hosts a release download may redirect to (GitHub's signed download URLs). Integrity never depends on this
/// list: the manifest signature and the payload hashes do (ADR-0125).
pub const RELEASE_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// The loopback-only test server override: replaces the origin of the manifest and of every payload
/// (`<server>/update-manifest.json`, `<server>/<name>`). Signatures, hashes and version rules still apply.
pub const SERVER_ENV: &str = "PHOTOSQUIRREL_UPDATE_SERVER";

/// The version this binary is.
pub fn current_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is semver")
}

/// The platform key of this build in a manifest's `packages` table.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos-universal"
    } else if cfg!(target_os = "windows") {
        "windows-x86_64"
    } else {
        "linux-x86_64"
    }
}
