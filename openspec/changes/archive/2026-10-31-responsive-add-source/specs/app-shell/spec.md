# Spec Delta

## MODIFIED Requirements

### Requirement: First-launch onboarding
With an empty catalog the application SHALL show an onboarding view that explains that sources are only read, never changed, and offers "Add a source". When a source added from onboarding is not scanned at once, the application SHALL show the Sources view with that source's scan as the primary action.

#### Scenario: First launch shows onboarding
- **WHEN** the application starts with an empty data directory
- **THEN** the central view shows the onboarding message and an "Add a source" action

#### Scenario: Onboarding add without scanning leads to Sources
- **WHEN** "Scan new sources right away" is off and the user adds a folder from the onboarding view
- **THEN** the Sources view is shown with the new source's row reading "Not scanned yet" and no run is active
