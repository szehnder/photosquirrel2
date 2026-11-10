# Spec Delta

## ADDED Requirements

### Requirement: Rollout in the signed manifest
A release's update manifest SHALL carry revision 0, a rollout that starts at the release time with the steps of `packaging/rollout.toml`, that file's urgent flag and its withdrawn versions. The envelope format and the signed comment SHALL stay unchanged, so versions without rollout support accept the manifest. Generation SHALL fail on an invalid schedule.

#### Scenario: Manifest carries the schedule
- **WHEN** the manifest is generated with a `packaging/rollout.toml` that has the steps 5% at 0 h, 25% at 48 h and 100% at 120 h, `urgent = false` and one withdrawn version
- **THEN** the signed manifest has revision 0, a rollout whose start equals its release time with those three steps, `urgent` false and that withdrawn version, its envelope format is 1 and its signed comment is `photosquirrel-update <version>`

#### Scenario: Invalid schedule fails generation
- **WHEN** `packaging/rollout.toml` has a percentage above 100, steps out of time order, a falling percentage, no step at hour 0 or more than eight steps
- **THEN** generation fails naming the file and the rule, and no manifest is written

#### Scenario: Version without rollout support accepts the manifest
- **WHEN** a manifest with a rollout, a revision and a withdrawn list is verified and read with the manifest type of the last release without rollout support
- **THEN** it verifies and reads with the same version, packages and urgent flag

### Requirement: Beta-first publication
The release job SHALL publish a release as a GitHub pre-release and copy its manifest to the beta manifest address when `packaging/rollout.toml` says `publish = "beta"` or the version has a pre-release part. Otherwise it SHALL publish it as the latest release. The beta address SHALL be the asset `update-manifest.json` of one fixed release, `beta-channel`, that is flagged pre-release and holds nothing else.

#### Scenario: Release job publishes to beta or stable
- **WHEN** the release workflow definition is read
- **THEN** the publishing job reads `publish` from `packaging/rollout.toml`, creates a pre-release and replaces the `beta-channel` asset for `beta` or a version with a pre-release part, creates the latest release otherwise, and fails on any other value

### Requirement: Rollout control workflow
A manually started workflow SHALL promote, pause, resume, complete or withdraw a published release by re-signing a manifest with a higher revision. Only a job in the protected signing environment, with read-only permission, SHALL read the update key. A separate job without the key SHALL check the result and replace the release asset. No client and no server SHALL be needed to change a rollout.

#### Scenario: Control workflow isolated
- **WHEN** the rollout control workflow definition is read
- **THEN** it starts only by `workflow_dispatch`, refuses a ref that is not a `v` tag, its signing job is the only one that references the update key, runs in the protected environment and has read-only permissions, its publishing job is the only one with write permission and runs only after the check, and every checkout has `persist-credentials: false` and every action a commit SHA

### Requirement: Re-signing changes only rollout fields
Re-signing SHALL first verify the published manifest against the repository's public update keys, SHALL change only the rollout, the revision and the withdrawn list, and SHALL raise the revision by one. The check before publishing SHALL fail when anything else differs, when the signature does not verify, or when the revision has not risen.

#### Scenario: Re-signed manifest keeps the release
- **WHEN** a signed manifest is re-signed for each of promote, pause, resume, complete and withdraw
- **THEN** each result verifies with the public update key, has the same version, release time, notes and packages, a revision one higher, and respectively a schedule restarted at the signing time, a single 0% step, a schedule restarted at the signing time, a single 100% step, and the named version added to the withdrawn list

#### Scenario: Altered or unsigned manifest refused
- **WHEN** the manifest given for re-signing does not verify, or the re-signed manifest given to the check differs from the published one in a package's SHA-256, or has the same revision
- **THEN** re-signing or the check fails naming the reason, and nothing is uploaded

### Requirement: Rollout status command
`cargo xtask rollout-status` SHALL print, for a release, its revision, its rollout with the percentage in force, and GitHub's public download counts of its manifest and of each package. It SHALL use no data from installations.

#### Scenario: Status from recorded release data
- **WHEN** the command is run on a recorded GitHub release description and its manifest
- **THEN** it prints the revision, each step, the percentage in force at the given time, and the download count of the manifest and of each of the eight packages

### Requirement: Rollout runbook
`docs/releasing.md` SHALL document publishing to beta first, promoting, pausing, resuming, completing and withdrawing a release, what each of them does for installed versions without rollout support, and how to read the download counts.

#### Scenario: Runbook covers rollout control
- **WHEN** the runbook is checked
- **THEN** it has a rollout section naming the control workflow, its five actions, `packaging/rollout.toml`, the `beta-channel` release and `cargo xtask rollout-status`, and its withdrawal section says what reaches versions without rollout support
