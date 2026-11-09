# Spec Delta

## Purpose

Keeps PhotoSquirrel current on macOS, Windows and Linux. The app learns about new versions only with the user's
consent, accepts only updates signed by the project, installs them without interrupting work or touching the user's
photos and catalogs, and puts the previous version back when a new one cannot start.

## ADDED Requirements

### Requirement: No update request without an answer
The application SHALL NOT send any update request while the update mode is undecided. Undecided is the state of a new settings file, and of one upgraded from a version without updates, until the user answers the update question.

#### Scenario: Undecided mode sends nothing
- **WHEN** the application starts with an undecided update mode and runs for 48 hours of app time with the update transport recorded
- **THEN** no update request was made and the update status reads "undecided"

### Requirement: Update question asked once
The application SHALL ask the update question once: in onboarding on a first launch, or as a one-time card at the first start after an upgrade from a version without updates. "Keep me up to date" SHALL be preselected and set the mode to automatic. "No thanks" SHALL set it to off. Either answer SHALL be saved, and the question SHALL NOT be asked again.

#### Scenario: Upgraded user asked once
- **WHEN** the application starts on a version-3 settings file, the user chooses "No thanks", and the application is restarted
- **THEN** the card was shown on the first start only, the mode is off, and no update request was made

#### Scenario: Keep me up to date enables automatic updates
- **WHEN** the user accepts the preselected choice on the first-launch update question
- **THEN** the update mode is automatic and the card is not shown on the next start

### Requirement: Check schedule
With mode automatic or notify, the application SHALL make its first update check no earlier than 60 seconds after start. It SHALL then check at most once per 24 hours, counted across restarts from the last completed check, with up to one hour of random delay added. A check SHALL never block the interface.

#### Scenario: First check after 60 seconds
- **WHEN** the application starts with mode automatic and no previous check, with the update transport recorded
- **THEN** no request is made in the first 60 s of app time and exactly one manifest request is made within the following 5 minutes

#### Scenario: At most one check a day across restarts
- **WHEN** a check completed 3 hours ago and the application is restarted with mode notify
- **THEN** no request is made until at least 21 hours later

### Requirement: Nothing about the user is sent
An update check SHALL be one HTTPS GET of a fixed manifest address with no query string, no cookie, no identifier, and no information about the installation, the catalog or the photos. The user agent SHALL be the product name alone, without a version. A download SHALL send the same headers plus a byte range when resuming.

#### Scenario: Check request carries no identifiers
- **WHEN** a check is made against a recording test server
- **THEN** the request has no query string and no cookie, its user agent is exactly `PhotoSquirrel`, and no request header contains the version, the host name, the user name or a path

### Requirement: Restricted transport
Update requests SHALL use HTTPS with certificate verification. They SHALL follow at most five redirects, and only to HTTPS addresses on the project's release hosts. Any other redirect, plain HTTP, or a response larger than the expected size SHALL fail the request without retrying elsewhere.

#### Scenario: Redirect to an unknown host refused
- **WHEN** the manifest address answers with a redirect to an HTTPS host outside the release hosts, or to plain HTTP
- **THEN** the check fails with "redirect to an untrusted host", that host is never contacted, and the update status shows the failure

#### Scenario: Oversized response cut off
- **WHEN** the manifest response is larger than 64 KiB, or a payload response runs past the size the manifest states
- **THEN** reading stops at the limit and the check or download fails without writing the excess

### Requirement: Signed manifest
The application SHALL accept an update manifest only when the signature carried in the same file verifies over the exact manifest text it carries, against a key built into the application, and its signed comment names the manifest's version. A manifest that is unsigned, carries a bad signature, is signed by another key, or names another version SHALL be rejected, and nothing it lists SHALL be downloaded.

#### Scenario: Tampered manifest rejected
- **WHEN** a manifest's payload SHA-256 is changed after signing, or the manifest is signed by a key that is not built in
- **THEN** the check reports "update manifest signature is not valid", no payload is downloaded, and the status keeps the previous result

#### Scenario: Either built-in key accepted
- **WHEN** one manifest is signed with the first built-in key and another with the second
- **THEN** both are accepted

### Requirement: Only newer versions
The application SHALL offer only a manifest whose version is newer than the installed version and not older than the newest version it has previously accepted, comparing versions by semantic-versioning precedence. A version that has a pre-release part SHALL be offered only when the installed version also has one.

#### Scenario: Older or replayed manifest ignored
- **WHEN** version 0.4.2 is installed, 0.4.3 was accepted earlier, and a validly signed manifest for 0.4.1 or 0.4.2 is served
- **THEN** no update is offered and the status reads "up to date"

#### Scenario: Stable install ignores a release candidate
- **WHEN** 0.4.0 is installed and a validly signed manifest for 0.5.0-rc.1 is served
- **THEN** no update is offered

### Requirement: Verified payload
A payload SHALL count as downloaded only when its size and SHA-256 equal the values in the accepted manifest. A partial or mismatching file SHALL never be installed or shown to the user as ready. A mismatching file SHALL be deleted, and the download SHALL be retried at the next check.

