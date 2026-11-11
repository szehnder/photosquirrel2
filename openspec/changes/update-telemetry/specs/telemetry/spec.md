# Spec Delta

## Purpose

Lets the project learn whether updates work in the field without weakening PhotoSquirrel's privacy promise. By
default nothing is sent. A user who opts in shares a small, fixed, documented set of anonymous health events, can
read every report byte for byte, and can stop with one click. Crash reports are a second, separate choice.

## ADDED Requirements

### Requirement: Nothing recorded or sent without the opt-in
The application SHALL NOT record or send any health event while health reports are undecided or off. Undecided is the state of a new settings file and of one upgraded from a version without the setting. The update mode SHALL NOT change the health-report setting, and answering the update question SHALL NOT turn health reports on.

#### Scenario: Default installation sends and records nothing
- **WHEN** the application starts on an empty configuration directory, the user accepts "Keep me up to date", and the application runs for 48 hours of app time with the telemetry transport and sockets recorded
- **THEN** no telemetry request was made, the event queue is empty or absent, and health reports are undecided

#### Scenario: Events from before the opt-in are never sent
- **WHEN** an update is installed while health reports are off, and the user then turns health reports on
- **THEN** the queue holds no event about that earlier update

### Requirement: Health-report question asked once and separately
The application SHALL ask the health-report question at most once, as a card of its own. The card SHALL NOT appear in onboarding, nor in a session in which the update question was shown, nor before the update question is answered. Neither answer SHALL be preselected or focused. "Share anonymous reports" SHALL set health reports on and "No thanks" off. Closing the card without an answer SHALL set off.

#### Scenario: Not asked on first launch
- **WHEN** the application starts on an empty data directory and the user answers the update question
- **THEN** no health-report card is shown in that session and health reports stay undecided

#### Scenario: Asked once at a later start with no default
- **WHEN** the application is started again with the update question answered and health reports undecided
- **THEN** the card is shown with neither button focused, and pressing Enter or Space without moving focus changes nothing

#### Scenario: Closing the card means no
- **WHEN** the user closes the card without choosing and restarts the application
- **THEN** health reports are off, the card is not shown again, and no telemetry request was made

### Requirement: Fixed event list
With health reports on, the application SHALL record only these events: `active_day`, `update_check`, `update_download` and `update_install`. The list SHALL be closed: a build SHALL have no way to record an event or field outside the documented schema. `docs/telemetry.md` SHALL list every event, field and allowed value, and SHALL match the code.

#### Scenario: Only listed events are recorded
- **WHEN** health reports are on and the application checks for, downloads and installs an update on a fake layout, then starts the new version
- **THEN** the queue holds only `active_day`, `update_check`, `update_download` and `update_install` events

#### Scenario: Documented schema matches the code
- **WHEN** the schema test compares the event names, field names and allowed values in code with `docs/telemetry.md`
- **THEN** they are identical

### Requirement: Event contents
Every event SHALL carry only: its name, the UTC day, and the PhotoSquirrel version that recorded it. `active_day` SHALL add the update mode, at most once per UTC day. `update_check` SHALL add a result (up to date, available, failed). `update_download` SHALL add the target version and a result (ok, failed). `update_install` SHALL add the previous and target versions and a result (ok, failed, rolled back). Failed and rolled-back results SHALL add a reason class.

#### Scenario: Successful update recorded by the new version
- **WHEN** health reports are on and an update from 0.5.0 to 0.5.1 is installed and 0.5.1 draws its first frame
- **THEN** the queue holds one `update_install` with previous 0.5.0, target 0.5.1 and result ok, recorded by 0.5.1

#### Scenario: Rollback after failed starts recorded by the restored version
- **WHEN** health reports are on, 0.5.1 fails its trial starts and 0.5.0 is restored and started
- **THEN** the queue holds one `update_install` with previous 0.5.0, target 0.5.1, result rolled back and reason class `start_failed`

#### Scenario: One active day per day
- **WHEN** health reports are on and the application is started five times on the same UTC day
- **THEN** the queue holds exactly one `active_day` event for that day

### Requirement: Closed reason classes
A reason class SHALL be one of a fixed documented set of short codes (such as `network`, `http_status`, `untrusted_redirect`, `signature`, `hash_mismatch`, `no_space`, `os_refused`, `version_check`, `start_failed`, `io`, `other`). Error messages, operating-system error text, URLs and file names SHALL NOT be recorded.

#### Scenario: A failure with a path in its message records only the class
- **WHEN** health reports are on and an install fails with an I/O error whose message names a file under the user's home folder
- **THEN** the recorded event has reason class `io`, and no queued or sent byte contains any part of that message or path

#### Scenario: Every update error maps to a class
- **WHEN** the mapping test walks every update error variant
- **THEN** each maps to exactly one documented reason class

