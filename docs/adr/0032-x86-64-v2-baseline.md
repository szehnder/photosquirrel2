# 0032. x86-64-v2 CPU baseline

- Status: accepted
- Date: 2026-10-17
- Deciders: maintainers

## Context
Hashing and pixel code benefit from SSE4.2/POPCNT (Hamming distances use popcount heavily).

## Decision
x86-64 builds target `x86-64-v2` (`.cargo/config.toml`), supported by every x86-64 CPU since ~2009. AArch64 builds use the default target (NEON is baseline).

## Alternatives considered
- **Runtime dispatch everywhere** - complexity for little gain.

## Consequences
- Very old CPUs (pre-2009) are unsupported.

## Revisit when
Never.
