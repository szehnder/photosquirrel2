# 0060. Release packages redistribute only what their notices and source offers cover

- Status: accepted
- Date: 2026-10-21
- Deciders: maintainers
- Amends: [0058](0058-third-party-licence-notices-via-cargo-about.md)

## Context
A review of the first release dry run found three things the packages carry, or need, beyond what ADR-0058's
`THIRD-PARTY-LICENSES` covers:
- `rawler` 0.8.0 (LGPL-2.1) is linked statically into both programs. ADR-0058 said the public source and the
  included LGPL text were enough; LGPL-2.1 section 6 also asks the distributor to say where the corresponding
  source of the work and of the library is, and to let users relink against a modified library.
- The AppImage bundled 14 shared libraries from the build image (linuxdeploy's default), among them LGPL
  `libgcrypt` and `libsystemd`, with no notices or source offer for any of them. Every one of them is present on
  any glibc desktop the packages support, and the .deb already depends on the system's.
- The Windows programs import `VCRUNTIME140.dll`, which Windows does not include and the packages did not ship,
  so they fail to start on a machine without the Visual C++ Redistributable.

Also, ADR-0058 says the dependency graph has "some 25 licences": it has 13 (0BSD, Apache-2.0, BSD-2-Clause,
BSD-3-Clause, CC0-1.0, CDLA-Permissive-2.0, IJG, ISC, LGPL-2.1, MIT, MPL-2.0, Unicode-3.0, Zlib).

## Decision
A package contains only third-party code that its notices name and, where the licence asks for it, whose source
it offers:
- **LGPL components.** `about.toml` accepts LGPL-2.1 for `rawler` alone (a per-crate `accepted`), not globally,
  so any other LGPL crate fails packaging until it gets the same treatment. `THIRD-PARTY-LICENSES` starts with an
  "LGPL components" section: the component and version, that it is linked statically, its source on crates.io,
  the complete source of the release at `https://github.com/szehnder/photosquirrel2/tree/v<version>` (with
  `Cargo.lock` and `rust-toolchain.toml`), and how to relink (`[patch.crates-io]` pointing at the modified copy,
  then `cargo build --profile dist`). `cargo xtask package` writes the LGPL crates to
  `target/packages/lgpl-sources.txt`; the release job downloads each one's `.crate` from crates.io, checks it
  against the `Cargo.lock` checksum and attaches it to the release; a missing source fails the release job. The
  README and the release notes say the same in short.
- **No bundled system libraries in the AppImage.** The packager's `appimage.excludedLibs` excludes the libraries
  linuxdeploy would copy; the AppImage uses the system's, like the .deb. Packaging fails if an extracted AppImage
  contains a `.so` that `THIRD-PARTY-LICENSES` does not name, if its programs have an unresolved library (`ldd`
  in the build image) or if its CLI does not run.
- **Static C runtime on Windows.** `.cargo/config.toml` adds `-C target-feature=+crt-static` for
  `x86_64-pc-windows-msvc`; packaging reads the import table of both `.exe` files and fails if either imports
  `VCRUNTIME*` or `MSVCP*`.

MPL-2.0 components (`dwrote`, `option-ext`) are used unmodified; the notices say every component's source is on
crates.io at the listed version.

## Alternatives considered
- **Drop rawler** - loses RAW previews and metadata for most camera formats; no permissive Rust decoder of the
  same coverage exists.
- **Bundle the AppImage libraries and append their Debian `copyright` files and a jammy source-package offer** -
  more to keep in step for libraries every target system already has.
- **Ship `vcruntime140.dll` or the VC++ Redistributable** - an app-local DLL or an installer step; the static
  CRT is simpler and adds a few hundred KB.

## Consequences
- A new LGPL dependency, or a library linuxdeploy starts bundling, fails packaging until it is reviewed.
- The AppImage needs the system's D-Bus, xcb and xkbcommon libraries (documented in the README requirements).
- libheif-sys looks for a Windows vcpkg libheif in the static-CRT triplet (`x64-windows-static`), relevant once
  ADR-0059 is revisited.

## Revisit when
rawler changes its licence or is replaced, the AppImage has to carry a library desktops lack, or the Windows
build moves to a toolchain without a static CRT.
