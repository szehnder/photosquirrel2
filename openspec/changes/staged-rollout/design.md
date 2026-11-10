# Design

## Context

See proposal.md for the motivation and the owner's constraints. "R" is the first version that ships this change.
The state this design starts from (read on 2026-11-10, with `auto-updates` implemented but not archived):

- **One signed file.** `update-manifest.json` is an envelope `{format: 1, manifest: "<exact JSON text>", signature}`
  (ADR-0125, `crates/ps-update/src/manifest.rs`). The manifest has `product`, `version`, `released` (RFC 3339),
  `urgent`, `notes`, `notes_url` and `packages`. `verify` checks the signature over the exact text against the
  built-in keys, requires the trusted comment `photosquirrel-update <version>`, and only then parses. The type has
  no `deny_unknown_fields`, so unknown fields are ignored; any `format` other than 1 is refused.
- **One fixed address.** `build::MANIFEST_URL` is `.../releases/latest/download/update-manifest.json`. GitHub's
  "latest" skips drafts and pre-releases. Requests go only to `github.com`, `objects.githubusercontent.com` and
  `release-assets.githubusercontent.com` (`build::RELEASE_HOSTS`), carry `User-Agent: PhotoSquirrel` and nothing
  else about the user.
- **Version rules.** `version::offered`: newer than the installed version, not older than `highest_accepted`, a
  pre-release part only for an installation that has one, and not in `skipped`. `check.rs` `judge` raises
  `highest_accepted` for every verified manifest, by version only.
- **An offer outlives the manifest that made it.** When a check does not offer anything, `judge` keeps
  `state.available` as long as it is newer than the installed version and not skipped. So after today's withdrawal
  (flag the release as pre-release), a copy that already holds the offer, or the staged file, still installs it.
- **`urgent`** only changes how a notice reads, and `xtask update-manifest` always writes `false`.
- **Signing.** Job `update-manifest` of `release.yml` runs in the environment `release-signing` (owner as required
  reviewer, `v*` tags only) with `contents: read`, builds xtask and signs. Job `release` is the only one with
  `contents: write`; it checks the manifest again with stock `minisign` and creates the release, with
  `--prerelease` when the version has a pre-release part.
- **State.** `state.json` in the update folder (`state.rs`), outside `settings.toml`. An unreadable file reads as
  the empty state.

## Goals / Non-Goals

**Goals:**
- A new release reaches a small share of installations first and everyone within about a week, with no action by
  the owner and no server.
- The owner can stop, hold, speed up or restart a rollout with one workflow run, and the stop also reaches copies
  that have the update staged.
- A release can be tried by installations that asked for early versions before any stable installation sees it.
- The request stays exactly as it is. Nothing identifies an installation or its bucket.
- Every ADR-0125 rule holds unchanged. The new fields can only take an offer away.
- Everything is testable headless: eligibility is one pure function; the workflows are checked as text.

**Non-Goals (design level):**
- Measuring failures or halting automatically.
- Precise percentages. With a few hundred installations, 5% is a handful of copies; the goal is a smaller blast
  radius and a day or two in which to pause, not statistics.
- Any change to how an update is downloaded, verified, installed or rolled back.

## Decisions

### D1. The manifest's new fields (ADR-0131)

```json
{
  "product": "PhotoSquirrel",
  "version": "0.5.1",
  "released": "2027-02-01T12:00:00Z",
  "urgent": false,
  "revision": 0,
  "rollout": {
    "start": "2027-02-01T12:00:00Z",
    "steps": [
      { "after_hours": 0, "percent": 5 },
      { "after_hours": 48, "percent": 25 },
      { "after_hours": 120, "percent": 100 }
    ]
  },
  "withdrawn": [],
  "notes": "...", "notes_url": "...", "packages": { }
}
```

