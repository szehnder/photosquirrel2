# Tasks

ROADMAP row 38. It is independent of row 37 (`auto-updates`) and may be applied first. It takes **ADR-0128**. There
are no app code, settings or catalog changes. Owner steps need only a browser and Windows (design D4).

## 1. Owner setup (no Mac needed)

- [x] 1.1 Owner: review ADR-0128 and set it to `accepted`. Verify: the file reads `Status: accepted` and the index row
  drops "(proposed)".
  Done: the owner accepted it on 2026-11-09. The file and the index row are updated.
- [x] 1.2 Owner: create the key, the CSR and the Developer ID Application certificate (G2 Sub-CA), and build the
  `.p12` with 3DES (design D4 steps 1-3). Verify: `openssl pkcs12 -in devid.p12 -info -noout` shows `pbeWithSHA1And3-KeyTripleDES-CBC`,
  and the subject is `Developer ID Application: … (<TEAMID>)`.
  Done (2026-11-09): the certificate was made from Windows and a browser (a G2 certificate valid until 2027-12-08;
  Apple now issues about 13 months). Run 42603578237 imported the `.p12` and signed with it, and `verify-macos`
  found team MD988WZMG9 on every Mach-O file, the bundle and the DMG.
- [x] 1.3 Owner: create the App Store Connect team API key (Developer access) and save the `.p8`, the Key ID and the
  Issuer ID (D4 step 4). Verify: the key is listed as active in App Store Connect.
  Done: run 42603578237 notarized with it; both submissions were Accepted.
- [x] 1.4 Owner: create the `macos_signing` environment (required reviewer; `main` and `v*`), its six secrets and the
  `APPLE_TEAM_ID` variable, then back up and delete the working files (D4 steps 5-6). Verify: Settings >
  Environments shows the rules and six secrets, and Variables shows `APPLE_TEAM_ID`.
  Done: created as `macos_signing` (GitHub cannot rename an environment, so the workflow follows the name, b477ab4).
  Run 42603578237 saw all six secrets (masked) and the variable, and waited for the owner's approval. The owner
  removes the branch rule for `claude/optimistic-faraday-v22v47` once this change is merged.

## 2. Signing job

- [x] 2.1 `package (macos)` also uploads `macos-app-unsigned`: a `ditto` zip of the `.app`, taken from the built DMG
  (cargo-packager deletes its intermediate `.app` once the DMG is built; run 42540009660 found none in
  `target/packager`). Verify: a dry-run artifact holds `PhotoSquirrel.app` whose `Contents/MacOS` has both programs.
  Done: the DMG's licence agreement is answered on stdin (abc346d). In run 42603578237, `sign-macos` unpacked the
  55 MB artifact and signed `Contents/MacOS/photosquirrel` and `photosquirrel-cli`.
- [x] 2.2 `packaging/macos/sign.sh` (D2) and the `sign-macos` job (D1): the environment, `contents: read`, no cargo,
  the `trap` clean-up, and no `set -x`. Verify: `scenario_release_packaging_signing_job_isolated_from_the_build` (an
  xtask test that reads the workflow and the script), and `shellcheck packaging/macos/sign.sh` passes.
  Done: the script and the job, which runs for a pushed tag or a dry run with the `sign_macos` input. The scenario
  passes, and `shellcheck` and `bash -n` are clean. The first real run is task 3.4.
- [x] 2.3 The DMG from `hdiutil`, with the Applications link. Compare it with the packager's DMG (window, icon,
  link) and record the result in design.md. The packager's DMG also shows the licence agreement (`licenseFile`) on
  open; the `hdiutil` DMG does not. Verify: the dry-run DMG mounts with `PhotoSquirrel.app` and an `Applications`
  link.
  Decided (owner, 2026-11-09): no click-through agreement on the shipped DMG. Apache-2.0 asks nothing of the user,
  and the notices stay in the app. `licenseFile` stays in the packager config, because the Windows installer's
  licence page also uses it; only the signed DMG ships, so the unsigned one's agreement never reaches users.
  Done: `sign.sh` builds the DMG from a folder holding the stapled app and an `Applications` link. Run 42603578237
  notarized and stapled it, and Gatekeeper accepted it. The window is Finder's default layout (no background or icon
  positions); without a Mac it was not looked at, and design.md records that.

## 3. Verification job

- [x] 3.1 `cargo xtask macos-signed`: the D3 parsers and checks, the file-list and `Info.plist` comparison with the
  unsigned app, and `--version` runs of the signed programs (a GUI start was dropped, design D1). Verify:
  `scenario_release_packaging_signed_app_accepted_by_gatekeeper` and
  `scenario_release_packaging_unsigned_or_wrongly_signed_app_fails_the_release`, on outputs recorded in
  `xtask/tests/data/macos-signing/`. The failing cases are captured from the ad-hoc dry-run build, and from edits of
  one line each.
  Done: `xtask/src/macos_signing.rs`. Both scenarios pass on hand-written recordings in Apple's formats (task 3.4
  replaces them). The failing cases are the ad-hoc recording and edits of one line each: runtime, team, timestamp,
  rejected, not stapled. `macos_signing.rs` is in the layering `MOVE_ALLOWED` table for its scratch folder.
