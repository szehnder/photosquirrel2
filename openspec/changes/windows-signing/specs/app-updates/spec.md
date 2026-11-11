# Spec Delta

## ADDED Requirements

### Requirement: Signed Windows installers keep their signer
When the installed Windows app is Authenticode-signed, the application SHALL run a downloaded installer only when its Authenticode signature verifies and its signer equals the installed app's signer, or the next signer that the installed version names. Otherwise it SHALL refuse the update and keep the installed version. An unsigned installation SHALL rely on the manifest checks alone.

#### Scenario: Installer from another signer refused
- **WHEN** a signed fake installation is offered an installer that passes the manifest checks but is unsigned, has a signature that does not verify, or is signed by another signer
- **THEN** the installer is not started, the install is refused with "the update is not signed by the same developer", the installed files are unchanged, and the version is skipped

#### Scenario: Same signer accepted with a renewed certificate
- **WHEN** a signed fake installation is offered an installer whose signature verifies with the same signer identity but another certificate (new serial number and validity)
- **THEN** the installer is run and the update is installed

#### Scenario: Announced next signer accepted
- **WHEN** a signed fake installation whose version names a next signer is offered an installer whose signature verifies with that next signer
- **THEN** the installer is run and the update is installed

#### Scenario: Unsigned installation moves to a signed installer
- **WHEN** an unsigned fake installation is offered an installer that passes the manifest checks, signed or not
- **THEN** the signer check does not refuse it and the manifest checks alone decide
