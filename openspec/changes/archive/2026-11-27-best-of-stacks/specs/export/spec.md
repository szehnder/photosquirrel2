## ADDED Requirements

### Requirement: Best of export choice
Each export SHALL offer what to export: everything (the default), the best of each stack only, or both. "Best of only" SHALL copy each stack's pick and every photo in no stack, and no other shot of a stack. The plan SHALL show the counts and bytes of each part before copying. The choice SHALL apply to that export only, and the default SHALL stay everything.

#### Scenario: Best of only
- **WHEN** the small corpus is exported with best of only
- **THEN** the library holds exactly the picks and the unstacked keepers, and no other shot of any stack

#### Scenario: Default unchanged
- **WHEN** an export is planned without choosing
- **THEN** it exports everything, as before this change

### Requirement: Both everything and the best of
"Both" SHALL export everything, and also export the best-of set (each stack's pick and every photo in no stack) under `Best of/` in the library, in the dated layout, as a library of its own.

#### Scenario: Both
- **WHEN** the small corpus is exported with both
- **THEN** the library holds every keeper in the dated layout and every pick also under `Best of/`, each copy hash-verified
