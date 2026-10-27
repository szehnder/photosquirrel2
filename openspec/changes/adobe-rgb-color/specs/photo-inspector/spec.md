# Spec Delta

## ADDED Requirements

### Requirement: Colour space shown
The inspector's File section SHALL show a photo's colour space when it is not sRGB (Adobe RGB (1998), Display P3, or the embedded profile's description), and the loupe SHALL show the preview converted to sRGB.

#### Scenario: Adobe RGB photo names its colour space
- **WHEN** the Canon Adobe RGB JPEG fixture is selected
- **THEN** the File section shows "Colour: Adobe RGB (1998)" and the loupe shows the converted preview

#### Scenario: sRGB photo shows no colour row
- **WHEN** an untagged JPEG is selected
- **THEN** the File section shows no colour row
