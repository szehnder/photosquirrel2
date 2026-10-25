# Spec Delta

## ADDED Requirements

### Requirement: Undecodable photos noticed in the run summary
When a run ends with photos waiting for a missing decoder, the run summary SHALL show a one-line notice per format with the count and a "How to enable HEIC previews" disclosure giving the reason and fix for this system. When the decoder is available and photos still wait, it SHALL offer a rescan of their sources. Counts SHALL come from the run's summary, not per-file events.

#### Scenario: Notice shown for HEIC without a decoder
- **WHEN** a run over a source holding 3 HEIC photos completes with no HEIC decoder
- **THEN** the summary shows "3 HEIC photos have no preview" and expanding "How to enable HEIC previews" shows the reason and the fix steps from the decoder report

#### Scenario: Rescan offered once the decoder is available
- **WHEN** the dashboard opens with a HEIC decoder available and 3 photos of one source still waiting
- **THEN** it offers a rescan, and choosing it starts a run of that source only
