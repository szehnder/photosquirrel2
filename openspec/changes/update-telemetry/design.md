# Design

## Context

See proposal.md for the motivation. The state this design starts from (checked 2026-11-10, after `auto-updates`):

- **The promises.**
  - README: "No telemetry. The app sends nothing about you or your photos anywhere: no analytics and no crash
    reports. It checks for updates only if you said yes when it asked, and a check sends nothing about you."
  - ADR-0029: nothing is sent except LLM requests the user started. "Revisit when: never without a new ADR."
  - ADR-0124 amended that for one thing only: a signed-manifest `GET` to GitHub after the user's answer, with no
    version, identifier, cookie or query.
  - app-shell "Quiet start": no connection in the first 60 s, and afterwards only the update checks the user allowed,
    only to the update hosts. `crates/ui-workspace/tests/network.rs` proves it with recorded sockets and transports.
- **What the app already knows locally about update outcomes.**
  - `crates/ps-update/src/history.rs`: `history.jsonl`, the last 200 entries of `check`, `download`, `install`,
    `trial`, `rollback`, `postponed` and `failure`, each with a time, a version and a **free-text** `detail`.
  - `crates/ps-update/src/state.rs`: `CheckRecord { at, ok, .. }`, `Failure { version, current, message, reason }`
    with `FailureReason` = `install` | `start` | `start_kept`, and the trial counters.
  - `crates/ps-update/src/error.rs`: `UpdateError`, about 20 variants. Their messages are written for the user and
    can contain paths (`Fallback(path)`, `Io`) and host names (`UntrustedRedirect`, `Network`).
  - `crates/ps-service/src/updates.rs`: `UpdateStatus` (mode, installed version, `Kind`, last check, available,
    ready, kept, `Notice`), `TrialStart`, and `Notice::Updated` / `Notice::Failed` shown once after an update.
  - `ps_update::kind::Kind`: `AppImage`, `MacApp`, `WindowsSetup`, `Deb`, `Archive`, `Developer`.

  So every outcome worth counting already exists as a typed value at the moment it is recorded. Nothing has to be
  parsed out of text, and the text must never be sent.
- **Releases.** 8 packages plus `update-manifest.json` per GitHub release. `PhotoSquirrel-<v>-macos-universal
  .app.tar.gz` is fetched only by the updater; every other package is also a first-install download.
- **Build.** `[profile.dist]` has `debug = false` and `strip = "symbols"` (ADR-0030). `ureq` 3 with rustls and the
  platform verifier is already linked through `ps-update` (ADR-0127). Settings are at version 4.

## Goals / Non-Goals

**Goals:**
- The owner can answer, per release: how far has it spread, on which systems and package kinds does it run, and
  where do checks, downloads, installs and first starts fail.
- A user who changes nothing sends exactly what they send today. That is proven by tests, not only stated.
- A user who opts in can read every byte before and after it leaves, and stop with one click.
- No running cost while the project has no revenue, and no vendor that the shipped clients are tied to.
- Each stage can ship alone, and stage 0 needs no decision about privacy at all.

**Non-Goals (design level):**
- Exact rates, unique-user counts or per-installation funnels. Without an identifier these are not available, and
  that is the intended trade.
- Product analytics of any kind (see proposal, Out of scope).
- Reports from versions that cannot start, or from installations whose user said no. The data is a self-selected
  sample and is read as such.

## Decisions

### D1. Three stages, each useful alone

| Stage | Needs consent | Client change | Tells the owner |
|---|---|---|---|
| 0. Public counts | no | none | how many times each package and the manifest were fetched |
| 1. Health reports | own opt-in | yes | where the app runs, and how checks, downloads, installs and first starts end |
| 2. Crash reports | second opt-in | yes | where the app panics |

Stage 0 ships first and stays. Stage 1 depends only on `auto-updates`. Stage 2 depends on stage 1's transport,
collector and Privacy group, and should not start before stage 1 has passed its gate.

*Improvement on the brief:* the brief had stage 1 send an event at every app start. This design sends
`active_day` at most once per UTC day instead. Starts per day are a behaviour pattern, are not needed to answer
"which versions are in use", and would make the queue grow with use.

