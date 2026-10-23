#!/usr/bin/env bash
# Merge the per-platform LGPL source lists of a release (ADR-0060): `merge-lgpl-sources.sh <dir> <os>...` reads
# `<dir>/lgpl-sources-<os>.txt` for every platform, writes their sorted union to `<dir>/lgpl-sources.txt` and fails
# if a list is missing or differs from the union. Every list comes from the same Cargo.lock over the same release
# targets (about.toml), so they should be equal; a difference means a cargo-about or feature mismatch that a person
# must look at before the source offer is trusted. The union is written even then, so the error shows what would be
# attached. Used by .github/workflows/release.yml.
set -euo pipefail

if [ $# -lt 2 ]; then
  echo "usage: $0 <dir> <os>..." >&2
  exit 2
fi
dir=$1
shift

# One `name version` per line, without carriage returns or blank lines, sorted and unique.
normalize() { tr -d '\r' < "$1" | sed '/^[[:space:]]*$/d' | LC_ALL=C sort -u; }

missing=0
for os in "$@"; do
  if [ ! -f "$dir/lgpl-sources-$os.txt" ]; then
    echo "error: the $os LGPL source list ($dir/lgpl-sources-$os.txt) is missing" >&2
    missing=1
  fi
done
[ "$missing" -eq 0 ] || exit 1

merged="$dir/lgpl-sources.txt"
for os in "$@"; do normalize "$dir/lgpl-sources-$os.txt"; done | LC_ALL=C sort -u > "$merged"

status=0
for os in "$@"; do
  if ! difference=$(diff <(normalize "$dir/lgpl-sources-$os.txt") "$merged"); then
    echo "error: the $os LGPL source list differs from the union of all platforms (> only in the union):" >&2
    echo "$difference" >&2
    status=1
  fi
done
echo "merged LGPL source list ($*):"
cat "$merged"
exit "$status"
