# Spec Delta

## ADDED Requirements

### Requirement: Damaged originals stay where they are
Cleanup SHALL NOT plan or move a source file catalogued as could not be read (damaged), even when its exact copy is in the library: the original medium may still hold the rest of the photo for a recovery tool. The plan SHALL count such files as kept, with the reason, and every other file of the source SHALL be planned as before.

#### Scenario: Cleanup keeps a damaged original
- **WHEN** a source with one cut CR2 and two intact JPEGs is fully exported and `cleanup plan` runs for it
- **THEN** the two JPEGs are planned, the cut CR2 is not planned and is reported as kept because it is damaged, and a `cleanup run` of the plan leaves the cut CR2 in place