- `rollout` is optional. Absent means 100%, which is also what every manifest published before R says.
- Rules, checked in `Manifest::check` after the signature: 1 to 8 steps; the first has `after_hours` 0; hours
  strictly rising and at most 8,760; `percent` 0 to 100 and never falling; `start` in the `...Z` form
  `xtask::update_manifest::now_rfc3339` writes. A manifest that breaks one is `BadManifest` ("the update manifest
  cannot be read: ..."): a schedule the client cannot evaluate must not be guessed at.
- **Percentage in force** at time `t`: the `percent` of the last step with `start + after_hours <= t`. Before
  `start`, the first step's. A pause is the single step `{0, 0}`; "complete" is the single step `{0, 100}`.
- `start` is separate from `released` because a release that sat on the beta channel for days must begin its
  stable ramp when it is promoted, not when it was built (D7). `released` never changes.
- `revision` (D5) and `withdrawn` (D6) are top-level, default 0 and empty, and are left out of the text when so.
- `format` stays 1 and the trusted comment stays `photosquirrel-update <version>`. A version before R refuses any
  other format, so bumping it would cut every such copy off from updates. They ignore the new fields instead (D9).

**Where the schedule comes from.** A new checked-in `packaging/rollout.toml`, read by `xtask update-manifest`:

```toml
publish = "beta"                          # or "stable" (D7)
urgent = false                            # true: every copy at once, and the notice says so
steps = [[0, 5], [48, 25], [120, 100]]    # [hours after the start, percent]
withdrawn = []                            # versions never to offer again (D6), at most 16
```

It is committed with the version bump, so the schedule of a release is in its tag. This also gives `urgent` a way
to be set, which it lacks today.

Why a schedule in the file instead of re-signing at each step: the owner would have to start a workflow and approve
a deployment three times per release, and a forgotten step leaves a release at 5% for ever. Re-signing is kept for
the exceptions (D8).

### D2. The bucket

- `state.rollout_seed`: 16 random bytes, hex, drawn from the OS (`getrandom`, already in the lock file through
  rustls) the first time a rollout is evaluated, then kept.
- Bucket for a version: the first 8 bytes of `SHA-256(seed || 0x00 || version)` as a big-endian integer, modulo 100.
  The bias of the modulo is below 10^-17.
- A scheduled check offers the version when `bucket < percent in force`.

**Improvement on the brief: a bucket per version, not one per installation.** With one fixed number, the copies
that drew 0-4 would be first for every release for ever, and those that drew 95-99 always last. Hashing the version
in keeps the properties that matter (stable for the life of one rollout, so the offered set only grows as the
percentage rises and a restarted ramp reaches the same copies first) and spreads both the risk and the wait evenly.
People who want to be first choose the beta channel (D7). Cost: one hash.

**Privacy.** The seed never leaves `state.json`: not in a request, the log, the update history or `update status`.
The decision is made after the response arrives, so the request cannot depend on it, and a test compares the
requests of two installations byte for byte. A lost or unreadable state file draws a new seed; the copy may then be
offered a version a few days earlier or later than before, which is harmless.

### D3. Time

A wrong local clock must not move a copy along the ramp. The client uses, in this order:

1. **The `Date` header of the first response to the check.** That is GitHub's own 302 for
   `releases/latest/download/update-manifest.json`. Checked on 2026-11-10 with `curl -sI` on a public repository:
   the 302 carries `Date` and `Cache-Control: no-cache`, so it is generated per request. `Transport::fetch` returns
   it beside the body. On the loopback test server it is the server's `Date`, which tests set.
2. **Without a readable date** (a proxy stripped it): elapsed time is `local now - first sighting`, where
   `state.first_seen = {version, at}` is written by the first check that verifies that version. A constant clock
   error cancels out. Such a copy is never early; it can be late by as long as it took to first see the version.

The date is not signed, only protected by TLS. Someone who can forge it can also withhold the manifest, and gains
only this: a copy is offered a genuinely signed, genuinely newer release some days before its turn, or later. That
is inside what ADR-0125 already accepts (delay), plus "early within the ramp", which a manual check gives anyone.

Alternative considered: the local clock capped by the response date. It is the same code with one more branch and
is wrong in the same direction when the clock is behind.

A copy decides only when it checks, about once a day, so each step reaches a copy up to a day late, plus one quit.

### D4. One eligibility function

`rollout::decide(...)` in a new `crates/ps-update/src/rollout.rs`, pure, with a table test. It runs after
`manifest::verify` and after `version::offered` has said yes; it can only turn that yes into a no. In order:

| # | Condition | Result |
|---|---|---|
| 1 | the version is in any verified `withdrawn` list (D6) | never offered; offer and staged file dropped |
| 2 | percentage in force is 0 (D6) | not offered; an existing offer is **held** |
| 3 | `urgent`, or the channel is beta, or this check is manual, or `state.manual_choice` is this version | offered |
| 4 | `bucket < percent in force` | offered |
| 5 | otherwise | not offered: "up to date", nothing downloaded, decided again at the next check |

- **Manual checks.** "Check for updates" in Settings, the command palette, `photosquirrel-cli update check` and the
  round trip's `--update-now` are manual. A manual check that offers a version records it in `manual_choice`, so the
  next scheduled check does not take the offer back. A user who asks gets the release: they chose to, and it is
  signed. Rows 1 and 2 still apply, so a release the owner stopped cannot be fetched this way.
- **`urgent`** means "everyone needs this now": a ramp would defeat it.
- **Notify mode and the packages that do not install themselves** (`.deb`, archives) use the same function: the
  notice appears when the version is offered.
- A copy that is not offered a version still raises `highest_accepted` to it. That is the replay floor and does not
  depend on eligibility.
- An older offer survives a newer version that is still ramping: with 0.5.1 staged and 0.5.2 at 5%, a copy outside
  the 5% installs 0.5.1 at quit as before.

**Status.** The update status gains `channel` and, when row 2 or 5 applied, a `rollout` entry: the version and
`rolling_out` or `paused`. Settings and `update status` show "PhotoSquirrel 0.5.1 is rolling out; Check for updates
gets it now" or "PhotoSquirrel 0.5.1 is paused by its publisher". No banner is shown for either. The bucket and the
date a copy will be reached are not shown: a date would let two users compare and infer their buckets, and the
manual check already answers "can I have it now".

### D5. Changing a rollout: `revision`, and what a replay can do

Controls work by replacing the release's `update-manifest.json` with a re-signed one for the same version (D8). The
client therefore sees, for a version it may already know, a manifest with a different `rollout`.

- `state.accepted_revision` is stored beside `highest_accepted`. For a manifest whose version **equals**
  `highest_accepted`: a lower revision is a replay, and is ignored whole (the check reads "up to date" and the state
  is untouched apart from the check record); an equal one is the same manifest; a higher one replaces
  `state.available` for that version and is evaluated afresh by D4. A higher **version** resets the revision to that
  manifest's.
- Evaluated afresh means the offer follows the newest manifest in both directions: wider (5% to 100%: offered at
  the next check) or narrower (paused: held; withdrawn: dropped). An installed version is never touched.
- On the beta channel, two manifests are compared by `(version, revision)` and the greater wins (D7).

**Is a replay acceptable without `revision`?** The trusted comment binds a signature to a version, not to a
revision, so every manifest ever signed for 0.5.1 stays valid. Someone who can serve chosen bytes (a broken TLS
path, a CDN fault, or the ability to replace release assets without the key) could serve an earlier one:
- *Narrowing* (the 5% schedule after the owner completed the rollout): a delay. ADR-0125 already accepts that a
  frozen manifest delays updates.
- *Widening* (the 100% manifest after a pause): copies take a release the owner stopped. That is what replaying a
  withdrawn release's manifest does today, so it is not a new weakness, but it is the one case where a replay
  causes an install the owner tried to prevent.

`revision` closes the second case for every copy that has seen the pause, at the cost of one integer in the state.
A copy that never saw the pause can still be shown the older manifest; nothing short of a trusted clock in the
signature (expiring manifests, as TUF does) fixes that, and expiry would need a scheduled re-signing job, which
ADR-0125 rejected with TUF. `withdrawn` (D6) does not have the problem at all, because it has no "undo".

### D6. Pause, withdraw, and the kill switch

**Pause (percentage 0).** For "something looks wrong, hold it while I look".
- Nobody new is offered the version: row 2 comes before row 3, so not by a manual check, not when urgent, not on beta.
- An offer that is not installed yet is held: `state.held = Some(version)`. While held, nothing is downloaded and
  the quit hook and `plan_install` do not install. The staged file stays, so resuming costs no second download.
- The release stays "latest", which is what lets copies see the pause.
- It ends with a higher revision whose percentage is above 0.
- It reaches a copy at its next check, so up to about 25 hours later. A copy that quits before then installs. This
  is why the first step is small.
- Versions before R ignore it.

**Withdraw.** For "this release is bad". Three effects, done by one workflow action (D8):
1. The bad release is flagged as a pre-release and the last good release is set as "latest". This is today's
   runbook step and the only part that reaches versions before R.
2. The last good release's manifest is re-signed with the bad version added to `withdrawn` (and a higher revision).
   That is the manifest copies now fetch.
3. When the `beta-channel` asset holds the bad version, it is replaced by that same manifest.

The client reads `withdrawn` from **every** verified manifest, before the version-order rules, and adds the entries
to `state.withdrawn` (at most 32 kept, newest first). This is deliberate: after step 1 the served manifest is for an
*older* version than `highest_accepted`, which `judge` otherwise ignores. Honouring the list regardless of order is
safe because it can only stop offers and has no undo: version numbers are never reused, and the remedy is a higher
version. A replayed manifest with a `withdrawn` list can only re-state a withdrawal the owner really signed.

For a withdrawn version, a copy:
- never offers it (row 1), not by a manual check either;
- drops `state.available` and the staged file for it, and clears `held`, `manual_choice` and `install_on_quit`;
- when it is **running** that version, shows one dismissible notice: "PhotoSquirrel 0.5.1 was withdrawn by its
  publisher. A corrected version will be installed as soon as it is published." with **Go back to 0.5.0** when a
  previous version is kept. That button is the existing guarded rollback (ADR-0126): it is refused when the catalog
  was upgraded past what the previous version opens.

**The fix must list the bad version too.** `packaging/rollout.toml`'s `withdrawn` goes into every later release's
manifest. Without it: 0.5.1 is withdrawn, 0.5.2 is published with a 5% ramp and becomes "latest", and a copy that
was offline during the incident with 0.5.1 staged now sees only 0.5.2's manifest, is outside the 5%, keeps its old
offer and installs 0.5.1 at quit. The runbook says to add the version to the file and to ship the fix as `urgent`.

**What the kill switch is not.** `withdrawn` does not remove, disable, replace or downgrade an installed version,
and does not stop it from running. Reasons:
- Versions only move forward (ADR-0125). A downgrade path driven by a manifest is the rollback attack that rule
  exists to prevent.
- A new version may have migrated the catalog; an older build refuses that catalog (ADR-0041). An automatic
  downgrade could leave a user with an app that cannot open their work.
- A mistaken or replayed "disable" would stop every copy at once, with no way to reach them again.

So the remedy for a bad version that is already installed is a higher version, published `urgent` (everyone at
once), and until then the notice. It also does not reach copies with updates off, copies that do not check, or
versions before R.

### D7. Channels and dark deploy

**Setting.** `[updates] channel = "stable" | "beta"`, default `stable`, read leniently like `mode`: an unknown
value fails validation for that key and behaves as stable. A missing key reads as stable, so the settings-file
version does not change, and an older build that reads the file ignores the key. Changing the channel drops an offer
that is not installed (and its staged file) and lets the next check decide afresh; it never triggers a check by
itself when the mode is undecided or off.

**What beta means.** A beta installation:
- also accepts versions with a pre-release part (`version::offered` takes the channel; this is the one sentence of
  ADR-0125 that changes);
- is offered every version at 100% unless it is paused or withdrawn (D4 row 3);
- finds releases that are not "latest".

Choosing Beta in Settings asks first: "Beta versions arrive before everyone else gets them and may have faults. A
beta version may upgrade your catalog, and then you cannot go back to the version before it. PhotoSquirrel never
changes or deletes your photos in any version." **Use beta versions** / **Stay on stable**.

Going back to stable keeps the installed version (versions only move forward); the copy waits until a stable
release is newer.

**Dark deploy.** A release is "dark" when it exists on GitHub but is not "latest": it is flagged pre-release. It has
an ordinary version (0.5.1, not 0.5.1-rc.1), the same eight packages and a signed manifest. Stable installations of
every version cannot see it, because they all fetch `latest`. Promoting it clears the flag: the very files that
beta installations ran become the stable release, with nothing rebuilt.

`publish` in `packaging/rollout.toml` chooses per release: `"beta"` publishes dark, `"stable"` publishes as
"latest" as today. A version with a pre-release part is always published dark and can never be promoted. The
proposed default is `"beta"`: no stable installation gets a release until the owner promotes it, which can be a
minute or a week later. That is the owner's call (Open decisions).

**How a beta installation finds a dark release.** `releases/latest/download/...` skips it, and the client cannot
guess a tag name. Options that need no token:

| Option | Hosts | Verdict |
|---|---|---|
| A. A second fixed asset address on a rolling release: `releases/download/beta-channel/update-manifest.json` | the three already allowed | **chosen** |
| B. The REST API, `api.github.com/repos/.../releases` | a new host | 60 requests an hour per IP without a token, shared by everyone behind one NAT; JSON to parse and trust for ordering |
| C. The Atom feed, `github.com/.../releases.atom` | allowed | XML parsing, no stability promise, and a second request for the manifest anyway |
| D. GitHub Pages or `raw.githubusercontent.com` | a new host | a workflow must write to a branch; raw is cached for minutes |
| E. Mark the dark release "latest" and hide it from stable with `rollout` 0% | allowed | versions before R ignore `rollout` and would all take it |

Option A in detail:
- One release with the tag `beta-channel`, created once by the owner, flagged pre-release so it can never be
  "latest", titled "Beta channel pointer (not a download)", with one asset: a copy of the `update-manifest.json` of
  the newest dark release. The tag does not match `v*`, so it starts no workflow and cannot reach the signing
  environment.
- `build::BETA_MANIFEST_URL` is that fixed address. Same redirect chain, same hosts, same 64 KiB limit.
- The copy is the same signed envelope. Its package addresses point under `releases/download/v<version>/`, which
  `Manifest::check` already requires, and a pre-release's assets are downloadable at those addresses.
- **A beta check is two requests:** the stable address, then the beta address. Each manifest goes through every
  rule on its own; the greater `(version, revision)` is used; `withdrawn` is read from both. Fetching both means
  the pointer only has to be right for dark releases: the release job does not touch it for stable ones, and a
  stale or missing pointer costs a beta installation nothing but the dark release.
- A failed beta request (the asset is briefly absent while it is replaced, or the release was deleted) leaves the
  stable result in force. The check counts as successful and the status adds "the beta check failed: <reason>".
- Privacy: the second request has the same headers as the first. GitHub can tell that an IP address asks for the
  beta manifest; nothing else is learned. The README's "what a check sends" says so.

Alternative considered: beta fetches only the pointer, and the release job keeps it at the newest release of any
kind. One request, but a forgotten or failed pointer update would freeze every beta installation, silently.

**Promote** (D8) re-signs the dark release's manifest with `rollout.start` set to now, the schedule from the
workflow's input (default: the release's own steps) and a higher revision, uploads it to the release, and then runs
`gh release edit v<version> --prerelease=false --latest`. Asset first, flag second: if the second step fails the
release is still dark, and the run can be repeated. Clearing the flag by hand in the GitHub UI also works, but the
ramp then counts from the old start and may already be at 100%.

