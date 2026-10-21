# ai-queue Specification

## Purpose
Runs photo description as a durable background job that the user starts and controls, that survives restarts,
never repeats work and never sends anything without being started.

## Requirements

### Requirement: Nothing sent before start
No photo SHALL be sent to any provider until the user starts the AI queue, and stopping or pausing it SHALL stop new requests. Changing the provider, the server address or the hosted consent SHALL pause a running queue, also when the change is made in `settings.toml` by another process: a queue started from the CLI SHALL re-check the file before each request, and the app's next settings save SHALL NOT restore a consent the file withdrew.

#### Scenario: Nothing sent before start
- **WHEN** a corpus is scanned and the application idles with a recording provider configured
- **THEN** the recording provider receives no request

#### Scenario: Pause stops new requests
- **WHEN** the queue is paused while running
- **THEN** no new request starts after in-flight requests finish

#### Scenario: Changing the destination pauses the queue
- **WHEN** the queue is running and the user chooses another provider, edits the server address or withdraws consent
- **THEN** the queue is paused, and an unrelated settings change leaves it running

#### Scenario: Consent withdrawn in the settings file
- **WHEN** a queue started from the CLI is running for a hosted provider and its consent is removed from `settings.toml` (or the server address there is changed), or the app saves its settings after such an edit
- **THEN** the CLI queue pauses before its next request and sends nothing more, and the app's save keeps the consent withdrawn

### Requirement: Persisted queue order
The queue SHALL be persisted in the catalog and SHALL process keepers of class photo or likely photo first, then other keepers, in capture-date order; non-keepers SHALL be included only when the user opts in.

#### Scenario: Keepers processed first
- **WHEN** the queue is started on the scanned small corpus
- **THEN** every photo and likely-photo keeper is requested before any other item and no non-keeper is requested

### Requirement: Concurrency and rate limits
The queue SHALL respect a configurable maximum of concurrent requests (default 2) and requests per minute (default unlimited for local, 30 for hosted providers).

#### Scenario: Concurrency limit respected
- **WHEN** the queue runs with concurrency 2 against a slow recording provider
- **THEN** no more than 2 requests are ever in flight

### Requirement: Survives restarts
Queue state SHALL survive application restarts and crashes; on restart the queue SHALL be paused and resume from the first unfinished item.

#### Scenario: Queue survives restart
- **WHEN** the application is killed after 10 of 50 items are done and restarted
- **THEN** the queue shows 10 done, is paused, and resuming sends requests only for the 40 remaining items

### Requirement: One result per content
Results SHALL be stored per content identity, so byte-identical copies are described once, and an item already described by the same provider, model and schema version SHALL NOT be sent again.

#### Scenario: Duplicates analyzed once
- **WHEN** the queue runs with "include non-keepers" on a group of 3 byte-identical copies
- **THEN** exactly one request is sent and all 3 copies show the same insight

### Requirement: Prompt upgrades are explicit
Each result SHALL record the prompt version that produced it. When the prompt version changes, results from older prompts SHALL be kept, SHALL be shown as described with an older prompt, and SHALL NOT be queued or sent again on their own; re-describing them with the current prompt SHALL be an explicit user action (the AI Review view or `photosquirrel-cli ai redescribe`) that queues them without starting the queue.

#### Scenario: Older prompt results kept
- **WHEN** the catalog holds results from prompt version 1 and the queue is started with the current prompt version 2
- **THEN** no request is sent for those photos, their results are kept, and the review view and CLI show them as described with an older prompt

#### Scenario: Re-describe with current prompt
- **WHEN** the user asks to re-describe the 3 results from an older prompt and then starts the queue
- **THEN** nothing is sent before the start, exactly 3 requests are sent after it, and the new results record the current prompt version

### Requirement: Provenance stored
Each result SHALL record provider, model, schema version, request latency and time.

#### Scenario: Result records provenance
- **WHEN** an insight is stored
- **THEN** it carries the provider name, model name, schema version 1 and the measured latency

### Requirement: Bounded retries
A failed item SHALL be retried at most twice with backoff and then marked failed with its error, without stopping the queue.

#### Scenario: Failed item retried twice
- **WHEN** a recording provider returns a server error for one item every time
- **THEN** that item is attempted 3 times, marked failed, and other items complete

### Requirement: Queue progress
The queue SHALL expose a progress snapshot with done, pending, failed, in-flight counts, requests per minute and ETA, available to the application and the CLI.

#### Scenario: Queue progress reported
- **WHEN** the CLI AI status command runs with `--json` during a queue run
- **THEN** it prints done, pending, failed and in-flight counts consistent with the catalog

### Requirement: CLI control
The CLI SHALL list providers, test the configured provider, start and pause the queue, and print results with `--json`.

#### Scenario: CLI starts queue
- **WHEN** the CLI AI start command runs against a recording local provider on the scanned small corpus
- **THEN** results are stored and the CLI AI results command lists them as JSON
