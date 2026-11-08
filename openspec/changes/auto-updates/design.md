# Design

## Context

See proposal.md for the motivation and the owner's three choices. The current state that shapes the design:

- **Packages (ADR-0030, `xtask/src/package.rs`).** Seven artifacts per release:
  - Linux: AppImage, `.deb` (package `photo-squirrel`), `.tar.gz`.
  - macOS: universal `.dmg`, `.tar.gz`. This `.tar.gz` holds the bare binaries, not the `.app` bundle.
  - Windows: NSIS `-setup.exe` with `installMode = "currentUser"` (it installs under `%LOCALAPPDATA%`, no admin),
    `.zip`.

  None of them is OS-signed. The release job checks for exactly 7 packages and 7 `.sha256` files, holds the only
  write token and compiles nothing.
- **Privacy rules.** ADR-0029 forbids update pings, and its "Revisit when" says "never without a new ADR". The
  app-shell "Quiet start" test (`crates/ui-workspace/tests/network.rs`) asserts there are no sockets after 10 s idle,
  and that `ui-workspace/Cargo.toml` names no HTTP client.
- **Start-up order (`crates/photosquirrel/src/main.rs`).** `CatalogBackend::warm` opens the catalog, and runs its
  migrations, on a background thread while the window is being created. A migration can therefore finish before the
  first frame.
- **Catalog version guard.** `ps-catalog` refuses a catalog written by a newer build without modifying it
  (`db::refuse_newer`, `PRAGMA user_version`). The owner lock is `std::fs::File::try_lock` (ADR-0041).
- **Settings.** The file is at version 3 (`ps_model::settings::SETTINGS_VERSION`). The running app owns
  `settings.toml` (ADR-0049). Unknown keys are ignored when loading.
- **Dependencies already locked:** `ureq` 3 (rustls, used by xtask), `reqwest` (ps-llm, async), `semver`, `flate2`,
  `blake3`, `rustix`, `windows-sys`, `directories`.

## Goals / Non-Goals

**Goals:**
- A user who said yes once ends up on the latest release within about a day of it being published, plus one quit,
  on every package kind that can update itself. Every other user is told, with one obvious next step.
- No update can ever be installed unless the project's key signed it. A network attacker, a compromised CDN or a
  replayed old manifest can at worst delay an update.
- A failed install or a new version that cannot start leaves the user on a working version, without them doing
  anything, and tells them what happened.
- Nothing is sent before consent. After consent, a request says nothing about the user.
- Everything can be checked headless: the CLI and fake install layouts in temporary folders on all three CI OSes.

**Non-Goals (design level):**
- Any process that runs with elevated rights. Updating a `.deb` stays the user's `sudo apt` step.
- Updating the CLI on its own, apart from the app. They always ship and update together.
- Keeping more than one previous version.

## Decisions

### D1. Consent and the update mode (ADR-0124, amends ADR-0029)

`[updates] mode = "undecided" | "automatic" | "notify" | "off"` arrives with settings version 4. Upgrading from v3
sets `undecided`. The mode is deserialized leniently: an unknown value fails validation for that one key and
behaves as `undecided`, so a bad value does not throw away the rest of the file through corrupt-file recovery.

**Where the question is asked:**
- The onboarding view (empty catalog) shows a card: "Keep PhotoSquirrel up to date". It reads: "Once a day
  PhotoSquirrel asks GitHub whether a newer version exists. It sends nothing about you or your photos. Updates are
  checked for the PhotoSquirrel signature before they are installed, and install when you quit." The buttons are
  **Keep me up to date** (primary, focused) and **No thanks**.
- Users upgrading with a non-empty catalog get the same card once, as a dismissible banner above the central view.
  Closing it without choosing counts as "Ask me later": it is shown again at the next start, three times at most,
  and then the mode is `off`. Nothing is sent while the mode is undecided.

