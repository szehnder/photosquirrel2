# Everyday commands for PhotoSquirrel. Run `just` to list them. Install just with your package manager
# (`pacman -S just`, `brew install just`, `apt install just`, `winget install Casey.Just`) or `cargo install just`.
#
# These wrap cargo and `cargo xtask` (see AGENTS.md); anything not here works with cargo directly.

# Pass recipe arguments through as "$@", so paths with spaces stay one argument.
set positional-arguments

# GPUI builds are large: keep parallel compile jobs and test threads modest (AGENTS.md, "Machine resources").
export CARGO_BUILD_JOBS := env("CARGO_BUILD_JOBS", "4")
test_threads := env("TEST_THREADS", "4")
# Tests write temp data under target/ instead of a RAM-backed /tmp.
tmp := justfile_directory() / "target" / "tmp"
# The demo keeps its own catalog and settings here, away from your real ones.
demo := justfile_directory() / "target" / "demo"

# List the recipes.
default:
    @just --list --unsorted

# Run the desktop app (release build; the closest thing to `go run`). Uses your real catalog.
run *args:
    cargo run --release -p photosquirrel -- "$@"

# Run the desktop app as a debug build: compiles faster, runs slower.
run-debug *args:
    cargo run -p photosquirrel -- "$@"

# Run the command-line tool, e.g. `just cli sources add ~/Pictures`, `just cli scan`, `just cli report`.
cli *args:
    cargo run --release -p photosquirrel-cli -- "$@"

# Make a sample photo corpus, scan it into a throwaway catalog under target/demo, and open the app on it.
demo seed="7":
    #!/usr/bin/env bash
    set -euo pipefail
    export PHOTOSQUIRREL_DATA_DIR="{{ demo }}/data" PHOTOSQUIRREL_CONFIG_DIR="{{ demo }}/config"
    # Set up once; a half-finished setup (no catalog yet) starts over.
    if [ ! -d "{{ demo }}/data" ]; then
        rm -rf "{{ demo }}/corpus"
        cargo run --release -p photosquirrel-cli -- fixtures generate --out "{{ demo }}/corpus" --seed {{ seed }}
        cargo run --release -p photosquirrel-cli -- sources add "{{ demo }}/corpus"
        cargo run --release -p photosquirrel-cli -- scan
    fi
    cargo run --release -p photosquirrel

# Start the demo over from a fresh corpus and catalog.
demo-reset:
    rm -rf "{{ demo }}"

# Run tests: `just test` for everything, `just test -p ps-scan`, or `just test -p ps-scan --test sources`.
test *args:
    mkdir -p "{{ tmp }}"
    TMPDIR="{{ tmp }}" cargo test "$@" -- --test-threads={{ test_threads }}

# Compile everything without running it (fast feedback while editing).
check:
    cargo check --workspace --all-targets

# Format the code.
fmt:
    cargo fmt --all

# Lint the way CI does.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Everything CI runs: fmt, clippy, layering, design tokens, tests, spec checks, coverage.
ci:
    #!/usr/bin/env bash
    set -euo pipefail
    # The spec step needs the openspec CLI; without a global install, run it through npx.
    if ! command -v openspec >/dev/null; then
        mkdir -p "{{ tmp }}/bin"
        printf '#!/bin/sh\nexec npx -y @fission-ai/openspec@2.1.1 "$@"\n' > "{{ tmp }}/bin/openspec"
        chmod +x "{{ tmp }}/bin/openspec"
        export PATH="{{ tmp }}/bin:$PATH"
    fi
    cargo xtask ci

# Check which optional tools this machine has (libheif, ffmpeg, LM Studio).
doctor:
    cargo xtask doctor

# Capture the app window to PNGs under target/screenshots, e.g. `just screenshot -- --theme dark`.
screenshot *args:
    cargo xtask screenshot "$@"

# Check the performance budgets.
bench:
    cargo xtask bench --check

# Build the Linux installers into target/packages (needs Docker).
package:
    cargo xtask package --docker
