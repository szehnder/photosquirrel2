# 0073. The AppImage runtime is pinned and has its own notice (amends 0060)

- Status: proposed
- Date: 2026-10-22
- Deciders: maintainers
- Amends: [0060](0060-packages-redistribute-only-what-they-license.md)

## Context
Every AppImage starts with the AppImage type2 runtime, an ELF program that mounts the embedded squashfs
image. ADR-0060 made packages carry only third-party code their notices name, but it looked only inside the
image (`usr/lib*`), and the runtime sits in front of it. The runtime statically contains musl (MIT), libfuse
(LGPL-2.1), squashfuse (BSD-2-Clause), zstd (BSD-3-Clause) and zlib (Zlib); `THIRD-PARTY-LICENSES` names none
of them, and nothing offers the libfuse source.

The runtime is also not pinned. cargo-packager 0.11.8 downloads linuxdeploy and linuxdeploy-plugin-appimage
from release URLs that move (`continuous`), and the plugin's appimagetool downloads the runtime from
`AppImage/type2-runtime`'s `continuous` release at packaging time. The 0.1.0 dry run's AppImage holds a
944,632-byte runtime built from type2-runtime commit `fedfad5` with libfuse 3.15.0; the next release could
ship a different one without any review. The image also carries cargo-packager's `AppRun` launcher
(AppImageKit, MIT), fetched from a moving URL too and not named either.

## Decision
- `packaging/linux/appimage-runtime.toml` pins the runtime: its upstream commit, download URL and SHA-256,
  the components it contains (name, version, licence, licence text file under
  `packaging/linux/appimage-runtime/`), and the source archives the release attaches for its LGPL components
  (URL and SHA-256). It pins `AppRun` the same way.
- `cargo xtask package` downloads the pinned runtime, checks its SHA-256 and passes it to appimagetool through
  linuxdeploy-plugin-appimage's `LDAI_RUNTIME_FILE`, and pre-places the pinned `AppRun` in cargo-packager's
  tool cache so it is not fetched.
- After packaging, xtask reads the runtime out of the built AppImage (the ELF image up to the end of its
  section header table, where the squashfs starts), zeroes the sections appimagetool fills in (`.digest_md5`,
  `.upd_info`, `.sha256_sig`, `.sig_key`), and fails unless its SHA-256 equals the pin. It also reads the
  component list the runtime prints in its help text and fails unless it equals the pin's components.
- `THIRD-PARTY-LICENSES` gains an "AppImage runtime" section rendered from the pin: each component, its
  version and licence text, and where the libfuse source is. The release job downloads each pinned source
  archive, checks it and attaches it to the release, as it does for the LGPL crates (ADR-0060).

## Alternatives considered
- **Notice only, no pin** - the notice would describe whatever runtime the last person checked, not the one in
  the package; a silent upstream change would make it wrong again.
- **Build the runtime from source in the packaging image** - the most reproducible, but a static musl, libfuse
  and squashfuse build to maintain for one 1 MB file.
- **Vendor the runtime binary in the repository** - reproducible without the network, but a third-party binary
  in git; kept as the fallback if the upstream download URL for the pinned build stops being stable.

## Consequences
- An upstream runtime change no longer reaches a release unreviewed: packaging fails until the pin, the
  notice texts and the source offer are updated together.
- The AppImage notices cover everything the file contains, inside and in front of the image.
- The release carries one more source archive (libfuse, about 4 MB).

## Revisit when
The AppImage tooling changes how it obtains or embeds the runtime, the runtime gains or drops a component, or
the AppImage format is replaced.