There is no other path to network access. "Check for updates" in Settings, the command palette and the CLI is an
explicit user action, allowed in every mode except a developer build (D9). This keeps ADR-0029's rule: only what
the user turned on or started.

Alternatives considered:
- *On by default.* It breaks the README's "no telemetry" promise, and ADR-0029 exists for exactly that promise.
- *Off by default.* Most users would never turn it on, which defeats the goal.

### D2. Check policy and the request (ADR-0124)

The schedule:
- The first check runs at least 60 s after start, so the "Quiet start" window stays silent.
- After that, a check runs when `now >= last_completed_check + 24 h + jitter`, where jitter is drawn once per
  completed check, uniformly from 0 to 60 min. Spreading the checks keeps a release from being hit by every client
  at the same minute.
- A timer on a background executor wakes once an hour to compare the clock. The UI thread never waits on it.

The request:
- `GET https://github.com/szehnder/photosquirrel2/releases/latest/download/update-manifest.json`. The address is
  built in as a constant.
- Headers: `User-Agent: PhotoSquirrel` and `Accept: application/json`. No cookie jar, no query string, no version.
  GitHub's `latest` already skips prereleases and drafts, which gives a stable channel for free.
- HTTPS only, with rustls and the platform's root certificates, so corporate TLS inspection that adds a root to the
  OS keeps working. Integrity does not depend on TLS (D3).
- The system proxy from `HTTPS_PROXY`/`https_proxy` is honoured.
- Up to 5 redirects, each to an `https` URL whose host is on the release-host allowlist: `github.com`,
  `objects.githubusercontent.com`, `release-assets.githubusercontent.com`. Task 2.2 confirms the current GitHub
  redirect hosts with a live request before the list is fixed in code. Anything else fails with "redirect to an
  untrusted host". This is narrower than ADR-0047's "no redirects" for AI, because GitHub release downloads always
  redirect to signed CDN URLs.
- Limits: connect 15 s, idle read 30 s, at most 64 KiB for the manifest, and at most the manifest's `size` for a
  payload. Every limit is enforced while reading, not afterwards.

**Test server seam.** Tests inject a `Transport`. Every build also honours
`PHOTOSQUIRREL_UPDATE_SERVER=http://127.0.0.1:<port>`, which accepts loopback only. It replaces the origin of the
manifest address (`<server>/update-manifest.json`) and of every payload (`<server>/<name>`). The signature, hash,
size and version rules all still apply, so the override can only ever deliver genuinely signed files. The release
round trip (D10) and the release candidates (D11) need it. Its use is logged and shown in `update status`.

**What a check sends:** an IP address and a TLS handshake to GitHub, the same as opening the release page in a
browser. The README says so.

### D3. The signed manifest (ADR-0125)

`update-manifest.json` is an envelope: `{"format": 1, "manifest": "<exact JSON text>", "signature": "<minisign
signature file text>"}`.

The client:
1. Parses the envelope.
2. Verifies the minisign signature over the UTF-8 bytes of `manifest` against each built-in key in turn
   (`minisign-verify`, Ed25519, prehashed `ED`).
3. Requires the signature's trusted comment to equal `photosquirrel-update <version>`.
4. Only then parses `manifest`.

Keeping both in one file means a check is one request, and the manifest and signature cannot be fetched from two
different "latest" releases during a publish. People can still check it with stock tools; `docs/releasing.md` gives
the `jq` and `minisign -V` lines.

The manifest's own fields:

```json
{
  "product": "PhotoSquirrel",
  "version": "0.4.1",
  "released": "2026-12-01T12:00:00Z",
  "urgent": false,
  "notes": "markdown, <= 4000 chars, from 'What's new in 0.4.1'",
  "notes_url": "https://github.com/szehnder/photosquirrel2/releases/tag/v0.4.1",
  "packages": {
    "linux-x86_64":    { "appimage": {..}, "deb": {..}, "tar.gz": {..} },
    "macos-universal": { "app.tar.gz": {..}, "dmg": {..}, "tar.gz": {..} },
    "windows-x86_64":  { "setup": {..}, "zip": {..} }
  }
}
```

