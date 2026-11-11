# 0132. The job that holds the update key compiles nothing (amends 0125)

- Status: accepted
- Date: 2026-11-10

## Context

ADR-0125 keeps the release update key in the protected `release-signing` environment and lets only a `sign` job
read it. As first built, that job compiled xtask before the step that was given the key. Compiling xtask runs the
build scripts of every one of its dependencies on the runner, so a compromised dependency could have replaced
`target/debug/xtask` and read the key in the next step. The gate review of `auto-updates` found this (M4).
`sign-macos` (ADR-0128) already avoids it: it compiles nothing.

## Decision

- A job that holds no secret, `update-manifest-text`, builds xtask and writes the manifest's exact text for the
  packages of the run (`cargo xtask update-manifest --unsigned-out`).
- The job that holds the key, `update-manifest`, runs no cargo, toolchain or cache step. It installs the
  distribution's `minisign` and runs `packaging/sign-update-manifest.sh`, which signs that text (trusted comment
  `photosquirrel-update <version>`), checks the signature against the two public key slots and writes the
  envelope. The key lives in a private temporary folder that is deleted on every outcome.
- A dry run runs the same script with a key it generates, so the signing path is exercised before every tag.
- xtask no longer has a way to read the release key. It can still sign with a throwaway key for local trials.

## Consequences

- What runs beside the key is one reviewed script and a distribution package, not hundreds of crates.
- The app, the release job (stock `minisign -V`) and the round trip verify the result as before; the envelope
  format is unchanged.
- The owner's approval of `release-signing` now gates a job that takes seconds.