#### Scenario: Corrupted download never installed
- **WHEN** the served payload has one byte changed
- **THEN** the download fails with "downloaded update does not match its signature", the file is removed, no update is marked ready, and the next check downloads it again

### Requirement: Resumable bounded download
Downloads SHALL run in the background into the application's own update folder, resume from the bytes already received, start only when free space there is at least three times the payload size, and keep at most one staged update, removing older staged files.

#### Scenario: Interrupted download resumes
- **WHEN** the connection drops after 40% of a payload and the next check runs
- **THEN** the download continues from the received bytes with a range request, and the finished file verifies

#### Scenario: Low disk space defers the download
- **WHEN** free space in the update folder is below three times the payload size
- **THEN** nothing is downloaded and the status says how much space is needed

### Requirement: Installation kind detected
The application SHALL work out how it was installed: AppImage, a macOS app bundle, a Windows per-user installation, a Debian package, a plain archive, or a developer build. It SHALL install updates itself only for an AppImage, a bundle or a per-user installation, and only when it can write to the installed location and the bundle is not translocated or on a read-only volume.

#### Scenario: Installation kinds reported
- **WHEN** the update status is read for fake installations of each kind in temporary folders: an AppImage with its environment, a bundle in a writable folder, a bundle on a read-only path, a translocated bundle, a per-user installation registered for the user, a package-owned path, and a plain unpacked archive
- **THEN** each reports its kind, the first, second and fifth can install themselves, and each of the others gives its reason

### Requirement: Updates for installations it cannot change
For an installation it does not change, the application SHALL show the new version with a link to its release page. For a Debian package it SHALL offer to download and verify the new package, then reveal the file and show the exact install command. It SHALL NOT run a privileged command itself.

#### Scenario: Debian package gets a verified file and a command
- **WHEN** an update is available to a Debian-package installation and the user chooses "Download"
- **THEN** the verified `.deb` is saved as a new file in the user's Downloads folder, revealed, and the command `sudo apt install` with that file's path is shown with a copy button, and no process is started with elevated rights

#### Scenario: Archive installation notified only
- **WHEN** an update is available to a plain-archive installation with mode automatic
- **THEN** a notice names the new version with links to its release notes and download page, and nothing is downloaded

### Requirement: Developer builds never check
A build not made by the release packaging SHALL report the installation kind "developer build" and SHALL never check for updates, even when asked to.

#### Scenario: Developer build refuses to check
- **WHEN** a developer build runs with mode automatic and "Check for updates" is chosen
- **THEN** no request is made and the status reads "updates are off in developer builds"

### Requirement: Installing never interrupts work
An update SHALL be installed only after the application has exited: when the user quits, or through "Restart now". "Restart now" SHALL be unavailable while a scan, export or cleanup is running, and say why. Quitting with a ready update SHALL leave interrupted scans resumable as on any other quit.

#### Scenario: Restart now waits for a scan
- **WHEN** an update is ready and a scan is running
- **THEN** "Restart now" is disabled with "Available when the scan finishes", and it becomes enabled when the scan completes

#### Scenario: Quit installs a ready update
- **WHEN** an update is ready, mode is automatic, and the user quits
- **THEN** the installer starts after the application process has exited, the next start runs the new version, and the catalog and settings are unchanged by the install

### Requirement: Install waits for every program of the installation
The installer SHALL replace files only when no program of that installation (application or CLI) is running. It SHALL wait up to 10 minutes. If they are still running, it SHALL leave the installation unchanged, keep the update ready for the next quit, and record why.

#### Scenario: Running CLI postpones the install
- **WHEN** an install is started while a CLI program of the same installation is running and keeps running for 10 minutes
- **THEN** no installed file has changed, the update is still ready, and the update history records "postponed: PhotoSquirrel is still running"

### Requirement: Install keeps the previous version and checks the new one
Before replacing anything, the installer SHALL keep a complete copy of the installed version. After installing, it SHALL run the new application and CLI with `--version`. If either does not print the manifest's version, it SHALL put the kept copy back. The installation SHALL never be left with a mix of the two versions.

#### Scenario: Broken new binary rolled back at install
- **WHEN** a fake installation is updated to a payload whose CLI prints the wrong version
- **THEN** the installed files equal the previous version byte for byte, the update history records the failed check, and the user is told the version they are still on

#### Scenario: Swap is complete or not at all
- **WHEN** the installer is killed at each step of the install of a fake AppImage, bundle and per-user installation, then the install is run again
- **THEN** after every kill the installation is either fully the previous or fully the new version and starts, and the repeated install completes

### Requirement: Failed start restores the previous version
The new version's first starts SHALL be trial starts. A trial succeeds when the window draws its first frame, and on a trial start the catalog SHALL NOT be opened for writing before that. When two trial starts in a row end without success, the next start SHALL restore the kept version, start it, tell the user, and never offer the failed version again, unless the catalog was upgraded past what the kept version opens.

