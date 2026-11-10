# 0127. A `ps-update` I/O crate with a blocking HTTPS client, minisign verification and no UI dependency

- Status: accepted
- Date: 2026-11-08
- Deciders: maintainers

## Context
The updater needs HTTPS, signature verification, archive extraction (macOS) and platform calls. ADR-0003 forbids
gpui-kit in I/O crates and tokio in core crates. The "Quiet start" test asserts that the UI crates link no HTTP
client. `ps-llm` uses async `reqwest`, but the updater runs a few requests a day on a plain thread.

## Decision
- A new I/O crate, `crates/ps-update`. It depends on `ps-model`, `ureq` 3 (rustls with the platform verifier),
  `minisign-verify`, `semver`, SHA-256, `serde_json` and `rustix` (Unix). There is no tokio. The macOS bundle is
  unpacked with the system `tar`, and the Windows registry is read with `reg query`, so neither needs a crate or
  `unsafe`.
- `ps-service` exposes `UpdateService` to the UI. `photosquirrel-cli` uses `ps-update` directly. The UI crates
  depend on neither `ps-update` nor any HTTP client.
- xtask gains `minisign` (signing) for `cargo xtask update-manifest`. It is not a dependency of any shipped binary.
- `cargo xtask layering` puts `ps-update` in the I/O layer.
- `PHOTOSQUIRREL_RELEASE_BUILD=1`, set only by `cargo xtask package`, marks release builds. Without it,
  `ps-update` refuses network calls.

## Alternatives considered
- **`cargo-packager-updater`** - no `.deb`/archive handling, rollback, catalog guard, process lock or modes. It also
  brings `reqwest` to the path the UI uses.
- **`self_update`** - it replaces a single binary and does not handle bundles or installers.
- **Reusing `ps-llm`'s `reqwest` client** - it would tie update code to the AI crate and its async runtime.

## Consequences
- New third-party notices for `minisign-verify` and the platform TLS verifier's crates (MIT, Apache-2.0, ISC),
  checked by cargo-about (ADR-0058, 0060).
- The updater runs on its own thread, with no runtime to start at launch.

## Revisit when
The app adopts an async runtime in the service layer, or another feature needs a shared HTTP client.