Each package entry is `{ "name", "size", "sha256", "url" }`, where `url` must be
`https://github.com/szehnder/photosquirrel2/releases/download/v<version>/<name>`. The client checks that prefix as
well. Unknown fields are ignored, so `channel` or `rollout` can be added later without bumping `format`.

**Version rules:**
- Compared with `semver` precedence.
- Offered only when `version > installed` and `version >= highest_accepted`.
- `highest_accepted` is updated after each verified manifest. A replayed older manifest, or a frozen `latest`,
  therefore cannot move a client backwards or hide a version it has already seen. Being frozen on the current
  version is the remaining risk.
- A version with a pre-release part is offered only to an installation whose own version has one.
- Versions in `skipped` (D7) are never offered automatically.

**Keys:**
- Two slots, `UPDATE_KEYS: [&str; 2]`, are built in from `packaging/update-key.pub` and
  `packaging/update-key-next.pub`. The second file may say `none`.
- The release key is a minisign key generated by the owner. Its encrypted secret and password are stored as secrets
  of the GitHub environment `release-signing`, which has the owner as required reviewer and allows only `v*` tags.
  An offline backup is the owner's (runbook).
- Test builds add a third key, the test key, only under `cfg(test)` or the `ps-update/test-key` feature. The
  packaging check (D10) fails if a release binary contains the test public key, or lacks the release keys.

Rotation: release N ships the new key in the second slot. Once N has been out long enough (the runbook says two
releases), CI switches to signing with it, and the following release moves it to the first slot.

If the key is exposed, the next release must rotate. A user still trusts the exposed key until they update, and the
runbook says so plainly. If the key is lost, updaters cannot be moved forward, and users must install one release by
hand that carries a new key. The release notes and the README say how.

Alternatives considered:
- *A detached `.minisig` next to the manifest.* That is two requests, with a race at publish time.
- *TUF.* It solves the freeze and key-compromise problems properly, but costs a lot more infrastructure and code than
  a single-maintainer project can run. Its main ideas (signed metadata, a highest-version floor, key rotation) are
  borrowed here.
- *Relying on HTTPS alone.* GitHub or a CDN would then be a single point of compromise.

### D4. Download and staging

The update folder is per user and per installation, independent of `--data-dir`:
- Linux: `$XDG_DATA_HOME/photosquirrel/updates`
- macOS: `~/Library/Application Support/PhotoSquirrel/updates`
- Windows: `%LOCALAPPDATA%\PhotoSquirrel\updates`

It holds:
- `state.json`: written atomically, by a temp file then a rename.
- `history.jsonl`: the last 200 lines.
- `download/<name>.part`
- `ready/<version>/`
- `previous/<version>/`
- `helper/`
- `installs/<blake3 of install path>.lock`

The download runs on a dedicated background thread at low priority (Linux and macOS `nice`/QoS background, Windows
`THREAD_MODE_BACKGROUND_BEGIN`):
- One connection, resumed with `Range: bytes=<len>-` when a `.part` file exists and the server answers 206.
- It is streamed through SHA-256, so the file is never read twice.
- It needs free space of at least 3 times the size: the download, an extracted copy, and the previous version.
- When the size and SHA-256 match, it is renamed into `ready/<version>/`. If they do not match, the file is deleted
  and `history` records it.
- Only one version is staged at a time. Staging a newer one removes the older one.

In `notify` mode nothing is downloaded until the user chooses to install. For a `.deb` the payload is downloaded and
verified into the update folder only after the user asks, then copied into the user's Downloads folder.

### D5. Installation kinds (ADR-0126)

Detected at run time from the running executable (`std::env::current_exe`, canonicalized):

