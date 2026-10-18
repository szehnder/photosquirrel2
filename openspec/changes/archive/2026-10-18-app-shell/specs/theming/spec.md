# Spec Delta

## Purpose

Gives PhotoSquirrel a consistent, content-first look where photos carry the colour, through design tokens and
themes instead of ad-hoc styling.

## ADDED Requirements

### Requirement: Tokens only
All colours, spacing, radii, type sizes and motion durations in the interface SHALL come from design tokens; the token check MUST fail when interface code contains a raw colour or pixel literal outside theme modules.

#### Scenario: Raw colour literal rejected
- **WHEN** the token check runs on interface code containing a raw colour literal outside a theme module
- **THEN** it fails and names the file and line

### Requirement: Graphite dark default and light theme
The default theme SHALL be a dark neutral graphite theme; a light theme SHALL be available and switching themes SHALL apply immediately without restart.

#### Scenario: Dark theme by default
- **WHEN** the application starts with default settings
- **THEN** the active theme is the dark graphite theme

#### Scenario: Switch to light live
- **WHEN** the user selects the light theme in settings
- **THEN** every open view repaints in the light theme without restarting

### Requirement: Acorn accent
The accent colour SHALL be the warm amber "acorn" token, used for selection, focus and primary actions in both themes.

#### Scenario: Accent is acorn
- **WHEN** the selection highlight colour is read from both themes
- **THEN** it is the acorn accent token in each

### Requirement: Neutral photo surround
Surfaces behind photos SHALL be neutral grey (chroma below 0.02 in Oklch) so they do not tint the perception of photo colours.

#### Scenario: Photo surround is neutral
- **WHEN** the gallery and loupe background tokens of both themes are checked
- **THEN** each has Oklch chroma below 0.02

### Requirement: Readable contrast
Text tokens SHALL meet a contrast ratio of at least 4.5:1 against the surfaces they are used on, and secondary text at least 3:1, in both themes.

#### Scenario: Text contrast meets AA
- **WHEN** every text-on-surface token pair of both themes is checked
- **THEN** primary text pairs are at least 4.5:1 and secondary text pairs at least 3:1

### Requirement: Typography
Interface text SHALL use Inter and technical metadata (EXIF values, hashes, paths) SHALL use JetBrains Mono, both bundled with the application.

#### Scenario: Metadata uses mono font
- **WHEN** the inspector renders a content hash
- **THEN** it is drawn with the JetBrains Mono font from the bundled assets

### Requirement: Reduced motion
When reduced motion is requested by the operating system or settings, motion duration tokens SHALL resolve to 0 ms.

#### Scenario: Reduced motion disables animation
- **WHEN** reduced motion is enabled in settings
- **THEN** every motion duration token resolves to 0 ms