### D2. Stage 0: adoption from GitHub's public data

**What is read.** `GET /repos/szehnder/photosquirrel2/releases` (public REST API) lists every release with its
assets and each asset's `download_count`. The counts are running totals, so a time series exists only if someone
writes them down regularly.

**The command.** `cargo xtask release-stats --out <file.csv>` appends one row per asset:
`date,tag,asset,download_count`. It uses the `ureq` that xtask already has, sends `GITHUB_TOKEN` when it is set (in
Actions) and nothing otherwise, writes the file atomically, and leaves it untouched on any error. A header comment
in the file carries the limits text below. `cargo xtask release-stats --summary` prints, per release, total
downloads per package kind and the daily manifest fetches.

**The schedule.** A workflow `stats.yml` runs daily at an off-peak minute:
1. A job with a read-only token builds xtask and produces the new rows as an artifact.
2. A second job, which runs no cargo, appends them to `release-stats.csv` on an orphan branch `stats` and pushes.
   It is the only job with `contents: write`, and branch protection keeps it away from `main`. This is the same
   split as the release job (the job that holds a write token compiles nothing).

**What the numbers mean.**
- `update-manifest.json`: each scheduled check is one fetch of the latest release's manifest (ADR-0124), so its
  daily increase approximates "installations with updates on that ran today". It is an upper bound: manual checks,
  the release round trip, mirrors and bots also fetch it. Task 2.1 confirms that a fetch through
  `releases/latest/download/` raises the asset's `download_count`; if it does not, this signal does not exist and the
  report says so.
- `...macos-universal.app.tar.gz`: only the updater downloads it, so its count is "macOS self-updates that reached
  the download step" for that version.
- AppImage, `-setup.exe`, `.deb`: first installs and updates mixed. `.dmg`, `.tar.gz`, `.zip`: manual downloads.

**Limits, stated in the report header and the README of the `stats` branch:**
- A download is not an installation, and an installation is not a successful update or a working app.
- Counts include bots, mirrors, CI and repeated downloads. GitHub does not document how it counts.
- Nothing here shows a failure, an OS version or a rollback.
- GitHub disables scheduled workflows in repositories with no recent activity (60 days, to re-verify). A gap in the
  rows only loses resolution, because the counts are totals.

Alternatives considered:
- *GitHub's traffic API (views, clones).* It needs push access, covers 14 days and says nothing about releases.
- *A redirect service in front of downloads to count them.* It would put a project server into the update path,
  which ADR-0124 kept on GitHub alone. Rejected.
- *Committing the CSV to `main`.* It would need a write token near code and add a commit a day. Rejected.

### D3. Consent (ADR-0130)

**Settings (version 5):**

```toml
[telemetry]
health_reports = "undecided"   # "undecided" | "on" | "off"
crash_reports  = "off"         # "off" | "ask" | "always"   (stage 2)
```

Upgrading from v4 writes `undecided` and `off`. Both are deserialized leniently like `updates.mode`: an unknown
value fails validation for that key and behaves as `off`.

**The question.** One card, "Help make updates reliable", shown once as a dismissible banner above the central view:

> PhotoSquirrel can send a small anonymous report, at most once a day: its version, your operating system and
> whether updates worked. Nothing about you, your files or your photos, and no ID that could link one report to the
> next. **See exactly what is sent**
>
> [ Share anonymous reports ]   [ No thanks ]

Rules, each a spec scenario:
- Never in onboarding, never before the update question is answered, and never in a session in which the update
  question was shown. The earliest moment is therefore the second start. A first-time user is asked one thing at a
  time, and the two questions cannot be read as one.
- The two buttons have equal weight. Neither is focused, so Enter or Space cannot answer it by accident.
- Closing the card is "No thanks". Unlike the update card it never comes back.
- "See exactly what is sent" opens the view of D6, which before the opt-in shows an example.

