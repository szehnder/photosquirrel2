#!/usr/bin/env bash
# Merge the per-platform LGPL source lists of a release (ADR-0060): `merge-lgpl-sources.sh <dir> <os>...` reads
# `<dir>/lgpl-sources-<os>.txt` for every platform, writes their sorted union to `<dir>/lgpl-sources.txt` and fails
# if a list is missing or differs from the others, naming for each platform what it lists that no other does and
# what it lacks. Every list comes from the same Cargo.lock over the same release
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

# Every component that not every platform lists is reported per platform: under a platform that lists it as
# "+ <component> (not listed by <the others>)", under one that does not as "- <component> (listed by <who lists it>)".
# The platforms whose list holds "$line".
lists() {
  for o in "$@"; do
    if normalize "$dir/lgpl-sources-$o.txt" | grep -qxF -- "$line"; then echo "$o"; fi
  done
}
status=0
for os in "$@"; do
  report=""
  while read -r line; do
    [ -n "$line" ] || continue
    listed=$(lists "$@" | paste -sd ' ' -)
    unlisted=""
    for o in "$@"; do
      case " $listed " in *" $o "*) ;; *) unlisted+="${unlisted:+, }$o" ;; esac
    done
    [ -n "$unlisted" ] || continue
    case " $listed " in
      *" $os "*) report+="  + $line (not listed by $unlisted)"$'\n' ;;
      *) report+="  - $line (listed by ${listed// /, })"$'\n' ;;
    esac
  done < "$merged"
  if [ -n "$report" ]; then
    echo "error: the $os LGPL source list differs from the other platforms':" >&2
    printf '%s' "$report" >&2
    status=1
  fi
done
echo "merged LGPL source list ($*):"
cat "$merged"
exit "$status"
