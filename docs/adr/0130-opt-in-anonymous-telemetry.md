# 0130. Opt-in anonymous telemetry: health reports and crash reports are separate choices, off by default, with a fixed public schema (amends 0029)

- Status: proposed
- Date: 2026-11-10
- Deciders: maintainers

## Context
ADR-0029 says PhotoSquirrel sends nothing anywhere, and that it is revisited "never without a new ADR". ADR-0124
amended it once: an update check is allowed after the user's answer, and it sends nothing about the user. Since
0.4.0 the app updates itself (ADR-0126), but the project cannot see whether updates work in the field. A check, a
download, an install, a trial start and a rollback are recorded only in the user's own `history.jsonl`
(`crates/ps-update/src/history.rs`). A release whose updater fails on one OS or package kind would be noticed only if
users write in. The owner may commercialise later and wants good telemetry, but has no revenue, and the app's
identity is privacy: sources are never modified and nothing about the user has ever left the machine
(`openspec/changes/update-telemetry`).

## Decision
PhotoSquirrel may send diagnostics about itself only after an opt-in made for that purpose, and only what a
public schema lists.

- **Two separate choices, both off by default:** health reports (`telemetry.health_reports` = `undecided` | `on` |
  `off`) and crash reports (`telemetry.crash_reports` = `off` | `ask` | `always`). Neither is bundled with the update
  question or with the other, neither is ever preselected, and each is turned off with one click.
- **Nothing is recorded or sent while a choice is undecided or off.** Turning health reports on does not send
  anything about earlier events. Turning them off deletes the queue and sends nothing, not even the opt-out.
- **A fixed event list:** `active_day`, `update_check`, `update_download`, `update_install`. Values are closed
  sets: versions of PhotoSquirrel, results, reason classes, the update mode. A report adds the OS family and
  major.minor version, the CPU architecture and the installation kind. `docs/telemetry.md` is the schema, and a test
  keeps it equal to the code. A new event or field needs a new ADR.
- **Never sent:** any user, device, installation, session or report identifier; locale, language, time zone or time
  of day; host or user names; paths and file names; error message text; anything about sources, photos, the catalog
  or settings other than the update mode.
- **Transparency:** Settings shows the exact bytes waiting to be sent and the last reports sent, and an example
  before the opt-in.
- **Delivery:** at most one small HTTPS `POST` a day to one built-in hostname the project controls, not before 60 s
  after start, no redirects, no cookies. Failures are silent. The collector stores daily counts only and no IP
  address. Only the release desktop app sends; the CLI and developer builds never do.
- **Crash reports** hold a source location inside PhotoSquirrel's code and stack offsets, never the panic message or
  a memory dump, and with `ask` are shown in full before each send.

This amends ADR-0029's "No analytics, crash reporting" to "no analytics or crash reporting unless the user turned
that report on; what is sent is fixed, anonymous and shown to the user". ADR-0029's default and ADR-0124's rule that
an update check carries nothing about the user both still hold unchanged.

## Alternatives considered
- **Keep ADR-0029 as it is, and use only GitHub's download counts** - free and needs no consent, and it is stage 0
  of the change. But a download is not an install, and a failed or rolled-back update is invisible.
- **Put the version in the update check's user agent or query** - gives adoption for free, but it is telemetry
  without its own consent, which ADR-0124 ruled out. Still ruled out.
- **On by default with an opt-out** - far more data, and the end of the README's promise. The app's identity is
  worth more than the data.
- **One "help improve PhotoSquirrel" switch for updates, health and crashes** - simpler, but a user who wants
  updates would be nudged into reporting. Bundled consent is not consent.
- **A random installation identifier** - makes funnels and unique counts exact. It also makes every report
  linkable, turns the data into personal data, and breaks "nothing identifies you". Daily counts answer the question
  asked.
- **A vendor SDK (Sentry, PostHog, Aptabase) linked into the app** - quick, but the vendor's SDK decides what is
  collected (session and device identifiers are common), it brings an async HTTP stack, and changing vendor needs a
  new client release. The client speaks its own tiny format to the project's own hostname instead.
- **Settings-only opt-in, never asked** - the purest form, but almost nobody would find it, and the reports of the
  few who do would say little. Asked once, with no default, is the compromise. The owner may still choose this.

## Consequences
- The README's "No telemetry" bullet, the release notes and `docs/telemetry.md` state precisely what is true by
  default and what changes after each opt-in. The app-shell "Quiet start" requirement gains the telemetry host, and
  its tests gain cases that prove nothing goes there without the opt-in.
- The numbers are a self-selected sample of opted-in users on installations that still start. They show failure
  classes and trends, not exact rates. A version that cannot start at all never reports.
- The project runs a small collector and needs a hostname it controls. Because the hostname is the project's, the
  backend behind it can change without a client release.
- A new I/O crate behind the facade holds the queue and the sender (its own ADR when the change is picked up). The UI
  crates still link no HTTP client, and there is still no tokio outside `ps-llm`.
- Crash reports need symbol files kept per release outside the packages, which stay stripped (ADR-0030). The crash
  backend is a later, separate ADR.
- A privacy statement is needed wherever the app is distributed commercially. Opt-in consent and anonymous counts
  keep that statement short.

## Revisit when
An event, field or identifier beyond the published schema is wanted; a third party would receive reports directly
from the app; or the opt-in rate is too low for the reports to say anything, in which case the answer is a better
question, never a default of on.
