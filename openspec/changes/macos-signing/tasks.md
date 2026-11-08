# Tasks

ROADMAP row 38. It is independent of row 37 (`auto-updates`) and may be applied first. It takes **ADR-0128**. There
are no app code, settings or catalog changes. Owner steps need only a browser and Windows (design D4).

## 1. Owner setup (no Mac needed)

- [ ] 1.1 Owner: review ADR-0128 and set it to `accepted`. Verify: the file reads `Status: accepted` and the index row
  drops "(proposed)".
- [ ] 1.2 Owner: create the key, the CSR and the Developer ID Application certificate (G2 Sub-CA), and build the
  `.p12` with 3DES (design D4 steps 1-3). Verify: `openssl pkcs12 -in devid.p12 -info -noout` shows `pbeWithSHA1And3-KeyTripleDES-CBC`,
  and the subject is `Developer ID Application: … (<TEAMID>)`.
- [ ] 1.3 Owner: create the App Store Connect team API key (Developer access) and save the `.p8`, the Key ID and the
  Issuer ID (D4 step 4). Verify: the key is listed as active in App Store Connect.
- [ ] 1.4 Owner: create the `macos-signing` environment (required reviewer; `main` and `v*`), its six secrets and the
  `APPLE_TEAM_ID` variable, then back up and delete the working files (D4 steps 5-6). Verify: Settings >
  Environments shows the rules and six secrets, and Variables shows `APPLE_TEAM_ID`.

## 2. Signing job

- [ ] 2.1 `package (macos)` also uploads `macos-app-unsigned` (a `ditto` zip of the `.app` from the packager staging
  folder). Verify: a dry-run artifact holds `PhotoSquirrel.app` whose `Contents/MacOS` has both programs.
- [ ] 2.2 `packaging/macos/sign.sh` (D2) and the `sign-macos` job (D1): the environment, `contents: read`, no cargo,
  the `trap` clean-up, and no `set -x`. Verify: `scenario_release_packaging_signing_job_isolated_from_the_build` (an
  xtask test that reads the workflow and the script), and `shellcheck packaging/macos/sign.sh` passes.
- [ ] 2.3 The DMG from `hdiutil`, with the Applications link. Compare it with the packager's DMG (window, icon,
  link) and record the result in design.md. Verify: the dry-run DMG mounts with `PhotoSquirrel.app` and an
  `Applications` link.

## 3. Verification job

- [ ] 3.1 `cargo xtask macos-signed`: the D3 parsers and checks, the file-list and `Info.plist` comparison with the
  unsigned app, and `--version` and `--exit-after 3` runs of the signed programs. Verify:
  `scenario_release_packaging_signed_app_accepted_by_gatekeeper` and
  `scenario_release_packaging_unsigned_or_wrongly_signed_app_fails_the_release`, on outputs recorded in
  `xtask/tests/data/macos-signing/`. The failing cases are captured from the ad-hoc dry-run build, and from edits of
  one line each.
- [ ] 3.2 Rebuild the `.tar.gz` (and, once `auto-updates` has landed, the `.app.tar.gz`) from the signed app through
  `archive()`, with new `.sha256` files. Verify: `scenario_release_packaging_archive_binaries_come_from_the_signed_app`.
- [ ] 3.3 The `verify-macos` job and the release job's selection of the signed macOS packages, plus the summary
  line for an unsigned dry run. Verify: `scenario_release_packaging_release_requires_the_signed_macos_packages` and
  `scenario_release_packaging_dry_run_without_signing_says_so` (both read the workflow).
- [ ] 3.4 First real run: `gh workflow run release.yml --ref main`, approved. Record the actual `codesign`, `spctl` and
  `stapler` outputs into `xtask/tests/data/macos-signing/`, replacing any hand-written recording, and the
  notarization time into design.md. Verify: `sign-macos` and `verify-macos` are green, and `spctl` shows
  `source=Notarized Developer ID` for the app and the DMG.

## 4. Docs and the updater

- [ ] 4.1 README "First launch": macOS is the notarized app; remove `xattr` and "is damaged"; keep the Windows
  SmartScreen note. Do the same in `packaging/release-notes.md`. Verify:
  `scenario_release_packaging_no_quarantine_workaround_for_macos`.
- [ ] 4.2 `docs/releasing.md` "macOS signing": D4 as the setup, the secret names, renewal (5 years), revocation and
  replacement, and notarization timeouts. If `auto-updates` has not created the file yet, create it with this
  section. Verify: the section exists and names every secret and the variable.
- [ ] 4.3 Fold design D5 into `auto-updates` (its spec, design D6/D10 and tasks 5.3, 9.3 and 10.1), if not already
  done with this change's spec commit. Verify: `openspec validate auto-updates --strict`.

## 5. Verification

- [ ] 5.1 `cargo fmt --all --check`, clippy -D warnings for xtask, `cargo test -p xtask`, `openspec validate
  macos-signing --strict`, `openspec validate --all --strict`, and `cargo xtask spec-coverage`, with every scenario of
  this change mapped.
- [ ] 5.2 Push. GitHub CI is green on all jobs, and an approved dry run on `main` is green, including signing and
  verification.
- [ ] 5.3 `cargo xtask ci` passes.
- [ ] 5.4 Gate review by a fresh agent that did not write the code. It covers in particular: secrets referenced only
  in `sign-macos`, no cargo in that job, clean-up on every outcome, the Team ID pinned, and the release unable to
  publish unsigned macOS packages for a tag.
