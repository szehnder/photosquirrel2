# Spec Delta

## ADDED Requirements

### Requirement: TIFF decode budgets
The TIFF analysis decode SHALL have two budgets: the median analysis decode plus grid thumbnail of a 12 MP 16-bit LZW TIFF from memory on one core, and the working memory of a streamed decode of a 24 MP 16-bit TIFF.

#### Scenario: TIFF analysis decode within budget
- **WHEN** the TIFF decode benchmark runs single-threaded on 12 MP 16-bit LZW TIFF fixtures
- **THEN** the median per-file time is reported as a budget measurement and is within its budget

#### Scenario: Streamed TIFF decode memory within budget
- **WHEN** a 24 MP 16-bit TIFF is decoded through a reader
- **THEN** the decoder's working memory is reported as a budget measurement and is within its budget
