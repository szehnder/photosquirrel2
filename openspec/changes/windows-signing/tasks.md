# Tasks

Draft, not on the ROADMAP yet. It needs `macos-signing` and `auto-updates` archived first (their jobs, specs and
`ps-update` are what this change extends). It takes **ADR-0129**. No settings or catalog changes. Tasks marked
"Owner:" need the owner; some of them cost money, so nothing after section 1 starts before task 1.2.

## 0. Picking the change up

- [ ] 0.1 Re-check every line of design.md "Facts to re-check" and every cell of its options table against the
  providers' current pages (prices, eligibility by country and for individuals, key storage rules, certificate
  lifetimes, SmartScreen and Smart App Control behaviour, the names of Microsoft's service and tools). Correct
  design.md and the ADR's context, with the date and the pages read. Verify: design.md's section names the date of
  the re-check and no longer says "not checked".
- [ ] 0.2 Re-read `.github/workflows/release.yml`, `xtask/src/package.rs`, `crates/ps-update/src/install.rs` and
  the pinned cargo-packager version's `src/package/nsis/mod.rs` and `src/codesign/windows.rs`, and update design.md
  "Context" where they changed (job names, the `signCommand` behaviour, how `run_installer` is called). Verify:
  every file, job and function named in design.md exists under that name.
- [ ] 0.3 Reconcile the specs. If `openspec/specs/release-packaging/spec.md` now holds "README first launch matches
  signing", add a `## MODIFIED Requirements` entry that drops its Windows SmartScreen clause, and adjust
  `scenario_release_packaging_no_quarantine_workaround_for_macos` in task 6.1. Check that `app-updates` exists in
  `openspec/specs/`. Verify: `OPENSPEC_TELEMETRY=0 openspec validate windows-signing --strict`.

## 1. Owner decisions and setup

- [ ] 1.1 Owner: review ADR-0129 and set it to `accepted`. Verify: the file reads `Status: accepted` and the index
  row drops "(proposed)".
- [ ] 1.2 Owner: choose the provider from the corrected options table, in the recommended order unless task 0.1
  changed it, and decide what monthly or yearly cost is acceptable. Verify: design.md records the choice, the date
  and the publisher name Windows will show.
- [ ] 1.3 Owner: apply to the provider and finish its validation (design D5 step 1). Verify: the provider's console
  shows an active signing identity, and a test file signed through it passes `signtool verify /pa /v`.
- [ ] 1.4 Owner: create the `windows_signing` environment (required reviewer; `main` and `v*`), its secrets or the
  OIDC trust, and the repository variable `WINDOWS_SIGNER_SUBJECT` (D5 steps 2-4). Verify: Settings > Environments
  shows the rules, and Variables shows `WINDOWS_SIGNER_SUBJECT`.

## 2. Experiments (design D2, before any workflow is written)

- [ ] 2.1 Q3: on a Windows runner or PC, build the programs, change them (any signature will do, or append
  nothing and only move them), and run `cargo packager --formats nsis` on them without rebuilding. Verify: the
  installer installs those exact files (compare hashes after a silent install into a temp profile), and design.md
  records how the programs must be placed.
- [ ] 2.2 Q1 and Q2: with the chosen provider, sign the two programs, pack, sign the installer (route A). Install
  from a browser download on Windows 11, with Smart App Control on if a machine has it, and uninstall from Settings.
  Verify: design.md records what SmartScreen and Smart App Control showed for the installer, the programs and the
  unsigned uninstaller, and which route (A, B or C) is taken, with the reason.
- [ ] 2.3 Q4: run the signed installer with `/S /NS` over an installed copy. Verify: the install location, the
  uninstall key and the shortcuts are as with the unsigned installer, and the app is not started.
- [ ] 2.4 Record real `signtool verify /pa /all /v` output of the signed installer and programs, and of an unsigned
  program, for task 4.1. Verify: the files are in `xtask/tests/data/windows-signing/`.

## 3. Signing jobs

- [ ] 3.1 `package (windows)` also uploads `windows-binaries-unsigned` (both programs and the notices). Verify: a
  dry-run artifact holds `photosquirrel.exe` and `photosquirrel-cli.exe` whose hashes equal those in the unsigned
  `.zip`.
- [ ] 3.2 `packaging/windows/sign.ps1` with the contract of design D3, for the chosen provider. Verify:
  `PSScriptAnalyzer` reports nothing, and a run without credentials exits non-zero, signs nothing and leaves no file
  behind.
- [ ] 3.3 The `sign_windows` workflow input and the signing jobs of the route chosen in 2.2 (design D1, D2): the
  `windows_signing` environment, `contents: read`, no cargo, signing only through the script. Verify:
  `scenario_release_packaging_windows_signing_job_isolated_from_the_build` (an xtask test that reads the workflow
  and the script).
- [ ] 3.4 `cargo xtask package --prebuilt <dir>` and the secret-free `pack-windows` job: the installer and `.zip`
  from the signed programs, with the existing import, licence and smoke checks. Verify:
  `scenario_release_packaging_zip_programs_come_from_the_signed_programs`, and the job has no environment and
  references no secret.

## 4. Verification job