### Requirement: Report envelope
A report SHALL be one JSON document with only: the schema number, the events, the operating-system family, its version as major and minor numbers, the CPU architecture and the installation kind. On Linux the distribution SHALL be sent only when it is on a built-in list of common distributions, and otherwise as `other`.

#### Scenario: Envelope holds only the documented fields
- **WHEN** a report is built on a fake Ubuntu 24.04 AppImage installation
- **THEN** its top-level keys are exactly the documented ones, with family `linux`, distribution `ubuntu`, version `24.04`, the architecture and kind `appimage`

#### Scenario: Uncommon distribution reported as other
- **WHEN** a report is built where `os-release` names a distribution that is not on the built-in list
- **THEN** the distribution is `other` and no version string from `os-release` is in the report

### Requirement: Nothing that identifies a user or their photos
A report SHALL NOT contain any user, device, installation, session or report identifier, nor a locale, language, time zone, time of day, host name, user name, path, file name, hardware model, source, photo, catalog or settings data other than the update mode. Two reports from one installation SHALL share no value that other installations with the same version, system and installation kind do not also send.

#### Scenario: Reports from two installations are indistinguishable
- **WHEN** two fake installations with different user names, data directories, locales, time zones and catalogs record the same events on the same day
- **THEN** their reports are byte-identical

#### Scenario: Canary strings never appear
- **WHEN** an installation whose user name, host name, data directory, source paths and photo names contain a canary string runs a scan, an export and an update with health reports on
- **THEN** no queued or sent byte contains the canary

### Requirement: Daily batch
With health reports on, the application SHALL send queued events in one report, no earlier than 60 seconds after start and at most once per 24 hours across restarts, with up to one hour of random delay. A report SHALL hold at most 50 events and 16 KiB. With an empty queue nothing SHALL be sent. Recording and sending SHALL never run on the interface thread and SHALL NOT delay quitting.

#### Scenario: One report a day
- **WHEN** health reports are on and the application runs for 72 hours of app time with events recorded each day
- **THEN** at most three reports were sent, none in the first 60 s, and consecutive reports are at least 24 hours apart

#### Scenario: Empty queue sends nothing
- **WHEN** health reports are on, the queue is empty and a send is due
- **THEN** no request is made

### Requirement: Restricted report request
A report SHALL be sent as one HTTPS `POST` to one built-in address on the telemetry host, with user agent `PhotoSquirrel`, no cookies, no query string and no header that identifies the user or installation. Redirects SHALL NOT be followed. The response SHALL be read to at most 1 KiB and SHALL NOT change what the application does, except that a `410` answer stops sending.

#### Scenario: Request carries no identifiers
- **WHEN** a report is sent to a recording server
- **THEN** the request is a POST with user agent `PhotoSquirrel`, no cookie, no query string, and a body equal to the report shown in Settings

#### Scenario: Redirect refused
- **WHEN** the telemetry server answers with a redirect to another host
- **THEN** no second request is made and the send counts as failed

### Requirement: Sending fails silently and gives up
A failed send SHALL show no notice, banner or dialog, and SHALL NOT be retried before the next daily slot. Events older than 30 days and events beyond the newest 200 SHALL be dropped unsent. After a `410` answer the application SHALL send nothing more until a newer version is installed.

#### Scenario: Offline for a month
- **WHEN** health reports are on and every send fails for 40 days of app time
- **THEN** at most one request was attempted per 24 hours, no notice was shown, and the queue holds no event older than 30 days

#### Scenario: Collector retired
- **WHEN** the telemetry server answers `410` and the application then runs for 7 more days of app time
- **THEN** no further request is made and the Privacy group reads "Reporting has been switched off by the project"

### Requirement: See exactly what is sent
Settings SHALL offer "See exactly what is sent", in every state of the setting. It SHALL show, as the exact JSON text, the events waiting to be sent and the last 10 reports sent with the day each was sent and the address. While health reports are off or undecided it SHALL show an example report built from this installation's real values, marked as not sent.

#### Scenario: Sent bytes equal shown bytes
- **WHEN** a report is sent to a recording server and the view is opened
- **THEN** the newest entry's text is byte-identical to the request body the server received

#### Scenario: Example before opting in
- **WHEN** the view is opened with health reports undecided
- **THEN** it shows an example report marked "Example - nothing has been sent", and no telemetry request was made

### Requirement: Turned off with one click
Turning health reports off SHALL take one click in Settings, SHALL take effect at once without a restart, and SHALL delete the queue. No request SHALL be made after it, including one that reports the opt-out. The environment variables `PHOTOSQUIRREL_TELEMETRY=0` and `DO_NOT_TRACK=1` SHALL force health reports and crash reports off, and Settings SHALL say so.

#### Scenario: Off deletes the queue and stops sending
- **WHEN** health reports are on with events queued, the user turns the switch off and the application runs for 48 more hours of app time
- **THEN** the queue is empty and no telemetry request was made after the click

