Native installers for macOS, Windows and Linux. You don't need Rust or any developer tools to try PhotoSquirrel;
download the file for your system below.

| System | Download | Notes |
|---|---|---|
| macOS 11+ (Apple silicon and Intel) | `PhotoSquirrel-{{version}}-macos-universal.dmg` | Open it and drag PhotoSquirrel to Applications |
| Windows 10/11 (x64) | `PhotoSquirrel-{{version}}-windows-x86_64-setup.exe` | Installs for your user; no admin rights needed |
| Linux, any distro (x64) | `PhotoSquirrel-{{version}}-linux-x86_64.AppImage` | `chmod +x` it, then run it |
| Debian, Ubuntu (x64) | `PhotoSquirrel-{{version}}-linux-x86_64.deb` | `sudo apt install ./PhotoSquirrel-{{version}}-linux-x86_64.deb` |
| Anything else | `.tar.gz` / `.zip` | Both programs, unpacked and ready to run |

Every package contains the desktop app (`photosquirrel`), the headless CLI (`photosquirrel-cli`) and the licence
notices of the open-source components they include (`THIRD-PARTY-LICENSES`). Each file has a
`.sha256` beside it; check it with `sha256sum -c <file>.sha256` (on macOS, `shasum -a 256 -c`).

### HEIC and AVIF photos

PhotoSquirrel decodes HEIC/HEIF (iPhone photos) with your system's own decoder and ships no HEVC decoder of its
own. Run `photosquirrel-cli doctor` to see what your machine has, and the exact command for your system.

- **macOS:** works out of the box (Apple's ImageIO); nothing to install.
- **Windows 10 (version 1809 or newer) and 11:** install the free
  [HEIF Image Extensions](https://apps.microsoft.com/detail/9pmmsr1cgpwg) and an HEVC decoder: the
  [HEVC Video Extensions](https://apps.microsoft.com/detail/9nmzlz57r3t7) ($0.99), or, if your PC maker provides it,
  the free [HEVC Video Extensions from Device Manufacturer](https://apps.microsoft.com/detail/9n4wgh0z6vhq). On a
  Windows "N" edition, add the Media Feature Pack first (Settings > Apps > Optional features).
- **Linux:** install libheif 1.12 or newer with its HEVC decoder:
  - Ubuntu 24.04+, Debian 13+: `sudo apt install libheif1 libheif-plugin-libde265`
  - Ubuntu 22.04, Debian 12: `sudo apt install libheif1`
  - Fedora (its own libheif has no HEVC decoder): enable RPM Fusion free
    (`sudo dnf install https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm`),
    then `sudo dnf install libheif-freeworld`
  - Arch, Manjaro, EndeavourOS: `sudo pacman -S --needed libheif libde265`
  - openSUSE (its libheif has no HEVC decoder): add the
    [Packman repository](https://en.opensuse.org/Additional_package_repositories#Packman) and install `libheif1`
    and `libheif-ffmpeg` from it
  - Anything else: libheif 1.12 or newer with an HEVC (libde265) decoder from your distribution
- **AVIF:** works everywhere, with no install (a bundled AV1 decoder, rav1d).

Without a decoder, HEIC photos are still found, catalogued, de-duplicated as exact copies and exported; they show
"No HEIC decoder on this system" instead of a preview, and the scan summary says how to enable previews. After the
install, restart PhotoSquirrel and rescan: only those photos are read again, and they get previews and are matched
to their resized or re-exported JPEG copies.

### What's new in 0.3.0

- **TIFF photos** are fully supported: scans, previews and look-matching, including large and tiled TIFFs, in
  bounded memory.
- **HEIC through your system's decoder**, with guidance for your system when a decoder is missing (see above).
- **AVIF** is decoded everywhere by a bundled AV1 decoder.
- **Previews filled in after an install:** once a decoder is available, the next rescan reads only the photos that
  waited for it and gives them previews, perceptual hashes and duplicate matching.
- **Better copies are offered, never copied twice:** when a photo already in your library gets a better copy later
  (a HEIC decoded at last, another keeper you chose, a better copy on a new drive), the export copies nothing for it
  and lists it as a better copy; `export run --upgrade` (or the option in the Export view) copies it beside the
  library copy, which is never removed or replaced.
- **The Excluded view has filters:** every excluded file has one category (yours, an accepted AI suggestion by kind,
  thumbnail caches, graphics, too small, could not be read, not a supported file) with a chip and a count; files
  that are not photos at all are hidden until you ask. Search, sort, List and Grid modes with thumbnails, and put
  back the selection or everything shown in one step (`report excluded` on the command line).

### LGPL components

{{lgpl}} (LGPL-2.1) is linked statically into both programs. Its source is attached to this release as a `.crate`
file (a gzipped tar, also on [crates.io](https://crates.io/crates/rawler)), and the complete source of this
release, with the `Cargo.lock` and Rust toolchain it was built with, is the
[`v{{version}}` tag](https://github.com/szehnder/photosquirrel2/tree/v{{version}}). To relink against a modified
copy, unpack both, point the workspace at your copy with `[patch.crates-io]` and run
`cargo build --profile dist`; `THIRD-PARTY-LICENSES` has the details.

The Linux AppImage also starts with the AppImage runtime (the program that mounts the image), which links libfuse
(LGPL-2.1) statically, together with musl, squashfuse, zstd and zlib. Attached to this release
({{runtime_sources}}): the libfuse source it was built from and the patch the runtime applies to it, and, to relink
the runtime against a modified libfuse, the source of the runtime itself and of squashfuse. The AppImage's
`THIRD-PARTY-LICENSES` names every component of the runtime with its version and licence and says how to relink.

### First launch

These builds are not code-signed yet, so the first launch shows a warning:

- **macOS** says the app "can't be opened" or "is damaged". Right-click PhotoSquirrel in Applications and choose
  **Open**. If that doesn't work, go to **System Settings → Privacy & Security** and click **Open Anyway**. Or
  run `xattr -dr com.apple.quarantine /Applications/PhotoSquirrel.app`.
- **Windows** SmartScreen says it "protected your PC". Click **More info → Run anyway**.

### Requirements

- **Linux:** glibc 2.35 or newer (Ubuntu 22.04+, Debian 12+, Fedora 36+), a Vulkan driver (Mesa is fine), and
  a Secret Service keyring such as GNOME Keyring or KWallet for saved API keys (optional).
- **x86_64:** a CPU from about 2009 or later (x86-64-v2: SSE4.2 and POPCNT).
- **Optional:** `ffmpeg` on your PATH for video poster frames; [LM Studio](https://lmstudio.ai) with a vision model
  for local AI insights.

PhotoSquirrel never modifies or deletes your source files: it copies the best copies into a library folder you
choose. The optional cleanup moves files that are already safe in your library into a quarantine folder on the same
drive, only after you confirm, and every move can be undone. See the
[README](https://github.com/szehnder/photosquirrel2#readme).