| Kind | Detected by | Self-installs | Payload |
|---|---|---|---|
| `appimage` | `$APPIMAGE` set, points to an existing regular file, and `$APPDIR` contains the running exe | if the AppImage's folder is writable | `.AppImage` |
| `macos-app` | exe at `<X>.app/Contents/MacOS/photosquirrel` | if `<X>.app`'s parent is writable, the path has no `/AppTranslocation/`, and the volume is not read-only (`statfs` `MNT_RDONLY`, so not the DMG) | `.app.tar.gz` |
| `windows-setup` | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<packager key>` `InstallLocation` equals the exe's folder | if that folder is writable | `-setup.exe` |
| `deb` | exe under `/usr/` and listed in `/var/lib/dpkg/info/photo-squirrel.list` | never | `.deb` (download and command) |
| `archive` | anything else built by packaging | never | link only |
| `developer` | built without `PHOTOSQUIRREL_RELEASE_BUILD=1`, which only `cargo xtask package` sets | never checks | none |

The CLI reports the same kind for the installation it belongs to. On macOS the CLI is inside the bundle
(`Contents/MacOS/photosquirrel-cli`). Task 4.1 confirms where cargo-packager puts it.

Kinds that cannot install themselves give a reason, which `update status` and Settings show:
- "Installed by the system package manager"
- "Running from the disk image; drag PhotoSquirrel to Applications first"
- "macOS moved the app to a temporary location; move it to Applications"
- "No permission to write <folder>"
- "Unpacked archive"
- "Developer build"

### D6. The installer helper and the swap (ADR-0126)

**When it starts.** Installing starts from one of two places:
- **On quit**, in automatic mode or after "Install on quit". This applies when an update is ready, the kind can
  install itself, and no file-changing operation holds the ADR-0065 slot. A scan that is still running is cancelled
  and left resumable, as on every quit.
- **Restart now.** It needs the ADR-0065 slot to be free, which is the spec's "Available when the scan finishes".

Either way the app writes `ready/<version>/plan.json`. The plan records the kind, the install path, the payload, the
expected version, the current version, the newest catalog format this version opens
(`ps_catalog::LATEST_SCHEMA`), and whether to relaunch.

**The helper.** The app copies its own installed CLI into `helper/<current version>/`. The old, known-good code
does the install, and is not inside the files being replaced. The app spawns
`helper/.../photosquirrel-cli update finish --plan <plan> --wait-pid <pid>` detached:
- Unix: `setsid` through `process_group(0)`, stdio set to `/dev/null`.
- Windows: `DETACHED_PROCESS | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB`.

Then the app quits as usual. The helper logs to `history.jsonl` and to `updates/helper.log`.

**The per-installation running lock.** Every app and CLI process of an installation takes a shared lock on
`installs/<hash>.lock` at start. That is one open and one `lock_shared`, with no measurable cost. The helper waits
for the parent PID to exit, then for an exclusive lock on that file. It retries every 250 ms for up to 10 min. On
timeout it writes "postponed" and exits, and the update stays ready.

**Steps, per kind.** Each step is idempotent, and the plan records its progress, so a killed helper resumes or
undoes the work:

- **appimage**
  1. Copy the current AppImage to `previous/<cur>/` and check its hash.
  2. Copy the payload to `<dir>/.<name>.update` in the same folder, `fsync`, set mode 0755.
  3. Smoke test: `.<name>.update --version` (the main binary, through the AppImage runtime), plus the CLI check
     that task 5.1 settles (see Risks).
  4. `rename` it over the original path. This is atomic. The user's launcher, desktop file and AppImageLauncher
     integration keep working because the path stays the same.
  5. The file name keeps the old version number. This is documented.
- **macos-app**
  1. Extract the `.app.tar.gz` into `<parent>/.PhotoSquirrel-update-<ver>/PhotoSquirrel.app`, on the same volume
     as the installed bundle.
  2. Check that its `Contents/MacOS/photosquirrel{,-cli} --version` print the version. When the installed bundle is
     Developer ID signed (change `macos-signing`, ADR-0128), the staged bundle must also pass `codesign --verify
     --strict` with the same TeamIdentifier, otherwise the install is refused and the version is skipped.
  3. `renamex_np(installed, staged, RENAME_SWAP)`, through `rustix` or `libc`. This is one atomic swap.
  4. Move the old bundle, now at the staging path, into `previous/<cur>/`: a rename on the same volume, otherwise a
     copy and then a removal of the staging copy only. Remove the hidden staging folder.
  5. If the swap fails with `EPERM`/`EACCES` (App Management, or a parent owned by another user), copy the verified
     payload to `~/Downloads/PhotoSquirrel-<ver>.app.tar.gz`. That is the D8 fallback. Record the version as
     `fallback_offered`, so it is not retried.
- **windows-setup**
  1. Copy the installed files listed by the installer (`photosquirrel.exe`, `photosquirrel-cli.exe`,
     `THIRD-PARTY-LICENSES`, resources) to `previous/<cur>/`, plus the uninstall key's `DisplayVersion`.
  2. Run `<ready>/<setup>.exe /S` and wait for exit code 0. Task 6.3 checks that cargo-packager's NSIS template
     honours `/S`, keeps the install location, and does not start the app.
  3. Check that the installed exes print the version and that `DisplayVersion` matches. The downloaded installer
     gets no Mark-of-the-Web, because the updater writes the file itself, so SmartScreen does not stop the silent
     run.

**After the smoke test.** If it fails, the kept copy goes back (appimage: rename the previous copy over the file;
macos: swap back; windows: copy the files and the `DisplayVersion` back), the version goes into `skipped`, and the
history records why. If it passes, `state.trial = { version, starts: 0, ok: false }` is set and the kept version is
recorded as `kept = { version, path, max_catalog_schema }`. The previous kept copy, two versions back, is removed
only now.

The helper relaunches the app only for Restart now.

### D7. Trial starts and automatic rollback (ADR-0126)

**Early check.** `photosquirrel` `main()`, right after argument parsing and before GPU selection, reads
`state.json`. That is one small file read, measured within the cold-start budget (task 7.4). If
`trial.version == current && !trial.ok`, it increments `trial.starts` and writes the file.

**If `starts >= 3`** (two earlier trial starts did not succeed):
- If the catalog guard allows it (D7a), the app spawns the helper for a rollback to `kept`, with relaunch, and exits.
- If not, it carries on and shows "PhotoSquirrel <new> had trouble starting. Your catalog was already upgraded, so
  going back is not possible. Please report it: <issues link>".

**Holding the catalog back.** On a trial start, `CatalogBackend::warm` is not called before the first frame.
`ui-workspace` signals the first frame (it already has a hook used by `--measure-startup`). The service then records
`trial.ok = true` on a background thread, and only then opens and warms the catalog. This costs one first-gallery
frame's worth of catalog open on the first start after an update only. The first-gallery-frame budget is measured on
non-trial starts, so it is unchanged.

**After a successful trial:**
- A one-time toast, "Updated to <version>", with **What's new**, which opens `notes` in a dialog with
  `notes_url`.
- `highest_accepted` is kept.
- `ready/` is emptied.

**D7a. The catalog guard.**
- Each app and CLI start of the installation appends its data directory's canonical path to `state.catalogs` (a set,
  at most 16 entries).
- Rollback, automatic or manual, reads `PRAGMA user_version` read-only from every listed catalog that exists, through
  the same read-only path `refuse_newer` uses.
- It refuses when any of them is greater than `kept.max_catalog_schema`, and names the catalog.
- Settings v4 files are read by older versions, which ignore unknown keys. ADR-0049 then rewrites the file without
  `[updates]`, so after a rollback the mode comes back as undecided and the card is asked again. That is acceptable,
  and documented.

**Manual rollback.** The Settings button "Go back to <kept>" and `update rollback` refuse while a program runs, and
use the same helper and the same guard. They add the newer version to `skipped`. The next newer release is offered
normally.

### D8. User-facing surfaces (UI)

All of these use ui-theme tokens (ADR-0028), with no per-file events (ADR-0019). Update state reaches the UI as one
coalesced `UpdateStatus` snapshot from `ps-service`.

| State | Surface | Actions |
|---|---|---|
| Undecided, first launch | Onboarding card | Keep me up to date / No thanks |
| Undecided, upgraded | Dismissible banner | same |
| Ready (automatic) | Status-bar item "Update ready", plus a banner once per version | Restart now (disabled with a reason while the ADR-0065 slot is held), What's new, Later |
| Available (notify) | Banner | Install and restart / Install on quit / What's new / Skip this version |
| Available, cannot install itself | Banner, once per version | Download (deb: verify and reveal, then show the command with Copy) / Release page / What's new |
| Urgent | The same banners, not auto-dismissed, shown each start until acted on or dismissed for that version | as above |
| Updated | Toast | What's new |
| Install or rollback failed | Banner | Details (opens the history) |
| Three days of failed scheduled checks | A status-bar note only | Check now |

The Settings > General > Updates group follows the settings spec. "Check for updates…" is in the command palette.
Every message uses the D5 reasons and names versions.

### D9. Crate and dependency boundaries (ADR-0127)

**New I/O crate `crates/ps-update`** (no gpui-kit). It depends on:
- `ps-model`: the settings and update state types.
- `ureq` 3, with rustls and `rustls-platform-verifier`, or `webpki-roots` if the platform verifier adds too much.
  Task 2.1 measures both. `ureq` keeps the updater free of tokio and lets it run on a plain thread.
- `minisign-verify`: verification only, no dependencies.
- `semver`, `sha2` (or the workspace's existing SHA-256 used by xtask's checksums), `serde_json`.
- `tar` + `flate2`, for macOS only, on `cfg(target_os = "macos")`.
- `rustix` (`renamex_np`, `statfs`) and `windows-sys` (registry, process creation flags).

**Other crates:**
- `ps-service` gains `UpdateService`: the schedule, the state, snapshots for the UI, the quit hook and the helper
  spawn.
- `photosquirrel-cli` gains `update` subcommands that call `ps-update` directly, plus the `doctor` fields.
- `ui-workspace` gets no HTTP client. The "Quiet start" manifest check keeps passing and is extended to
  `photosquirrel`.
- `xtask` gains `minisign` (the signing side), used only by `cargo xtask update-manifest`.
- `cargo xtask layering` adds `ps-update` to the I/O layer, and forbids it to the core crates and to `ui-*`.

**Licences:** `minisign-verify` and `minisign` are MIT, and `tar` is MIT/Apache. They pass the cargo-about config
(ADR-0058/0060) without new licence kinds. Task 2.1 checks this.

**Build flag:** `PHOTOSQUIRREL_RELEASE_BUILD=1` is set by `cargo xtask package` and read with `option_env!`. Without
it, `ps-update` reports `developer` and refuses every network call.

Alternatives considered:
- *`cargo-packager-updater`.* It shares the packager's minisign format and covers AppImage, NSIS and `.app`
  bundles, but has no `.deb` or archive handling, no trial or rollback, no catalog guard, no running-process lock and
  no notify/off modes. It also pulls `reqwest` into the UI-facing path. Wrapping it would mean rewriting most of it.
- *`self_update`.* It replaces a single binary; it does not handle `.app` bundles or installers.
- *Putting it in `ps-service`.* That would mix network code into the facade and make the layering rule harder to
  state.

### D10. Release pipeline

`cargo xtask package` changes:
- It also produces `PhotoSquirrel-<v>-macos-universal.app.tar.gz` (the `.app` from the DMG build, `tar` with fixed
  ordering) and its `.sha256`.
- It sets `PHOTOSQUIRREL_RELEASE_BUILD=1`.
- A new check runs on each platform: the built app and CLI must contain the release public keys and not the test
  key, found by a byte search for the base64 key strings.

New `cargo xtask update-manifest --packages <dir> --notes packaging/release-notes.md --tag v<x> [--sign-key-env
VAR]`:
- It builds the manifest from the packages and their `.sha256` files.
- It takes the notes from the "What's new in <version>" section. It fails if the section is missing or longer than
  4,000 characters.
- It signs the manifest and writes the envelope.
- `--check` re-verifies the envelope against `packaging/update-key*.pub`, or against `--pub` for a dry run, and
  against the packages.

`release.yml` changes:
- **`sign` job:** needs `package`, and also `verify-macos` whenever that job runs (change `macos-signing`), so the
  manifest hashes the signed macOS packages. `contents: read`.
  - For a pushed tag it runs in `environment: release-signing` and reads `UPDATE_SIGNING_KEY` and
    `UPDATE_SIGNING_PASSWORD`.
  - For a dry run it generates a throwaway key and uploads its public half.
  - It compiles only xtask (`--locked`) and uploads `update-manifest.json`.
- **`update-roundtrip` job** (matrix linux, macos, windows; tag only): needs `sign`.
  1. Downloads the previous non-prerelease GitHub release that has an `update-manifest.json`.
  2. Installs it: the AppImage into a temp folder; on macOS, `hdiutil attach` the DMG and copy the `.app` to a temp
     folder; on Windows, run the setup with `/S`.
  3. Serves the candidate's packages and signed envelope, unchanged, from `http://127.0.0.1:<port>/`.
  4. Runs `photosquirrel-cli update install --yes` from the old installation with `PHOTOSQUIRREL_UPDATE_SERVER`
     (D2), then checks the installed `--version`, a headless trial start (`photosquirrel --exit-after 5`, on a
     virtual display on Linux), and `update status` showing `trial ok`.
  5. If no previous release has a manifest (that is, for 0.4.0), it prints a notice and succeeds.

  Nothing is published before this passes.
