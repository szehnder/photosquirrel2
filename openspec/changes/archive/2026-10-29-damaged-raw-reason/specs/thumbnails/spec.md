# Spec Delta

## ADDED Requirements

### Requirement: Damaged RAW files told apart from unsupported ones
A RAW file whose own structure records a directory, strip, tile or preview that ends past the end of the file SHALL get a "could not be read" placeholder that says it was cut short, even when its embedded preview is intact, and SHALL NOT get perceptual hashes. A RAW from a camera the decoder does not know, or with no embedded preview and no such evidence, SHALL stay a format variant not supported.

#### Scenario: RAW cut inside its preview is damaged
- **WHEN** a CR2 fixture cut inside its embedded preview JPEG is analysed
- **THEN** its placeholder reason is "could not be read" with a message giving the file's length and the length its data needs, and the record carries RAW decoder generation 1

#### Scenario: RAW cut after an intact preview is damaged
- **WHEN** a CR2 fixture whose embedded preview is whole but whose sensor data is cut is analysed
- **THEN** its placeholder reason is "could not be read", no thumbnail pixels and no perceptual hashes are stored, and it is not grouped with its camera JPEG

#### Scenario: Unknown camera is still not supported
- **WHEN** a complete CR2-like file without a usable embedded preview, from a camera missing from the RAW decoder's camera table, is analysed
- **THEN** its placeholder reason is a variant not supported, with a detail naming the camera's make and model

#### Scenario: Complete RAW fixtures are never damaged
- **WHEN** every complete RAW fixture that `ps-fixtures` writes, including one with a private tag whose count runs past the end of the file, is analysed
- **THEN** none of them gets a "could not be read" placeholder
