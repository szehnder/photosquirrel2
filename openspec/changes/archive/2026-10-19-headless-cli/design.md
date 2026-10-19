# Design

## Context

ADR-0027 makes `ps-service` the only engine API and `photosquirrel-cli` its scriptable surface. Progress comes
from snapshots (ADR-0019); cancellation is cooperative (ADR-0008); doctor reports decoders (ADR-0017) and the
keychain (ADR-0031); nothing goes on the network (ADR-0029). `ground-truth-corpus` already added the clap
skeleton and `fixtures generate`.

## Goals / Non-Goals

**Goals:**
- Each engine feature from M1 is scriptable with `--json`; tests drive the binary (`assert_cmd`-style via
  `std::process::Command` on the built binary).

**Non-Goals:**
- Export/report/AI commands (later changes add them under the same conventions); a TUI.

## Decisions

- **Structure.** `photosquirrel-cli/src/cmd/{sources,scan,status,groups,doctor,fixtures}.rs`; each returns a
  serialisable output struct rendered by `render::human` or `render::json`. The JSON envelope is
  `{ "schema_version": 1, "command": "...", "data": ... }`; NDJSON snapshot lines carry `"type": "snapshot" | "summary"`.
- **Exit codes.** `0` ok, `1` operational error (`anyhow` error chain printed), `2` clap usage error, `130`
  interrupt.
- **Interrupt.** `unsafe_code = "deny"` rules out a hand-written signal handler, so the CLI adds the small
  `ctrlc` crate (workspace dependency); its handler calls `ps-service::scans::cancel`, and a second interrupt
  exits immediately (the catalog is still consistent, ADR-0008). A minor dependency, not an architectural decision.
- **Progress rendering.** A 100 ms ticker samples `snapshot()`; human mode redraws a compact multi-line block
  on a TTY and prints a line every 5 s otherwise.
- **Data dir.** Global `--data-dir` flag and `PHOTOSQUIRREL_DATA_DIR` (clap `env`), resolved by `ps-service`.
- **Doctor.** `ps-media::capabilities()`, keychain probe (`keyring` entry read of a sentinel account), catalog
  `user_version`, settings provider name; no network calls (a provider "reachable" check lives behind `ai test`).
- **Evaluate.** Reads truth, maps truth paths to catalog files via source roots named `drive-*`, calls
  `ps-fixtures::eval`, exits 1 when any gate fails.

## Risks / Trade-offs

- [JSON schema drift breaks scripts] -> `insta` JSON snapshots of every command's output on the small corpus.
- [Interrupt tests on Windows] -> use `GenerateConsoleCtrlEvent` in the test on Windows, `kill -INT` elsewhere.

## Verification

- `cargo test -p photosquirrel-cli` (all `scenario_cli_*`, snapshot tests).
- `cargo run -p photosquirrel-cli -- --data-dir /tmp/ps sources add /tmp/ps-corpus/drive-a && cargo run -p photosquirrel-cli -- --data-dir /tmp/ps scan --json | tail -1`.
- `cargo xtask ci`, `openspec validate headless-cli --strict`.