#### Scenario: Environment override
- **WHEN** the application starts with health reports on and `DO_NOT_TRACK=1` set
- **THEN** nothing is recorded or sent and the Privacy group reads "Switched off by your system (DO_NOT_TRACK)"

### Requirement: Only the release application reports
Only the desktop application of a release build SHALL record or send health events. The CLI, developer builds and the update helper SHALL never open a connection to the telemetry host, whatever the settings say.

#### Scenario: CLI and developer builds stay silent
- **WHEN** health reports are on and a developer build of the application runs for 48 hours of app time, and the CLI runs a scan and `update check`
- **THEN** no telemetry request was made by either

### Requirement: Collector keeps counts only
The project's collector SHALL accept a report only when it matches the published schema exactly, rejecting unknown fields and values. It SHALL store only daily counts per combination of documented values. It SHALL NOT store or log the sender's IP address, request headers, the raw report or the time of arrival, and SHALL NOT set cookies.

#### Scenario: Unknown field rejected
- **WHEN** the collector receives a report with one extra field
- **THEN** it answers `400` and stores nothing

#### Scenario: Only counters stored
- **WHEN** the collector receives two valid identical reports from different addresses
- **THEN** each matching counter is 2, and storage holds no address, header, timestamp or report body

### Requirement: Adoption report from public data
`cargo xtask release-stats` SHALL record, with no change to the application, the download count of every release asset and of the update manifest from GitHub's public data into a dated row of a report file. The report SHALL state that downloads are neither installations nor successful updates.

#### Scenario: Snapshot appended
- **WHEN** `cargo xtask release-stats` runs twice against a recorded GitHub response with rising counts
- **THEN** the report holds two dated rows per asset, the daily difference is derivable, and its header carries the limits text

#### Scenario: GitHub unavailable
- **WHEN** the command runs and GitHub answers with an error or a rate limit
- **THEN** it exits non-zero, names the cause and leaves the report unchanged

### Requirement: Published statement of what is sent
The README and `docs/telemetry.md` SHALL state what the application sends by default (nothing but the update check the user allowed), what it sends after each opt-in, where it goes, what the collector keeps, and how to turn it off. Release notes of the first version with health reports SHALL say that they are off unless turned on.

#### Scenario: README states the default and the opt-ins
- **WHEN** the documentation test reads the README privacy text
- **THEN** it says that nothing is sent by default, names health reports and crash reports as off until turned on, and links `docs/telemetry.md`

### Requirement: Crash reports are a separate choice
Crash reports SHALL have their own setting, off by default, independent of health reports and of the update mode. No question, card or default SHALL turn it on together with another choice. While it is off, the application SHALL NOT send anything about a crash, and a crash SHALL only be written to the local log.

#### Scenario: Health reports on does not enable crash reports
- **WHEN** the user turns health reports on and the application later panics and is started again
- **THEN** crash reports are off, no crash report was sent or offered, and the panic is in the local log

### Requirement: Crash report contents
A crash report SHALL contain only: the PhotoSquirrel version, the report envelope's system fields, the thread's role name, the source location of the panic as a path inside the build tree, and the stack as module names with offsets. It SHALL NOT contain the panic message, any path outside the build tree, environment variables, memory or register contents, a minidump, log lines, or any identifier.

#### Scenario: Panic message with a user path is not included
- **WHEN** crash reports are on and a panic's message names a photo under the user's home folder
- **THEN** the stored report holds the source location and stack offsets, and no byte of the message, the path or the canary user name

### Requirement: Crash report reviewed before sending
With crash reports set to ask, the application SHALL show a stored crash report in full at the next start with "Send", "Don't send" and "Always send", none preselected, and SHALL send only on "Send" or "Always send". Unsent reports SHALL be deleted after 30 days. At most 5 crash reports SHALL be sent per day.

#### Scenario: Nothing sent until the user says so
- **WHEN** crash reports are set to ask, the application panics, is started again, and the user chooses "Don't send"
- **THEN** no request was made, the report is deleted, and the next start shows no card

#### Scenario: Send transmits the shown bytes
- **WHEN** the user chooses "Send" on the card with a recording server
- **THEN** exactly one POST was made whose body is byte-identical to the text shown on the card

### Requirement: Symbols stay out of packages
Release packages SHALL stay stripped of symbols and debug information. Each release build SHALL keep its symbol files outside the packages, named by version and platform, so that the stack offsets of a crash report can be turned into function names and lines without sending symbols or source to a third party.

#### Scenario: Offsets resolve with the kept symbols
- **WHEN** a packaged binary built by the release dry run is made to panic and its crash report's offsets are resolved with that build's kept symbol file
- **THEN** the resolved stack names the panicking function and line, and the packaged binary contains no debug sections
