## ADDED Requirements

### Requirement: Best of filter
The Library SHALL offer a "Best of" filter that shows each stack's pick and every photo that is in no stack, and hides the other shots of every stack. The filter SHALL follow pick changes and threshold changes without a reload of the whole gallery.

#### Scenario: Best of shows one shot per stack
- **WHEN** the user turns on Best of on the small corpus
- **THEN** each stack contributes exactly its pick, every unstacked photo is shown, and the count shown equals unstacked photos plus stacks

#### Scenario: Best of follows a new pick
- **WHEN** Best of is on and the user picks another shot in a stack
- **THEN** the gallery shows the new pick instead of the old one
