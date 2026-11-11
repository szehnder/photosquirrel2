# Spec Delta

## MODIFIED Requirements

### Requirement: First-launch onboarding
With an empty catalog the application SHALL show an onboarding view that explains that sources are only read, never changed, and offers "Add a source". The onboarding view SHALL also ask the update question, with "Keep me up to date" preselected, until it is answered. When a source added from onboarding is not scanned at once, the application SHALL show the Sources view with that source's scan as the primary action.

#### Scenario: First launch shows onboarding
- **WHEN** the application starts with an empty data directory
- **THEN** the central view shows the onboarding message, an "Add a source" action and the update question with "Keep me up to date" preselected

#### Scenario: Onboarding add without scanning leads to Sources
- **WHEN** "Scan new sources right away" is off and the user adds a folder from the onboarding view
- **THEN** the Sources view is shown with the new source's row reading "Not scanned yet" and no run is active

### Requirement: Quiet start
The application MUST NOT open any network connection at start-up or within the first 60 seconds. While idle, it MUST NOT open any connection except the update checks the user allowed, which go only to the update hosts.

#### Scenario: No network at startup
- **WHEN** the application starts and idles for 10 s with outbound connections recorded
- **THEN** no network connection was attempted

#### Scenario: No network while updates are undecided or off
- **WHEN** the application starts with the update mode undecided, and again with it off, and each idles for 48 hours of app time with the update transport and sockets recorded
- **THEN** no network connection was attempted in either run

#### Scenario: Only the update host while updates are on
- **WHEN** the application starts with the update mode automatic and idles for 10 minutes of app time with the update transport recorded
- **THEN** nothing was requested in the first 60 s, and every request made was to the manifest address