*Improvement on the brief:* the brief said "off by default, never bundled". This adds *when* it may be asked
(not together with the update question, not on first launch), that the card has no default button, and that
dismissing equals no. The owner may still prefer no card at all, with the switch only in Settings: see Open
questions.

**Overrides.** `PHOTOSQUIRREL_TELEMETRY=0` or `DO_NOT_TRACK=1` in the environment force both choices off for that
process without changing the settings file. The Privacy group shows the reason and disables the switches.
`DO_NOT_TRACK` is an informal convention among command-line tools; honouring it costs nothing.

**No recording while off.** The queue does not exist while health reports are off or undecided. A user who opts in
on Tuesday sends nothing about Monday. Turning the switch off deletes `queue.jsonl` at once and sends nothing: an
"opted out" event would be a request made after the user said stop.

### D4. The event list and the report (ADR-0130)

The whole schema, version 1. `docs/telemetry.md` is its public form, and a test keeps the two equal.

**Report (one JSON document per send):**

```json
{
  "schema": 1,
  "os": { "family": "linux", "distribution": "ubuntu", "version": "24.04" },
  "arch": "x86_64",
  "kind": "appimage",
  "events": [
    { "name": "active_day", "day": "2027-03-02", "app": "0.5.1", "update_mode": "automatic" },
    { "name": "update_check", "day": "2027-03-02", "app": "0.5.1", "result": "available" },
    { "name": "update_download", "day": "2027-03-02", "app": "0.5.1", "target": "0.5.2", "result": "ok" },
    { "name": "update_install", "day": "2027-03-03", "app": "0.5.2", "previous": "0.5.1", "target": "0.5.2",
      "result": "ok" }
  ]
}
```

**Fields and their closed value sets:**

| Field | Values |
|---|---|
| `os.family` | `linux`, `macos`, `windows` |
| `os.version` | major.minor digits only (`15.1`, `24.04`); Windows `10` or `11`; absent when unknown or rolling |
| `os.distribution` | Linux only: a built-in list of about 15 common `os-release` IDs, else `other` |
| `arch` | `x86_64`, `aarch64` |
| `kind` | `appimage`, `mac_app`, `windows_setup`, `deb`, `archive` (never `developer`: those never send) |
| `day` | the UTC date the event was recorded |
| `app`, `previous`, `target` | a PhotoSquirrel version, `MAJOR.MINOR.PATCH` with an optional `-rc.N` |
| `update_mode` | `undecided`, `automatic`, `notify`, `off` |
| `result` | check: `up_to_date`, `available`, `failed`; download: `ok`, `failed`; install: `ok`, `failed`, `rolled_back` |
| `reason` | only with `failed` or `rolled_back`; a reason class below |

**Reason classes** (`reason`), each mapped from typed values, never from text:

| Class | From |
|---|---|
| `network` | `UpdateError::Network`, timeouts |
| `http_status` | `Status(_)` (the number is not sent) |
| `untrusted_redirect` | `UntrustedRedirect`, `TooManyRedirects`, `Insecure` |
| `signature` | `BadSignature`, `BadManifest` |
| `hash_mismatch` | `Mismatch`, `TooLarge` |
| `no_space` | `NoSpace` |
| `os_refused` | `Fallback` (macOS refused the bundle swap) |
| `version_check` | the new binary did not print the expected `--version` |
| `start_failed` | `FailureReason::Start` (rolled back) and `StartKept` (kept, result `failed`) |
| `user` | "Go back to <version>" chosen by the user (result `rolled_back`) |
| `io` | `Io` |
| `other` | everything else, including variants added later without a mapping |

`ps-update` gains `UpdateError::class() -> ReasonClass` with an exhaustive `match`, so a new error variant does not
compile until it has a class. `state::Failure` gains a `class` field (default `other`), because an install failure
is recorded by the helper process and read by the app at its next start.

**Where events come from.** Only the desktop app records, from values it already has:
- `active_day`: after the first frame, on a background thread, when the stored last active day is not today.
- `update_check`, `update_download`: where `UpdateService` records the same outcome into the history.
- `update_install`: at the next start, from `TrialStart`, `Notice::Updated` and `Notice::Failed`. A success is
  recorded by the new version after its first frame, the same moment ADR-0126 counts a trial as passed. A rollback
  is recorded by the restored version.
