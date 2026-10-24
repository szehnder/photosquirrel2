# Spec Delta

## ADDED Requirements

### Requirement: Identity from bytes fed during another read
An identity computed from the bytes of a file fed in file order while it is read for another purpose SHALL equal the identity computed by reading the file for hashing alone, for full and sampled identities, and SHALL report which bytes it still needs.

#### Scenario: Incremental identity equals one-shot identity
- **WHEN** a 10 MB file and a 300 MB file are fed to the incremental identity in uneven pieces, skipping what it does not need
- **THEN** both identities equal the identities of reading the files for hashing alone, and the 300 MB file needed only its size and three 4 MB windows