- [ ] 4.1 `cargo xtask windows-signed`: the parsers and checks of design D4, each needing exit code 0 as well as the
  expected text. Verify: `scenario_release_packaging_signed_installer_and_programs_accepted` and
  `scenario_release_packaging_unsigned_or_wrongly_signed_windows_file_fails_the_release`, on the recordings of task
  2.4 and on edits of one line each (other publisher, no timestamp, failed verification, unsigned inner program).
- [ ] 4.2 The `verify-windows` job, and the release job's selection of the signed Windows packages with the summary
  line for an unsigned dry run. Verify: `scenario_release_packaging_release_requires_the_signed_windows_packages`
  and `scenario_release_packaging_dry_run_without_windows_signing_says_so` (both read the workflow), and
  `scenario_release_packaging_release_workflow_covers_every_platform` expects the new `needs`.
- [ ] 4.3 `update-manifest`, `update-manifest-dry` and `update-roundtrip` need `verify-windows` and swap in the
  signed Windows packages first (`packaging/use-signed-windows.sh`, or one script for both platforms). Verify:
  `scenario_release_packaging_manifest_is_signed_after_windows_verification`.
- [ ] 4.4 First real run: `gh workflow run release.yml --ref main -f sign_windows=true`, approved. Replace any
  hand-written recordings with the run's `signtool` output, and record the run number and the job times in
  design.md "Measurements". Verify: the signing jobs and `verify-windows` are green, and the release job used the
  signed packages.

## 5. The updater

- [ ] 5.1 The decision logic in `crates/ps-update/src/install.rs`: before `run_installer`, compare the installed
  program's signer with the installer's (design D6), with the signers read through a function that tests replace.
  Verify: `scenario_app_updates_installer_from_another_signer_refused`,
  `scenario_app_updates_same_signer_accepted_with_a_renewed_certificate` and
  `scenario_app_updates_unsigned_installation_moves_to_a_signed_installer`, on all three CI systems.
- [ ] 5.2 Reading a signer on Windows with `windows-sys` (`WinVerifyTrust`, the signer certificate's subject), the
  installer held open without write sharing from the check until it has started, and the revocation and offline
  behaviour decided and recorded in design D6. Verify: a Windows-only test reads the expected subject from a file
  with an embedded Microsoft signature, gets "unsigned" for a freshly built test program, and gets a failure for a
  signed file with one byte changed.
- [ ] 5.3 `packaging/windows-next-signer` (`none` by default), built into the app and accepted as a second signer;
  `cargo xtask package` fails when the file is missing or malformed. Verify:
  `scenario_app_updates_announced_next_signer_accepted`.
- [ ] 5.4 The round trip on signed runs: `update-roundtrip` (Windows) updates the previous release to the signed
  candidate, and a rehearsal offers a signed installation an unsigned installer. Verify: the first passes and the
  second is refused with "the update is not signed by the same developer", with the installed files unchanged.

## 6. Docs

- [ ] 6.1 README "First launch" and `packaging/release-notes.md`: Windows is the signed installer, with the
  publisher name Windows shows; say honestly, from task 2.2, whether SmartScreen may still ask about a new release;
  keep **More info → Run anyway** for the last unsigned version and earlier. Verify:
  `scenario_release_packaging_windows_first_launch_names_the_publisher`.
- [ ] 6.2 `docs/releasing.md` "Windows signing (ADR-0129)", beside "macOS signing": the provider's setup step by
  step (design D5), the secret and variable names, the extra approvals when cutting a release, renewal (what
  expires, when, and how to check a month before), revocation (when it is safe, and whether signatures made before
  a date stay valid), what to do when a credential leaks, timestamp failures, and how to change the provider with
  `packaging/windows-next-signer` one release ahead. Verify: the section exists and names every secret, the
  variable and the next-signer file.
- [ ] 6.3 If the provider requires it (SignPath Foundation does), add the code-signing policy and the privacy note
  to the README or the project page, in the provider's wording. Verify: the provider's checklist is met.
- [ ] 6.4 Owner: add the ADR to `docs/adr/README.md` and the change to `openspec/ROADMAP.md`, if not already done.
  Verify: both list it.

## 7. Verification

- [ ] 7.1 `cargo fmt --all --check`, clippy -D warnings, `cargo test -p xtask`, `cargo test -p ps-update`,
  `openspec validate windows-signing --strict`, `openspec validate --all --strict`, and `cargo xtask spec-coverage`
  with every scenario of this change mapped. Verify: all pass.
- [ ] 7.2 `cargo xtask ci` passes. Verify: its exit code is 0.
- [ ] 7.3 Push. GitHub CI is green on all jobs, and an approved dry run with `sign_windows` is green, including
  signing, verification and the update round trip. Verify: the run numbers are recorded here.
- [ ] 7.4 Owner: download the dry run's installer with a browser on a Windows PC and install it. Verify: the
  dialog names the expected publisher, the app starts, and what SmartScreen showed is recorded in design.md.
- [ ] 7.5 Gate review by a fresh agent that did not write the code. It covers in particular: credentials referenced
  only in the signing jobs, no cargo in them, clean-up on every outcome, the publisher pinned, both inner programs
  signed, the release unable to publish unsigned Windows packages for a tag, and the updater refusing another
  signer without starting the installer. Verify: the review's result and fixes are recorded here.
