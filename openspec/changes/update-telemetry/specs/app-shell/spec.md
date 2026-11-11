# Spec Delta

## MODIFIED Requirements

### Requirement: Quiet start
The application MUST NOT open any network connection at start-up or within the first 60 seconds. While idle, it MUST NOT open any connection except the update checks the user allowed, which go only to the update hosts, and the health and crash reports the user turned on, which go only to the telemetry host.

#### Scenario: No network at startup
- **WHEN** the application starts and idles for 10 s with outbound connections recorded
- **THEN** no network connection was attempted

#### Scenario: No network while updates are undecided or off
- **WHEN** the application starts with the update mode undecided, and again with it off, and each idles for 48 hours of app time with the update transport and sockets recorded
- **THEN** no network connection was attempted in either run

#### Scenario: Only the update host while updates are on
- **WHEN** the application starts with the update mode automatic and idles for 10 minutes of app time with the update transport recorded
- **THEN** nothing was requested in the first 60 s, and every request made was to the manifest address

#### Scenario: No telemetry host without the opt-in
- **WHEN** the application starts with the update mode automatic and health reports undecided, and again with health reports off, and each runs for 48 hours of app time with both transports and sockets recorded
- **THEN** every request made was to the manifest address and none was to the telemetry host

#### Scenario: Only the telemetry host after the opt-in
- **WHEN** the application starts with the update mode off and health reports on, and runs for 48 hours of app time with both transports recorded
- **THEN** nothing was requested in the first 60 s, and every request made was a report to the telemetry address