- For `.deb` and archive installations, which the app cannot update itself, `update_install` with result `ok` is
  recorded when the app sees that its version is newer than at its last recorded start.

The helper (`update finish`) and the CLI record nothing. They write the typed class into the update state, which
they do today for the message.

**Why these values cannot identify anyone.** There is no identifier, so reports cannot be linked. Every value comes
from a small set shared by many installations. Times are days. Rare Linux distributions collapse to `other`. The
spec's test builds reports on two installations that differ in everything personal and requires the bytes to be
equal.

**What is deliberately left out:** the locale (the brief says locale-free), the time zone, the hour, the window
system or GPU, the number of starts, how long the app ran, the size of anything, the HTTP status number, and the
update's download size or duration.

### D5. Queue, schedule and request

**Files**, in a `telemetry` folder beside the update folder (`PHOTOSQUIRREL_TELEMETRY_DIR` moves it for tests):
- `queue.jsonl`: one event per line. At most 200 lines and 30 days; older or surplus events are dropped unsent.
- `sent.jsonl`: the last 10 reports exactly as sent, with the day and the address.
- `state.json`: the last send attempt, the jitter, the last active day, the last seen version, and `stopped` (set
  by a `410`).

They are written atomically (write, then rename) by `ps-telemetry` only. They are the app's own files, like the
update folder, so the safety rule about user paths is not touched. `cargo xtask layering` lists the module in its
file-operation allowlist the same way it lists `ps-update`'s.

**Schedule.** The service shares `UpdateService`'s hourly background timer. A send is due when health reports are
on, the queue is not empty, at least 60 s have passed since start, and `now >= last_attempt + 24 h + jitter` (0-60
min, drawn per attempt). The 24 hours count from the last *attempt*, not the last success, so a broken network never
causes more than one request a day. Nothing is sent at quit.

**Request.**
- `POST https://<telemetry host>/v1/report`, body the report, `Content-Type: application/json`, `User-Agent:
  PhotoSquirrel`. No cookie jar, no query string, no other header.
- HTTPS with rustls and the platform verifier, `HTTPS_PROXY` honoured, as for updates.
- No redirects at all. Connect 15 s, total 30 s. The response body is read to 1 KiB and ignored.
- `2xx`: the sent events leave the queue and the report is appended to `sent.jsonl`. `410`: `stopped` is set to the
  running version; nothing is sent until a newer version runs. Anything else: the queue is kept and the attempt
  time recorded. No notice, no retry loop, a debug-level log line only.
- There is no report or batch identifier. If a response is lost after the collector counted the report, the events
  are counted twice the next day. That is rare and accepted; an identifier to prevent it would be a value that links
  two requests.

**Test seam.** `PHOTOSQUIRREL_TELEMETRY_SERVER=http://127.0.0.1:<port>`, loopback only, as for updates. Tests
otherwise inject a recording transport. A developer build (no `PHOTOSQUIRREL_RELEASE_BUILD`) never reaches the real
host and reaches the loopback server only.

**Size.** A typical day is one report of two events, under 400 bytes. The cap is 50 events and 16 KiB.

### D6. "See exactly what is sent" and the Privacy group

Settings > General gains a **Privacy** group, below Updates and separate from it:

```
Privacy
  [ ] Share anonymous health reports
      Version, operating system and whether updates worked. At most once a day. No ID, no file names, no photos.
      Last report: 2 March - 2 events                                  [ See exactly what is sent ]
  Crash reports   ( Off | Ask me each time | Always send )             (stage 2)
```

- The switch turns reporting off with one click and no confirmation. Turning it on needs no card.
- **See exactly what is sent** opens a dialog with three parts: *Waiting to be sent* (the report that would go out
  now, built from the queue), *Sent* (the last 10 reports with day and address) and, at the bottom, where reports go
  and what the collector keeps, with a link to `docs/telemetry.md`. The text is not a summary or a reformatted copy:
  the view shows the bytes of the request body, and a test compares them with what a recording server received.