- **`release` job:** needs `update-roundtrip` and still compiles nothing.
  - Its package count becomes 8.
  - It rechecks the envelope with the `minisign` CLI from apt (`minisign -V` over the extracted `manifest` text,
    with `packaging/update-key.pub`) and the manifest's sums with `sha256sum -c`. The full structural check
    (`xtask update-manifest --check`) already ran in the read-only `sign` job.
- `gh release create` attaches `update-manifest.json`.

The new `packaging/release-notes.md` "Updates" section explains automatic updates, what a check sends, turning them
off, `.deb` and archive users, and "this is the last version you install by hand" for 0.4.0.

The new `docs/releasing.md` covers:
- Cut a release (tag, the reviewer approval of `release-signing`, watching the round trip).
- Withdraw a bad release: mark it prerelease, or delete its `update-manifest.json` asset, so that `latest` falls back
  to the previous release; then ship a fixed version quickly. Clients already on it are not moved back, because there
  are no downgrades.
- Rotate the key.
- A lost or exposed key.
- Manual manifest verification (`jq -r .manifest`, `jq -r .signature`, `minisign -Vm`).

### D11. Release candidates prove the path before 0.4.0

The first version with the updater must be right, because a broken 0.4.0 updater can only be fixed by hand. Before
tagging 0.4.0 (owner tasks 9.x):
1. Tag `v0.4.0-rc.1` and `v0.4.0-rc.2`, both prereleases and signed with the real key.
2. Install rc.1 by hand on each OS (AppImage, a `.app` in `/Applications`, NSIS).
3. Serve rc.2's files locally with `PHOTOSQUIRREL_UPDATE_SERVER`, and check update, trial and rollback.
4. Repeat on macOS 14 or 15 with the bundle in `/Applications`, to see whether App Management prompts. That decides
   whether the D6 fallback is the normal macOS path until OS signing exists.

