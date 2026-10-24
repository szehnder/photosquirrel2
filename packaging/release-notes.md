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

### What's new in 0.2.0

- **Optional, reversible cleanup.** Move the files of a drive that are already safe in your library into a
  quarantine folder on that drive, choose which folders and files go, and undo a whole run or one folder of it.
- **Better capture dates.** Dates in folder names such as `2019-07-04 Beach` or `2019-07 Italy` (month only, never
  shown as a day), capture days in your time zone or the camera's recorded offset, and existing catalogs re-dated
  on the next scan without reading any file again. Libraries you already exported keep their layout, and nothing is
  copied twice.
- **The CLI pauses on battery** and resumes on mains (`--ignore-battery` to keep going).
- Clearer keeper reasons, toolbar and badges in the duplicate review.

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
