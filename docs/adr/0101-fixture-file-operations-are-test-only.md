# 0101. Fixture file operations are test-only: a cargo feature and a layering rule

- Status: accepted
- Date: 2026-10-26
- Deciders: maintainers
- Amends: [0004](0004-non-destructive-by-construction.md) (where file-changing code may be reachable)

## Context
`ps_fixtures::scratch` writes, pads, re-times, copies, moves, links and removes files, so `ps-scan`'s tests can
arrange temporary corpora while `cargo xtask layering`'s read-only check covers them. `ps-fixtures` is also a
normal dependency of `ps-service` and `photosquirrel-cli` (corpus generation and the evaluation gate), so the
module was callable from shipped code, and the token-based move check would not notice a call to it: a remove
or rename path into a shipped binary that no check rejects.

## Decision
- `scratch` is behind the `ps-fixtures` cargo feature `scratch`, enabled only from `[dev-dependencies]`
  (`ps-scan`). Normal builds of the app and the CLI do not compile it (`cargo tree -e normal,features` shows no
  `scratch` feature for `photosquirrel` or `photosquirrel-cli`).
- `cargo xtask layering` rejects any use of `scratch::` (or an import of `scratch` from `ps_fixtures`) outside
  integration tests (`crates/*/tests/`), benches and `ps-fixtures` itself, matched on code tokens.

## Alternatives considered
- **A separate test-support crate** - one more crate for one module; the feature keeps the corpus helpers it
  shares with `ps-fixtures` in one place.
- **Only the layering rule** - a dependency that forgets the rule's paths (a new crate) would still compile the
  code into a binary.

## Consequences
- `cargo test -p ps-fixtures` alone does not run `scratch`'s own unit test; the workspace test run does (the
  feature is unified in from `ps-scan`'s dev-dependencies).

## Revisit when
Another production crate needs a fixture-writing helper (then it gets its own reviewed module in `ps-export`).