## Risks / Trade-offs

- **A broken updater in 0.4.0 strands every user.** Mitigated by the RC round trips (D11), the CI fake-layout tests
  on all three OSes, and the pre-publish round trip from the previous release from 0.4.1 on. The release notes keep
  the manual download path.
- **macOS App Management (13+) may prompt or refuse an unsigned bundle swap.** With `macos-signing` (ADR-0128) the
  bundles are Developer ID signed, and a same-Team-ID self-update is not stopped, so the fallback stays for unsigned
  installations only. For those, the D6 fallback never leaves a half-swapped bundle and is asked once per version.
- **The AppImage smoke test runs the GUI binary.** `--version` exits before any window opens (clap), and the
  AppImage's AppRun runs the main binary. Reaching the CLI inside the AppImage needs the runtime's
  `--appimage-extract-and-run` or the runtime's argv0 dispatch. Task 5.1 decides between: (a) the main binary's
  `--version` alone, plus an extracted squashfs check that `usr/bin/photosquirrel-cli --version` prints the version,
  and (b) a `PHOTOSQUIRREL_RUN=cli` dispatch in AppRun. Both are recorded in tasks.md.
- **The test server seam** (D2) is present in release builds, because the round trip has to exercise real
  binaries. It is limited to loopback and logged, and every signature, hash and version check still applies, so
  someone who controls the environment can serve only genuinely signed files.