- While reporting is off or undecided the dialog shows an example built from this installation's real system
  values, under "Example - nothing has been sent". So the user can judge the report before saying yes.
- **Copy** puts the shown text on the clipboard.

The dialog reads the files on a background executor and never blocks the UI thread. All colours and sizes come from
`ui-theme` tokens.

### D7. Where reports go

**The rule that matters most: the app sends to a hostname the project controls** (`telemetry.<owner's domain>`),
and speaks the project's own tiny format. What stands behind the name can then change, to another host, a paid plan
or a vendor, by changing DNS or the collector, without shipping a client. Shipped clients that never update keep
working, or are told to stop with `410`.

**Options compared.** Limits and prices are from memory as of 2026 and **must be re-verified** (task 1.3).

| Option | Free tier (to re-verify) | Cost of outgrowing it | Privacy posture | Role here |
|---|---|---|---|---|
| GitHub only (stage 0) | free; API rate limits only | nothing to outgrow | no new data flow at all | always on |
| Own collector: Cloudflare Worker + D1 | Workers: about 100k requests/day. D1: about 5 GB, about 100k row writes/day | Workers Paid, about US$5/month, then per use | project's own code; counts only; Cloudflare carries the request and sees the IP in transit | **recommended** |
| Same Worker + Analytics Engine | included allowance; about 3 months retention | part of Workers Paid | one timestamped data point per event; sampled when queried | fallback if D1 writes run out |
| Aptabase (open source, AGPL) | cloud: about 20k events/month | paid plans by volume; self-hosting needs a server with ClickHouse | privacy-first, no device ID; its SDK adds locale and a session ID; unique counts from a daily salted hash of IP and user agent | a possible backend *behind* the collector, not linked into the app |
| PostHog | cloud: about 1M events/month | per event beyond that | built for per-person analytics; wants a `distinct_id`; IP capture and GeoIP on unless disabled | only if product analytics is ever wanted, behind the collector |
| Self-hosted on a VPS | none | about US$5/month and upkeep | full control | not while free matters |

Also looked at: TelemetryDeck (privacy-first, a free tier, HTTP ingest; it expects a hashed user ID), Plausible,
Umami and GoatCounter (web analytics; page views are the wrong shape, and GoatCounter's free plan is for
non-commercial use).

**Recommendation: a single-file Cloudflare Worker with D1, in `collector/` in this repository.**
- `POST /v1/report`: a body over 16 KiB or not exactly matching schema 1 is answered `400` and dropped. The
  validator is hand-written against closed sets, so an unknown key or value is a rejection, not a stored string.
- For each event, one upsert into `counts(day, name, app, os_family, os_distribution, os_version, arch, kind, a, b,
  c, n)`, where `a`, `b`, `c` hold the event's own closed values and `n` is incremented. `day` is the event's day and
  must lie between 31 days ago and tomorrow.
- It never reads `CF-Connecting-IP` or any other header, writes no log line, and has Workers logs and Logpush off.
  It stores no row per report and no arrival time. The answer is `204` with no body and no cookie.
- A `RETIRED` variable makes it answer `410` to everything, which stops clients cleanly when the project ends
  reporting or moves.
- It is deployed by a workflow from a tagged commit in a protected environment (`telemetry-collector`), so what
  runs is what the repository shows.
- `cargo xtask telemetry-report` reads the counts through Cloudflare's API with an owner token and writes the same
  kind of CSV as stage 0. The Cloudflare dashboard is not needed.

**Capacity.** At two to three events per installation per day, 100k row writes are about 40,000 reporting
installations a day. Well before that, the project has either revenue or a reason to pay US$5.

**What is honest to say about IP addresses.** Any HTTPS request shows the sender's IP to the network operator:
GitHub for the update check, Cloudflare for a report. The project never receives or stores it. The README says
exactly that, and does not claim "no IP is seen".