### D8. The control workflow

A new `.github/workflows/rollout.yml`, `workflow_dispatch` only, with inputs `version`, `action` (`promote`,
`pause`, `resume`, `complete`, `withdraw`), `fallback` (for `withdraw`: the last good version), optional `steps`,
and `dry_run`. One `concurrency` group, so two runs cannot interleave.

| Action | Manifest re-signed | Rollout written | Release flags |
|---|---|---|---|
| promote | `version`'s | start now, the schedule | `version`: not pre-release, latest |
| pause | `version`'s | single step 0% | none |
| resume | `version`'s | start now, the schedule | none |
| complete | `version`'s | single step 100% | none |
| withdraw | `fallback`'s | unchanged, `withdrawn` gains `version` | `version`: pre-release; `fallback`: latest |

`resume` restarts the schedule instead of continuing it. The buckets do not change, so the copies reached first are
the ones that had it before the pause; nobody new is added until the second step. That is the cautious reading of
"resume", and `complete` or `steps` covers the others.

Every action also replaces the `beta-channel` asset when it currently holds `version` (or, for `withdraw`, the bad
version), so beta installations see a pause or a withdrawal of a dark release.

**Jobs, keeping ADR-0125's split:**
1. `resign` runs in the environment `release-signing` with `contents: read`. It downloads the published manifest,
   builds xtask and runs `cargo xtask update-manifest resign`. That command first verifies the input against
   `packaging/update-key.pub` and `update-key-next.pub`, then changes only `rollout`, `revision` (+1) and
   `withdrawn`, signs, and uploads the result as an artifact. **It cannot be made to sign new packages, notes or a
   new version**: everything but those three fields must already carry the key's signature.