- **A frozen `latest`** (a CDN or proxy serving an old valid manifest forever) is not detected. The status shows
  "last successful check", and "Check for updates" shows the version served. Accepted.
- **GitHub availability and rate limits.** A daily jittered GET of a release asset is not an API call, so it is not
  API rate-limited. Failures stay silent for three days (spec).
- **Corporate proxies and TLS inspection.** The platform verifier and `HTTPS_PROXY` handle the common cases.
  Otherwise the user sees the failure and can use `notify` with manual downloads.
- **Disk use.** The kept previous version is about 100-200 MB, depending on the platform, in the update folder. It
  is shown in Settings. Removing it is the same as declining rollback; there is no button in v1.
- **The settings round trip through an older version** drops `[updates]` (D7a). The user is asked again. Accepted.

## Migration Plan

- Settings v3 to v4: the mode is undecided. No catalog migration; the next free migration number stays 27.
- Users of 0.3.x: install 0.4.0 by hand (release notes), answer the card once, and from then on updates are
  automatic.
- Rollback of this change itself: a later release can switch off checking by shipping with the schedule disabled.
  Clients already on 0.4.x keep checking until they update to that release. Deleting `update-manifest.json` from the
  latest release (runbook) stops offers at once.

## Verification

```bash
cargo xtask ci
```

```bash
OPENSPEC_TELEMETRY=0 openspec validate auto-updates --strict
```

```bash
cargo xtask spec-coverage
```

```bash
cargo xtask layering
```

```bash
cargo test -p ps-update --test-threads=4
```

```bash
cargo test -p ui-workspace --test network
```

```bash
cargo run -p photosquirrel-cli -- update status --json
```

```bash
PHOTOSQUIRREL_UPDATE_SERVER=http://127.0.0.1:8765 cargo run -p photosquirrel-cli -- update check --json
```

```bash
cargo xtask update-manifest --packages target/packages --notes packaging/release-notes.md --tag v0.4.0 --check
```

```bash
cargo xtask bench --check
```

```bash
gh workflow run release.yml
```

- The `ps-update` tests use a local HTTP server and the test key, with fake install layouts per kind in a `tempfile`
  dir.
- `cargo xtask bench --check` must show cold start and idle memory unchanged.
- The `gh workflow run release.yml` dry run must show the sign job using a throwaway key and the release job seeing
  8 packages and a valid manifest.