- [x] 3.2 Rebuild the `.tar.gz` (and, once `auto-updates` has landed, the `.app.tar.gz`) from the signed app through
  `archive()`, with new `.sha256` files. Verify: `scenario_release_packaging_archive_binaries_come_from_the_signed_app`.
  Done: `rebuild_archive` calls `package::archive` with the signed app's `Contents/MacOS` and its notices. The
  scenario passes with stand-in programs on Linux.
- [x] 3.3 The `verify-macos` job and the release job's selection of the signed macOS packages, plus the summary
  line for an unsigned dry run. Verify: `scenario_release_packaging_release_requires_the_signed_macos_packages` and
  `scenario_release_packaging_dry_run_without_signing_says_so` (both read the workflow).
  Done: both pass, and `scenario_release_packaging_release_workflow_covers_every_platform` expects the new `needs`.
- [x] 3.4 First real run: `gh workflow run release.yml -f sign_macos=true` from a ref the environment admits,
  approved. Record the actual `codesign`, `spctl` and `stapler` outputs into `xtask/tests/data/macos-signing/`,
  replacing the hand-written recordings, and the notarization time into design.md. Verify: `sign-macos` and
  `verify-macos` are green, and `spctl` shows `source=Notarized Developer ID` for the app and the DMG.
  Done: run 42603578237 (b477ab4) is green end to end. Notarization was Accepted in 36 s (app) and 41 s (DMG).
  `verify-macos` passed every D3 check against Apple's real tools ("signed by team MD988WZMG9, notarized and
  stapled", 2 Mach-O files), and the release job used the signed packages. That commit did not yet print the tool
  outputs (b982790 adds that), so the hand-written recordings stay until task 3.5.
- [ ] 3.5 Owner: on the next signed run (any later dry run with `sign_macos`, or the first tagged release), copy the
  `codesign`, `spctl` and `stapler` groups of the `verify-macos` log into `xtask/tests/data/macos-signing/` (or ask
  an agent to). Verify: both signing scenarios pass on the real recordings.

## 4. Docs and the updater

- [x] 4.1 README "First launch": macOS is the notarized app; remove `xattr` and "is damaged"; keep the Windows
  SmartScreen note. Do the same in `packaging/release-notes.md`. Verify:
  `scenario_release_packaging_no_quarantine_workaround_for_macos`.
  Done: the scenario passes. The README says macOS releases after 0.3.1 are notarized, and keeps right-click
  **Open** for 0.3.1 and earlier.
- [x] 4.2 `docs/releasing.md` "macOS signing": D4 as the setup, the secret names, renewal (about 13 months),
  revocation and replacement, and notarization timeouts. If `auto-updates` has not created the file yet, create it
  with this section. Verify: the section exists and names every secret and the variable.
  Done: `docs/releasing.md` created, with "Cutting a release" and "macOS signing".
- [x] 4.3 Fold design D5 into `auto-updates` (its spec, design D6/D10 and tasks 5.3, 9.3 and 10.1), if not already
  done with this change's spec commit. Verify: `openspec validate auto-updates --strict`.
  Done in 919dab7.

## 5. Verification

- [x] 5.1 `cargo fmt --all --check`, clippy -D warnings for xtask, `cargo test -p xtask`, `openspec validate
  macos-signing --strict`, `openspec validate --all --strict`, and `cargo xtask spec-coverage`, with every scenario of
  this change mapped.
  Done: fmt clean; clippy -D warnings (xtask, all targets) clean; `cargo test -p xtask` 80 passed; shellcheck clean;
  `openspec validate --all --strict` 33/33; spec coverage maps all 7 scenarios of this change.
- [ ] 5.2 Push. GitHub CI is green on all jobs (draft PR szehnder/photosquirrel2#1), and an approved dry run with
  `sign_macos` is green, including signing and verification (run 42603578237).
- [ ] 5.3 `cargo xtask ci` passes.
- [x] 5.4 Gate review by a fresh agent that did not write the code. It covers in particular: secrets referenced only
  in `sign-macos`, no cargo in that job, clean-up on every outcome, the Team ID pinned, and the release unable to
  publish unsigned macOS packages for a tag.
  Done: PASS-WITH-FIXES (2026-11-09). The safety properties hold: the secrets are only in `sign-macos`, it runs no
  cargo, clean-up runs on every outcome, the Team ID is pinned, and a tag cannot publish unsigned packages. The
  fixes:
  - M1: a dry run that asked to sign but failed to sign is now a failure (the release `if:` and the step guard).
  - L2: every Apple tool must exit 0 as well as print the expected text.
  - L3: the app inside the DMG must be byte-identical to the checked app, beside an `Applications` link.
  - L4: the keychain search list is kept as an array (paths with spaces, bash 3.2).
  - L5: Mach-O detection without a pipe, the fat64 magic, and the workflow tests now assert the release job's
    condition.
  - L6: design D1/D2 wording updated.

  The fixes in `sign.sh` and the DMG check are proven on the next signed run (task 3.5).