2. `resign-dry` does the same with a throwaway key and no environment, when `dry_run` is set.
3. `publish` has `contents: write`, no environment and no key, and compiles nothing. Like the `release` job, it
   verifies the artifact with stock `minisign`, and a short script compares it with the published manifest: equal
   apart from the three fields, revision higher. Then `gh release upload --clobber`, and the flags. It does not run
   on a dry run.

Signing follows whatever `update-manifest` does at implementation time. If the follow-up noted in `auto-updates`
(sign with stock `minisign` in a job that compiles nothing) has been done by then, `resign` does the same: the edit
is a few lines of JSON.

**The environment rule is not changed.** `release-signing` admits only `v*` tags. The workflow is started from a
release tag (`gh workflow run rollout.yml --ref v0.5.1 -f version=0.5.1 -f action=pause`), which satisfies the rule,
and it refuses any other ref unless `dry_run` is set. `version` may differ from the ref (withdraw re-signs the
fallback's manifest; any release tag at or after R will do as the ref). The owner still approves each run, and the
approval page shows the inputs.

Alternative considered: add `main` to the environment's deployment branches, as `macos_signing` has. A workflow fix
would then need no new tag, but any workflow on `main` could ask for the key. Rejected for now; a dry run from
`main` rehearses the workflow without the key instead (Open decisions).

### D9. Versions before R, and the first releases

- A version before R ignores `rollout`, `revision` and `withdrawn` and reads `latest` only. It takes every stable
  release at once, cannot be paused, and is stopped only by the pre-release flag. This lasts for as long as such a
  copy is in use. A release-packaging scenario keeps a frozen copy of the pre-R manifest type and checks that it
  reads a manifest with all three fields; the pre-publish round trip (previous release updates itself to the
  candidate) proves it on the real binaries.
- The pre-release flag works for every version, because every version reads `latest`. So "dark" protects old copies
  too; only "reaching beta installations" needs R.
- **Release R itself** cannot be ramped: its audience is all before R. Publish it dark, try it by hand, promote it.
  There are no beta installations yet, since the channel arrives with R.
- **R+1 is the first ramped release**, and only for copies already on R. Copies still on older versions take it at
  once, so its early exposure is "5% of the copies on R, plus every older copy that checks". Leave a week or two
  between R and R+1 so that automatic updates have moved most copies to R.
- The round trip uses `--update-now`, a manual check, so a ramp never makes it flaky. A dark candidate is served to
  it from the loopback server exactly as now.
- State: a version before R that is put back by the guarded rollback reads `state.json`, ignores the new fields and
  drops them when it saves. The next R then draws a new seed. Harmless.

### D10. Seeing the ramp without telemetry

GitHub counts downloads per release asset and publishes the numbers (`download_count` in
`repos/<owner>/<repo>/releases/tags/<tag>`). `cargo xtask rollout-status [--tag v0.5.1]` reads that through `gh
api` and the manifest, and prints the revision, the steps, the percentage in force now, and per asset the count.
It is the owner's tool; it runs on the owner's machine and asks GitHub, never an installation.

How to read it:
- **The manifest's count on the "latest" release** rises by about one per stable check, so its daily increase is
  roughly the number of installations that check. The `beta-channel` asset's daily increase is the beta ones.
- **`.app.tar.gz`** is downloaded only by the updater, so it is the cleanest sign that copies are taking the
  release; the AppImage and the Windows setup mix updater and browser downloads.
- New-version payload downloads divided by daily checks should track the schedule: near 5% in the first two days,
  then 25%, then the rest.
- Caveats, which the command prints: replacing an asset resets its count (so the manifest's count restarts at each
  control action; the command shows the asset's upload time); resumed downloads, bots and mirrors inflate counts;
  copies before R take the release at once. Task 5.4 confirms that a download through `latest/download/...` is
  counted on the asset.

This shows that a ramp is moving. It cannot show that the new version *works* on those copies. That needs reports
from installations, which is the separate draft `update-telemetry` (opt-in, ADR-0130, proposed); nothing here depends on
it.

### D11. State and code layout

New `UpdateState` fields, all `#[serde(default)]`: `rollout_seed: Option<String>`, `accepted_revision: u64`,
`first_seen: Option<{version, at}>`, `manual_choice: Option<String>`, `held: Option<String>`, `withdrawn:
Vec<String>`, `withdrawn_noticed: Vec<String>`, `beta_failure: Option<String>`.

`Transport::fetch` returns the body and the first response's `Date` (parsed IMF-fixdate, seconds since the epoch);
a small parser for it and for the `...Z` RFC 3339 form lives in `rollout.rs`, with no new dependency. The UI still
reaches all of this only through `ps-service`. `ps-update` stays an I/O crate with no tokio (ADR-0127).

## Risks / Trade-offs

- **A halt is not instant.** A copy learns of a pause or withdrawal at its next daily check; one that quits first
  installs. Accepted: the alternative is a request at quit, which delays quitting and fails offline.
- **5% of a small base is nearly nobody.** With 200 installations the first step is about 10 copies, and none may
  run in two days. The ramp still limits the damage and buys time; raise the first step while the base is small.
- **Old versions weaken the first ramps** (D9). Accepted; it fades as copies update.
- **Replacing an asset is delete-then-upload.** For a moment the address answers 404. A check that hits it fails
  quietly (failures stay silent for three days) and runs again the next day.
- **The `beta-channel` release looks like a download** on the Releases page. Its title and notes say it is not.
- **A dark release with a catalog migration** cannot be rolled back by beta users once the catalog is upgraded.
  The confirmation says so, and the trial-start rollback (ADR-0126) still covers a version that cannot start.
- **The control workflow runs the tag's copy of itself.** A fault in `rollout.yml` is fixed only by the next tag.
  It is small, text-checked in CI, and rehearsed by a dry run.
- **More than one way to stop a release** (pause, withdraw, the flag by hand). The runbook leads with one table:
  unsure, pause; bad, withdraw.
- **`manual_choice` lets a user leave the ramp.** Intended; it is their copy, and the release is signed.
- **`update-manifest` still builds xtask beside the key** (the `auto-updates` follow-up). `resign` adds a second
  use of the key with the same exposure, not a wider one.

## Migration Plan

- No settings-file version bump, no catalog migration. `state.json` gains defaulted fields.
- Order: the client and xtask parts ship in R with `publish = "stable"` and a single 100% step, so R behaves like
  today. The owner creates `beta-channel` and rehearses the control workflow on a release candidate. R+1 turns on
  the 5/25/100 schedule and, if the owner agrees, `publish = "beta"`.
- Undoing the change: set `steps = [[0, 100]]` and `publish = "stable"`. Every release then reaches everyone at
  once, as before; the client code stays inert.

## Open decisions for the owner

1. **The schedule**: 5% / 25% / 100% at 0 / 48 / 120 hours, or a larger first step while there are few users.
2. **`publish = "beta"` as the default**: every release is dark until promoted, at the cost of one workflow run and
   one approval per release. Or `"stable"`, with dark deploys only on request.
3. **Bucket per version** (proposed) or one fixed bucket per installation (the brief).
4. **A pause also blocks manual checks** (proposed), so "Check for updates" cannot fetch a release the owner
   stopped.
5. **The control workflow runs from a `v*` tag** (proposed, environment rules unchanged), or `main` is added to
   `release-signing`.
6. **The notice on a withdrawn installed version**: in this change (proposed), or left out to keep it smaller.
7. **A beta check is two requests**, which the README must say. Or one request and a pointer that must never go
   stale.
8. **One ADR (0131) for rollout, revision, withdrawal and channels**, or the channel and `beta-channel` pointer
   split into their own.

## Verification

```bash
cargo xtask ci
```

```bash
OPENSPEC_TELEMETRY=0 openspec validate staged-rollout --strict
```

```bash
cargo xtask spec-coverage
```

```bash
cargo xtask layering
```

```bash
gh workflow run rollout.yml --ref main -f version=<version> -f action=pause -f dry_run=true
```