**Abuse.** The endpoint is public and the schema is published, so anyone can post invented reports. A client secret
would be worthless in an open-source app. The strict schema limits what can be stored to plausible counts, the body
cap and a per-IP edge rate limit (Cloudflare's free rule; the project stores nothing from it) limit volume, and the
report reader treats sudden jumps as noise. The numbers guide attention; they are not accounting.

**Without a domain.** A free `*.workers.dev` name works, but it is Cloudflare's name, so moving away would need a
client release and would strand old clients. A domain costs about US$10-15 a year and is the only part of this
change that is not free. Commercialising needs one anyway. Recommended: buy it first (task 1.4).

Alternatives considered:
- *Linking a vendor SDK into the app.* See ADR-0130: the SDK decides what is sent, which makes "the exact bytes" and
  "no identifier" promises hard to keep, and it ties shipped clients to the vendor.
- *Taking the telemetry address from the signed update manifest.* It would allow a swap without a domain, but a
  release could then silently redirect where reports go. A built-in, documented address is easier to trust.
- *Storing raw reports and aggregating later.* More flexible, but each stored report is a timestamped record of one
  installation's day. Counts are enough for the questions asked, and what is never stored cannot leak.

### D8. Stage 2: crash reports

**A separate choice.** `telemetry.crash_reports` = `off` (default), `ask` or `always`, in the Privacy group. It is
not offered on the health-report card and is not implied by it. While it is `off`, a panic is written to the local
log as today and nothing else happens.

**What is captured.** A panic hook, chained before the existing one, writes `telemetry/crashes/<n>.json`:

```json
{ "schema": 1, "app": "0.6.0", "os": { "family": "macos", "version": "15.1" }, "arch": "aarch64",
  "kind": "mac_app", "thread": "scan-worker", "location": "crates/ps-scan/src/walk.rs:212:17",
  "stack": [ { "module": "photosquirrel", "offset": "0x1a2b3c" },
             { "module": "photosquirrel", "offset": "0x1a09f0" } ] }
```

- `location` is `PanicInfo::location()`. It survives stripping because it is static data. The dist build must
  remap path prefixes (`--remap-path-prefix` or `trim-paths`) so that it is a path inside the build tree
  (`crates/...` or `<dependency>-<version>/src/...`) and never the build machine's home folder. A location that is
  not of that shape is left out.
- `stack` is return addresses as offsets from their module's load address, at most 64 frames. Only the app's own
  binary and system library *names* appear; a module outside a built-in list of names is written as `other`.
- `thread` is one of the app's own fixed thread role names, else `other`.

**What must never be included:**
- The panic message. Messages routinely contain paths and file names ("failed to open /home/anna/Photos/...").
- Any path outside the build tree, environment variables, command-line arguments, host or user names.
- Memory, registers, stack contents or a minidump. They can hold file names, EXIF text and pixels.
- Log lines, `history.jsonl`, settings, catalog data, the list of sources, GPU or driver strings.
- Any identifier, and the time beyond the day.

**Review before sending.** With `ask`, the next start shows a card: "PhotoSquirrel stopped unexpectedly last
time", the full report text, and **Send**, **Don't send**, **Always send**, none focused. Only Send and Always send
make a request (`POST /v1/crash`, same host and transport rules as D5). Unsent reports are deleted after 30 days,
and at most 5 are sent a day so that a crash loop cannot flood the collector.

**Symbolication.** The packages are stripped, so a stack is offsets only. To resolve them:
- The dist build is made with line tables and split debug info (`debug = "line-tables-only"`,
  `split-debuginfo = "packed"`: a `.dSYM` on macOS, a `.pdb` on Windows, a `.dwp` or an `objcopy --only-keep-debug`
  file on Linux), and then stripped as today. The packaged binary must be byte-for-byte free of debug sections, and
  the cold-start and size numbers must not move; task 9.3 measures both and the extra build time and memory.
- The symbol files are kept per version and platform, outside the packages. Where (public release assets, which is
  free and fits an open-source project but changes the release job's asset checks, or a private bucket) is decided
  in the stage 2 ADR, with a `release-packaging` delta if needed.
- `cargo xtask symbolicate <report.json>` resolves offsets on the owner's machine with `addr2line`-style tooling.
  Symbols and source never go to a third party.

**Options compared** (to re-verify, task 9.1):

| Option | Free tier (to re-verify) | Symbolication | Privacy posture | Role here |
|---|---|---|---|---|
| Own format to the same collector | within D7's limits | offline, with kept symbol files | the report is the few fields above, shown in full | **recommended first** |
| Sentry (hosted) | Developer plan: one user, about 5k errors/month | upload debug files to Sentry | the SDK's defaults collect device context and a server name; needs careful scrubbing | a backend behind the collector if triage tooling is wanted |
| GlitchTip (open source, Sentry-compatible) | small hosted free tier; or self-host | limited | as Sentry's protocol, own server possible | alternative backend |
| Bugsink (self-hosted, Sentry-compatible) | self-host only | limited | own server | alternative backend |

The collector stores crash reports as counts per fingerprint (a hash of `app`, `location` and the top frames) plus
one copy of the stack, which is identical for every report with that fingerprint. That keeps "counts only" true.

**Limits.** Stage 2 sees Rust panics. It does not see a segmentation fault inside a C decoder (libheif, ffmpeg), an
out-of-memory kill or a hang. Those need out-of-process capture and minidumps, which this change rules out.

### D9. Crate and dependency boundaries

**New I/O crate `crates/ps-telemetry`** (no gpui-kit, no tokio), behind the `ps-service` facade like `ps-llm` and
`ps-update`. It holds the schema types, the queue and sent log, the system-values probe and the sender. It depends
on `ps-model`, `serde`/`serde_json` and `ureq` with the platform verifier, all already locked. No new third-party
crate is expected; if OS version detection or module offsets need one, that is a notice and licence check
(ADR-0058/0060). The crate boundary needs its own ADR when the change is picked up (task 1.2).

- `ps-telemetry` does not depend on `ps-update`. `ps-service` translates `Kind`, `UpdateError::class()` and the
  release-build flag into `ps-telemetry`'s own closed enums. The schema therefore lives in one crate and cannot grow
  because an updater type grew.
- `ps-service` gains `TelemetryService`: it records from the places `UpdateService` records history, owns the
  schedule, and gives the UI snapshots (state, last report, the texts for the view). Every call that touches a file
  or the network runs on a background executor or a plain thread. The UI thread only reads snapshots.
- `ui-workspace` and `photosquirrel` still name no HTTP client; the existing manifest check in the quiet-start test
  covers this.
- `photosquirrel-cli` does not link `ps-telemetry` at all, which makes "the CLI never sends" structural.
  `cargo xtask layering` forbids `ps-telemetry` to the core crates, to `ui-*` and to `photosquirrel-cli`.

Alternatives considered:
- *Reusing `ps-update`'s transport.* Its `Http` does `GET` with a redirect allowlist for GitHub's hosts. Reports
  need `POST`, one other host and no redirects. Sharing would widen the most security-sensitive module of the
  updater to save about 30 lines of agent set-up. Rejected; a shared helper for building the TLS agent is fine if
  it stays that small.
- *A module inside `ps-update`.* No new crate and no new ADR, but the updater would then be the crate that sends
  data about the user's system, and the CLI links it. Rejected.
- *Inside `ps-service`.* Network code in the facade, which `auto-updates` D9 already rejected.

### D10. How the promises change

**ADR-0029** is accepted and is not edited. ADR-0130 amends it, as ADR-0124 did, and states which words change.

| Promise | By default (user changes nothing) | After "health reports" on | After "crash reports" on |
|---|---|---|---|
| Sources are never modified | unchanged | unchanged | unchanged |
| Nothing about your photos, files or catalog is sent | unchanged | unchanged | unchanged |
| No identifier of you or your device is sent | unchanged | unchanged | unchanged |
| No analytics | true | a fixed daily report: version, system, update outcomes | as left |
| No crash reports | true | true | a source location and stack offsets, shown before sending |
| Update check sends nothing about you | unchanged | unchanged | unchanged |
| Quiet first 60 s | unchanged | unchanged | unchanged |

**README, proposed text** for the "No telemetry" bullet:

> **No telemetry unless you turn it on.** By default the app sends nothing about you or your photos anywhere: no
> analytics and no crash reports. It checks for updates only if you said yes when it asked, and a check sends
> nothing about you (see [Updates](#updates)). Two things are off until you turn them on, each asked for separately:
> anonymous health reports (the app's version, your operating system and whether updates worked) and crash reports.
> Neither contains your name, an ID, a path or anything about your photos, and Settings shows every report word for
> word (see [what is sent](docs/telemetry.md)). The only other network traffic is the optional AI feature below,
> and only after you start it.

**`docs/telemetry.md`** (new) is the public schema: every event, field and value, the address, what the collector
keeps, that the network operator sees an IP address in transit, how to turn reporting off, and the two environment
variables.

**Quiet start.** The requirement gains "and the health and crash reports the user turned on, which go only to the
telemetry host". `crates/ui-workspace/tests/network.rs` gains two scenarios: with updates automatic and health
reports undecided or off, 48 hours of app time produce requests to the manifest address only; with health reports
on, nothing happens in the first 60 s and every request is a report to the telemetry address.

## Risks / Trade-offs

- **Trust.** Some users will read any telemetry code as a broken promise, whatever the default. Mitigated by the
  default, the wording, the visible bytes, the open collector and release notes that lead with "off unless you turn
  it on". The owner should decide whether that risk is worth the data before stage 1 starts; stage 0 carries none.
- **A biased sample.** Only opted-in users on versions that start are counted. Read the reports for failure classes
  and trends. A version that cannot start at all shows up only as missing `ok` events and, later, rollbacks.
- **Small numbers.** With few users, a combination such as "NixOS, aarch64, archive" may be one person. It is still
  not linked to anyone, the distribution list collapses rare systems, and the counts stay with the owner. Publishing
  statistics would need a minimum count per row.
- **Double counting and forged reports** (D5, D7). Accepted.
- **The deployed collector cannot be verified by users.** The client-side guarantees are the real ones: what is not
  sent cannot be kept. The collector is open and deployed from a tag to keep the distance small.
- **Free tiers change.** The hostname rule (D7) is the defence. If Cloudflare's free plan ends, the collector moves
  or is retired with `410`.
- **Settings round trip through an older version** drops `[telemetry]`, as it drops `[updates]` today. The choice
  comes back as undecided and off, so the failure direction is "sends less".
- **Law.** Reading values from the user's device for reporting needs consent in the EU; this design is opt-in and
  stores anonymous counts. A short privacy statement is still needed before commercial distribution. This is not
  legal advice; the owner should have it checked then.

## Migration Plan

- Stage 0 needs no client release. It can ship the day it is merged.
- Stage 1: settings v4 to v5, both choices undecided and off. No catalog migration. Existing users see the card
  once, at their second start after upgrading at the earliest.
- Rolling back stage 1: set the collector's `RETIRED` variable (clients stop on their next attempt), and ship a
  release without the card. Nothing on users' machines needs cleaning up beyond the small `telemetry` folder.
- Stage 2 is a later release, after stage 1's gate.

## Open questions (for the owner)

1. **Ask or not.** A one-time card with no default (this design), or no card and only the switch in Settings? The
   second is purer and will collect almost nothing.
2. **The hostname.** Which domain? Without one, accept a `workers.dev` name and the lock-in, or wait?
3. **Collector.** The own Worker with counts only (recommended), or a vendor for its dashboard, behind the
   hostname?
4. **`active_day`.** Keep it (adoption by version and system), or restrict stage 1 to update outcomes only?
5. **`update_mode` in `active_day`.** It shows how many users have updates off. Keep, or drop as too curious?
6. **Stage 2.** Part of this change, its own change, or not at all?
7. **Where stage 0's CSV lives.** An orphan `stats` branch (this design) or a separate private repository?

## Verification

```bash
cargo xtask ci
```

```bash
OPENSPEC_TELEMETRY=0 openspec validate update-telemetry --strict
```

```bash
cargo xtask spec-coverage
```

```bash
cargo xtask layering
```
