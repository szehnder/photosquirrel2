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

PhotoSquirrel never modifies or deletes your source folders: it copies the best copies into a library folder you
choose. See the [README](https://github.com/szehnder/photosquirrel2#readme).
