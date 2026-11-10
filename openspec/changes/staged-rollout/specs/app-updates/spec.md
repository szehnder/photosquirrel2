# Spec Delta

## ADDED Requirements

### Requirement: Rollout schedule
The application SHALL read an optional `rollout` from a verified manifest: a start time and one to eight steps, each a number of hours after the start and a percentage. The percentage in force SHALL be that of the last step whose time has come. A manifest without a rollout SHALL count as 100 percent. A rollout that breaks these rules SHALL fail the check as a manifest that cannot be read.

#### Scenario: Percentage follows the schedule
- **WHEN** a manifest's rollout starts at time T with the steps 5% at 0 h, 25% at 48 h and 100% at 120 h, and it is evaluated at T-1 h, T+1 h, T+47 h, T+48 h and T+121 h
- **THEN** the percentages in force are 5, 5, 5, 25 and 100

#### Scenario: Manifest without a rollout reaches everyone
- **WHEN** a verified manifest for a newer version has no rollout and a scheduled check runs on an installation whose bucket is 99
- **THEN** the version is offered

#### Scenario: Malformed rollout rejected
- **WHEN** a validly signed manifest carries a rollout with a percentage of 140, with steps out of time order, with a percentage that falls, with no step at hour 0, with no step at all, or with nine steps
- **THEN** each check fails with "the update manifest cannot be read", nothing is offered, and an earlier offer is unchanged

### Requirement: Rollout bucket
Each installation SHALL keep a random rollout seed in its update state, drawn once from the operating system's random source. Its bucket for a version SHALL be a number from 0 to 99 computed from the seed and that version: the same for one version every time, and unrelated between versions. The seed and the bucket SHALL NOT be sent, logged or stored anywhere else.

#### Scenario: Bucket is stable for a version and differs between versions
- **WHEN** the buckets for versions 0.5.1 and 0.5.2 are computed twice, with the update state reloaded in between, for 10,000 generated seeds
- **THEN** every seed gives the same bucket for the same version both times, between 3% and 7% of the seeds have a bucket below 5 for each version, and fewer than 1% have a bucket below 5 for both

#### Scenario: Rollout adds nothing to a request
- **WHEN** two installations on the stable channel with different seeds each make a check against a recording test server that serves a manifest with a rollout
- **THEN** their requests are identical byte for byte, and neither seed or bucket appears in a request, a log line or the update history

### Requirement: Gradual automatic offers
A scheduled check SHALL offer a version that has a rollout only when the installation's bucket for that version is below the percentage in force. Otherwise it SHALL report "up to date", download nothing, and decide again at each later check.

#### Scenario: Installation outside the first step waits
- **WHEN** a version rolls out at 5% from the start and 100% after 120 h, and an installation whose bucket is 40 makes scheduled checks 1 h and 121 h after the start
- **THEN** the first check offers nothing and downloads nothing, and the second offers the version

#### Scenario: Installation inside the first step is offered at once
- **WHEN** the same manifest is checked 1 h after the start by an installation whose bucket is 3
- **THEN** the version is offered and handled like any available update

### Requirement: Rollout time comes from the check
The time used to evaluate a rollout SHALL be the date the release host states in its first response to the check, not the computer's clock. When that response states no readable date, the time since the start SHALL be taken as the time since this installation first saw the version, measured on the computer's clock, so a clock set ahead gains nothing.

#### Scenario: Clock set ahead gets nothing early
- **WHEN** the computer's clock is 30 days ahead, the response's date is 1 h after the rollout's start, and the installation's bucket is 40 with 5% in force for the first 120 h
- **THEN** a scheduled check offers nothing

#### Scenario: Response without a date
- **WHEN** the response states no date, the computer's clock is 30 days ahead of the rollout's start, and this installation first saw the version 1 h ago with a bucket of 40 and the same schedule
- **THEN** a scheduled check offers nothing, and a scheduled check 120 h after the first sighting offers the version

### Requirement: Manual check skips the ramp
"Check for updates", in the application or the CLI, SHALL offer a newer version whatever the installation's bucket, unless its rollout is paused or the version is withdrawn. A version offered this way SHALL stay offered at later scheduled checks.

#### Scenario: Manual check offers a version outside the bucket
- **WHEN** an installation whose bucket is 40 chooses "Check for updates" while 5% is in force, and a scheduled check runs a day later with 5% still in force
- **THEN** the manual check offers the version, and it is still offered after the scheduled check

### Requirement: Urgent releases skip the ramp
A manifest marked urgent SHALL be offered as if the percentage in force were 100, unless its rollout is paused.

#### Scenario: Urgent release reaches every bucket
- **WHEN** an urgent manifest with 5% in force is found by a scheduled check on an installation whose bucket is 99
- **THEN** the version is offered

### Requirement: Paused rollout
While a rollout's percentage in force is 0, the version SHALL be offered to nobody: not by a manual check, not when urgent, not on the beta channel. An offer of it that is not yet installed SHALL be held: nothing is downloaded or installed, a staged payload is kept, and the status says the version is paused. The offer SHALL continue when a later manifest raises the percentage.

#### Scenario: Pause holds a staged update
- **WHEN** an installation has 0.5.1 staged to install on quit, a check accepts a higher revision of the 0.5.1 manifest whose rollout is 0%, and the user quits
- **THEN** 0.5.1 is not installed, the staged file is still there, and the status reads "PhotoSquirrel 0.5.1 is paused by its publisher"