#### Scenario: New version that cannot start is rolled back
- **WHEN** an installed update exits before its first frame on two starts in a row
- **THEN** the third start restores the previous version, which starts and shows "PhotoSquirrel <new> could not start, so <previous> was put back", and that version is not offered again

#### Scenario: Trial start opens the catalog after the first frame
- **WHEN** an installed update starts for the first time on a catalog that needs a migration
- **THEN** the catalog is not opened for writing until the first frame is drawn and the trial is recorded, and is migrated after that

#### Scenario: Successful trial keeps the new version
- **WHEN** the new version draws its first frame on its first start
- **THEN** the trial is recorded as successful, a one-time "Updated to <version>" notice with "What's new" is shown, and the kept copy stays until the next successful update replaces it

### Requirement: Guarded manual rollback
The user SHALL be able to go back to the kept previous version from Settings or the CLI. This SHALL be refused, with the reason, when the catalog has been upgraded past the newest catalog format the previous version opens.

#### Scenario: Rollback refused after a catalog upgrade
- **WHEN** the new version has migrated the catalog to a format newer than the kept version supports and rollback is requested
- **THEN** the rollback is refused with "the catalog was upgraded by <new>; <previous> cannot open it", and nothing is changed

#### Scenario: Rollback before a catalog upgrade
- **WHEN** rollback is requested and the catalog format is one the kept version opens
- **THEN** the kept version is restored, starts, and the newer version is not offered again automatically

### Requirement: Updates touch only the installation
The updater SHALL create, replace or remove files only in the installation it updates and in its own update folder; elsewhere it SHALL only create new files in the user's Downloads folder, never replacing one. It SHALL NOT touch any source, library, quarantine, catalog, thumbnail store or settings file.

#### Scenario: User data untouched by install and rollback
- **WHEN** a fake installation is updated and rolled back with a scanned corpus, an exported library and a quarantine present
- **THEN** the trees of every source, the library, the quarantine and the data directory are byte-identical before and after, apart from the update folder

### Requirement: Notify mode
With mode notify, the application SHALL check and show an available update with its notes, and download and install it only when the user chooses "Install and restart" or "Install on quit".

#### Scenario: Notify mode waits for the user
- **WHEN** a newer version is published and mode is notify
- **THEN** a notice shows the version and its notes, no payload is downloaded until the user chooses to install, and then the install proceeds as in automatic mode

### Requirement: Off mode and manual checks
With mode off, the application SHALL never check on its own. "Check for updates" SHALL make one check whatever the mode, and report the result: up to date, available, or the failure.

#### Scenario: Manual check with mode off
- **WHEN** mode is off and the user chooses "Check for updates"
- **THEN** exactly one manifest request is made, the result is shown, and no further request is made in the next 48 hours of app time

### Requirement: Clear update notices
Every update notice SHALL name the versions involved. A failure SHALL say in plain words what failed, that the user is still on their version, and where the details are. Scheduled checks that fail SHALL show nothing until three days in a row have failed.

#### Scenario: Offline user is not nagged
- **WHEN** scheduled checks fail for lack of network on two consecutive days
- **THEN** no notice is shown, and Settings shows the last failure and the time of the last successful check

#### Scenario: Failed install explained
- **WHEN** an install is rolled back
- **THEN** the next start shows "Couldn't install PhotoSquirrel <new>. You're still on <current>." with a Details action that opens the update history

### Requirement: Urgent updates
A manifest marked urgent SHALL be shown as a banner that stays until it is acted on or dismissed, even in notify mode. Urgency SHALL NOT change what is downloaded or when it is installed.

#### Scenario: Urgent notice persists
- **WHEN** an urgent update is available with mode notify and the user dismisses nothing
- **THEN** the banner is shown on every start until the update is installed or dismissed for that version

### Requirement: Update history
The application SHALL record each check, download, install, trial, rollback and failure, with time and versions, as lines in a history file in its update folder that the user can read. It SHALL keep the last 200 entries.

#### Scenario: History records an update's life
- **WHEN** a check, download, install and successful trial complete
- **THEN** the history file has one line for each in order, with versions and times, and the CLI's update status lists the last ones

### Requirement: Signed bundles keep their signer
When the installed macOS bundle is signed with a Developer ID, the application SHALL install a new bundle only when its code signature verifies strictly and its Team ID equals the installed bundle's. Otherwise it SHALL refuse the update and keep the installed version.

#### Scenario: Bundle from another signer refused
- **WHEN** a signed fake bundle is offered a payload that passes the manifest checks but is ad-hoc signed or signed with another Team ID
- **THEN** the install is refused with "the update is not signed by the same developer", the installed bundle is unchanged, and the version is skipped

### Requirement: Blocked bundle swap falls back
When the operating system refuses to let the application replace its own bundle, the application SHALL keep the installed version unchanged, and offer the verified update in the downloads folder with instructions to replace the app by hand. It SHALL ask once, not on every quit.

#### Scenario: Refused swap falls back to a manual install
- **WHEN** the bundle swap of a fake bundle fails with a permission error
- **THEN** the installed bundle is unchanged, the verified update is saved to the downloads folder and revealed, and further quits do not retry that version
