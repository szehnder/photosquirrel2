# Spec Delta

## ADDED Requirements

### Requirement: Damaged originals stay where they are
Cleanup SHALL NOT plan or move a source file catalogued as could not be read (damaged), even when its exact copy is in the library: the original medium may still hold the rest of the photo for a recovery tool. The plan SHALL count such files as kept, with the reason, and every other file of the source SHALL be planned as before.

#### Scenario: Cleanup keeps a damaged original
- **WHEN** a source with one cut CR2 and two intact JPEGs is fully exported and `cleanup plan` runs for it
- **THEN** the two JPEGs are planned, the cut CR2 is not planned and is reported as kept because it is damaged, and a `cleanup run` of the plan leaves the cut CR2 in place

### Requirement: A RAW is checked for a cut before a move
Immediately before a move, cleanup SHALL check the structure of a RAW source file and of a RAW library copy that makes the file safe, whatever the catalog says. A RAW found cut SHALL stay in place as damaged, and a file whose covering copy is found cut SHALL stay in place with that reason.

#### Scenario: Cleanup checks a RAW for a cut before moving
- **WHEN** a catalog calls a cut CR2 and its exact copy intact, a smaller JPEG of the same photo is covered by their library copy, and a cleanup plan of all of them runs
- **THEN** both CR2s are skipped as damaged, the smaller JPEG is skipped because its covering copy is damaged, and an unrelated JPEG is moved
