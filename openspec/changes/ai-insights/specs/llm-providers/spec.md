# Spec Delta

## Purpose

Connects PhotoSquirrel to a vision-capable language model (local LM Studio by default, or a hosted provider the
user explicitly chose) to describe photos, while keeping originals and secrets private.

## ADDED Requirements

### Requirement: Three wire formats
The application SHALL support providers speaking OpenAI-compatible chat completions, Anthropic Messages and Google Gemini, each configured with a base URL, model name and optional API key.

#### Scenario: OpenAI-compatible request shape
- **WHEN** an insight is requested from an OpenAI-compatible provider against a recorded fixture server
- **THEN** the request matches the recorded chat-completions shape with one image part and the response is parsed

#### Scenario: Anthropic request shape
- **WHEN** an insight is requested from an Anthropic provider against a recorded fixture server
- **THEN** the request matches the recorded Messages shape with one base64 image block and the response is parsed

#### Scenario: Gemini request shape
- **WHEN** an insight is requested from a Gemini provider against a recorded fixture server
- **THEN** the request matches the recorded generate-content shape with one inline image and the response is parsed

### Requirement: Local default provider
With default settings the provider SHALL be LM Studio's OpenAI-compatible endpoint at `http://localhost:1234/v1`, requiring no API key.

#### Scenario: Default is LM Studio
- **WHEN** the provider configuration is read with default settings
- **THEN** it is OpenAI-compatible at `http://localhost:1234/v1` with no API key

### Requirement: Previews only, never originals
Requests SHALL contain a JPEG preview of at most 1024 px on the long edge derived from the thumbnail pipeline, and MUST NOT contain the original file's bytes or its file path.

#### Scenario: Original never uploaded
- **WHEN** an insight is requested for a 12 MP corpus photo against a recording server
- **THEN** the uploaded image is a JPEG with a long edge of at most 1024 px, differs from the original bytes, and the request contains no source path

### Requirement: Versioned insight schema
Responses SHALL be validated against the versioned insight schema (subject, description, category, adults count, children count, pets with kind and count, is-photo verdict with confidence, tags); an invalid response SHALL be repaired by one follow-up request and otherwise recorded as failed.

#### Scenario: Valid insight parsed
- **WHEN** the provider returns a schema-valid insight with category "pets", 2 adults, 1 child and 1 dog
- **THEN** the stored insight has exactly those values and schema version 1

#### Scenario: Invalid response repaired once
- **WHEN** the first response is not valid JSON and the repair response is valid
- **THEN** exactly two requests are sent and the repaired insight is stored

#### Scenario: Second failure recorded
- **WHEN** both the first and the repair responses are invalid
- **THEN** no third request is sent and the item is failed with the validation error

### Requirement: Test connection
Provider settings SHALL offer a connection test reporting reachability, latency, whether the model exists and whether it accepts images, without sending any photo.

#### Scenario: Test unreachable provider
- **WHEN** the connection test runs against a port where nothing listens
- **THEN** it reports unreachable with a reason within 5 s and no photo is sent

### Requirement: Keys in the keychain
API keys SHALL be stored in the operating system's credential store, with `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` and `GEMINI_API_KEY` overriding stored keys; without a credential store keys SHALL be kept for the session only and the user told so. Keys MUST NOT appear in logs or the catalog.

#### Scenario: Key not written to disk
- **WHEN** an Anthropic key is saved and a request is made
- **THEN** the key string is absent from the settings file, the catalog and the log files

#### Scenario: Env var overrides key
- **WHEN** `ANTHROPIC_API_KEY` is set and a different key is stored
- **THEN** requests use the environment variable's key

### Requirement: Hosted provider consent
Selecting a non-local provider SHALL require an explicit consent that names the provider and the destination host and states that previews will be uploaded; without consent the AI queue MUST refuse to start. Consent SHALL be stored for that provider and host only: choosing another provider or editing the server address SHALL withdraw it. Requests MUST NOT follow HTTP redirects.

#### Scenario: Hosted provider needs consent
- **WHEN** the user selects Gemini without giving consent and starts the AI queue
- **THEN** the queue does not start, no request is sent and the consent prompt names Gemini

#### Scenario: Consent covers one host
- **WHEN** the user consented to Gemini at its API host and the server address is then changed to another host
- **THEN** the consent no longer applies, the queue refuses to start and the prompt names the new host

#### Scenario: Redirects are never followed
- **WHEN** a local provider answers a request with a redirect to another server
- **THEN** the request fails and nothing is sent to the other server

### Requirement: Rate limits respected
On a rate-limit response the provider layer SHALL wait for the provider's retry-after time (or exponential backoff from 2 s when absent) before retrying.

#### Scenario: Rate limit backs off
- **WHEN** a recorded server answers 429 with retry-after 3 s and then succeeds
- **THEN** the retry is sent no earlier than 3 s after the 429 and the insight is stored

### Requirement: Live local smoke test
An opt-in live test SHALL describe one corpus photo through a running local LM Studio when `PHOTOSQUIRREL_LIVE_LLM=1` is set, and SHALL be skipped otherwise.

#### Scenario: Live LM Studio smoke
- **WHEN** the live test runs with `PHOTOSQUIRREL_LIVE_LLM=1` and LM Studio serving a vision model
- **THEN** a schema-valid insight is stored for the photo