#### Scenario: Resume continues the held offer
- **WHEN** a later check accepts a still higher revision of the 0.5.1 manifest with 100% in force, and the user quits
- **THEN** 0.5.1 is installed from the staged file without a new download

#### Scenario: Manual check during a pause
- **WHEN** the newest manifest's rollout is 0% and the user chooses "Check for updates"
- **THEN** nothing is offered and the result names the paused version

### Requirement: Manifest revision
A manifest MAY carry a signed revision number, counted as 0 when absent. For the newest version the installation has accepted, a manifest with a lower revision than the one accepted SHALL be ignored and change nothing. One with a higher revision SHALL replace what is stored for that version, its rollout included.

#### Scenario: Replayed earlier revision ignored
- **WHEN** an installation accepted revision 2 of the 0.5.1 manifest, which is paused, and a validly signed revision 1 with 100% in force is then served
- **THEN** nothing is offered, the stored revision is still 2, and the check reads "up to date"

#### Scenario: Higher revision widens the ramp
- **WHEN** an installation whose bucket is 40 saw revision 0 of a manifest with 5% in force, and a scheduled check then finds revision 1 with 100% in force
- **THEN** the version is offered

### Requirement: Withdrawn versions
A verified manifest MAY list withdrawn versions. The application SHALL honour the list of every verified manifest, whatever that manifest's own version: a withdrawn version SHALL never be offered, not by a manual check either, and an offer or staged payload of it SHALL be dropped. The list SHALL NOT cause an installed version to be removed, replaced or downgraded.

#### Scenario: Withdrawal drops a staged update
- **WHEN** an installation of 0.5.0 has 0.5.1 staged, and a check finds the 0.5.0 manifest at revision 1 listing 0.5.1 as withdrawn
- **THEN** the staged file is removed, nothing is installed at quit, a manual check does not offer 0.5.1, and 0.5.2 is offered once it is published

#### Scenario: Withdrawal never downgrades
- **WHEN** an installation running 0.5.1 accepts a manifest that lists 0.5.1 as withdrawn
- **THEN** no file of the installation changes and no version is installed

### Requirement: Notice on a withdrawn installed version
When the installed version is listed as withdrawn, the application SHALL show a notice, once per version and dismissible, that names the version, says that a corrected version is installed as soon as it is published, and offers the guarded "Go back to <previous>" when a previous version is kept.

#### Scenario: Installed withdrawn version shows a notice
- **WHEN** an installation running 0.5.1 with 0.5.0 kept accepts a manifest that lists 0.5.1 as withdrawn, and is restarted after the notice is dismissed
- **THEN** the notice names 0.5.1 and offers "Go back to 0.5.0", and it is not shown again after the restart

### Requirement: Update channels
The application SHALL have a stable and a beta update channel. On stable, a check SHALL request the stable manifest address only. On beta, it SHALL also request one fixed beta manifest address on the same host, apply every manifest rule to both, and use the manifest with the newer version, then revision. Beta SHALL accept versions with a pre-release part and SHALL treat a rollout that is not paused as 100 percent.

#### Scenario: Stable channel never requests the beta address
- **WHEN** an installation on the stable channel runs for 48 hours of app time with the update transport recorded
- **THEN** every request is for the stable manifest address or a payload, and none is for the beta address

#### Scenario: Dark release reaches beta only
- **WHEN** the stable address serves 0.5.0 and the beta address serves 0.5.1 with 5% in force, and installations of 0.5.0 with a bucket of 99 check on each channel
- **THEN** the beta installation is offered 0.5.1, and the stable installation reads "up to date"

#### Scenario: Beta channel accepts a release candidate
- **WHEN** the beta address serves a validly signed manifest for 0.6.0-rc.1 and an installation of 0.5.0 on the beta channel checks
- **THEN** 0.6.0-rc.1 is offered

#### Scenario: Beta address unavailable
- **WHEN** the beta address answers 404 and the stable address serves a newer version to an installation on the beta channel
- **THEN** the stable version is offered and the status notes that the beta check failed

#### Scenario: Beta check carries no identifiers
- **WHEN** an installation on the beta channel checks against a recording test server
- **THEN** exactly two manifest requests are made, each without a query string, a cookie or a header naming the version, the channel, the seed or the bucket

### Requirement: Rollout never widens trust
Rollout, revision, withdrawal and channel rules SHALL only remove offers. A version SHALL be offered only when the signature, version-order and payload rules would allow it without them. None of these fields SHALL make the application install a version that is not newer than the installed one, accept a manifest no built-in key signed, or contact a host outside the release hosts.

#### Scenario: Beta channel keeps the signature and version rules
- **WHEN** the beta address serves a manifest signed by a key that is not built in, and then a validly signed, urgent manifest with 100% in force for a version older than the installed one
- **THEN** the first check reports "update manifest signature is not valid", the second reads "up to date", and nothing is downloaded

### Requirement: Rollout shown in the update status
The update status, in Settings and the CLI, SHALL show the channel. For a newer version this installation is not offered yet, it SHALL say that the version is rolling out and that "Check for updates" gets it now, or that it is paused. It SHALL NOT show the seed or the bucket.

#### Scenario: Status names a version that is rolling out
- **WHEN** a scheduled check on the stable channel finds 0.5.1 with 5% in force on an installation whose bucket is 40, and the update status is read
- **THEN** it shows the channel "stable" and "PhotoSquirrel 0.5.1 is rolling out; Check for updates gets it now", and contains neither the seed nor the bucket
