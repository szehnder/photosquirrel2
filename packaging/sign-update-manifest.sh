#!/usr/bin/env bash
# Signs the update manifest (ADR-0125) with the stock `minisign` tool and writes the envelope the app reads:
# {"format": 1, "manifest": "<the exact text>", "signature": "<minisign signature of that text>"}.
#
#   packaging/sign-update-manifest.sh <manifest text> <envelope out> --trusted <public key file>...
#   packaging/sign-update-manifest.sh <manifest text> <envelope out> --throwaway <public key out>
#
# With --trusted (a tagged release) the secret key's file text is read from UPDATE_SIGNING_KEY and its password
# from UPDATE_SIGNING_PASSWORD, and the signature must verify against one of the given public key files (a slot
# holding `none` is skipped). With --throwaway (a dry run) a key is generated, used once and deleted; its public
# half is written out. The job that holds the release key runs only this script: nothing is compiled beside the key.
set -euo pipefail
text=$1
out=$2
mode=$3
shift 3

umask 077
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python=$(command -v python3 || command -v python)
version=$("$python" -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])' "$text")
comment="photosquirrel-update ${version}"
sig="$work/manifest.minisig"

sign() {
  minisign -S -s "$work/key.sec" -m "$text" -x "$sig" -t "$comment" -c "PhotoSquirrel update manifest" >/dev/null
}

case "$mode" in
  --trusted)
    : "${UPDATE_SIGNING_KEY:?the release update key is not set}"
    : "${UPDATE_SIGNING_PASSWORD:?the release update key password is not set}"
    printf '%s\n' "$UPDATE_SIGNING_KEY" >"$work/key.sec"
    printf '%s\n' "$UPDATE_SIGNING_PASSWORD" | sign
    keys=("$@")
    ;;
  --throwaway)
    minisign -G -W -f -p "$work/key.pub" -s "$work/key.sec" >/dev/null
    sign
    cp "$work/key.pub" "$1"
    chmod 644 "$1"
    keys=("$1")
    echo "dry run: signed with a throwaway key ($1)"
    ;;
  *)
    echo "usage: $0 <manifest text> <envelope out> (--trusted <public key file>... | --throwaway <public key out>)" >&2
    exit 2
    ;;
esac

# The signature must be one the app accepts: by a given key, over this text, naming this version.
verified=""
for key in "${keys[@]}"; do
  line=$(grep -v '^untrusted comment:' "$key" | grep -v '^none$' | head -n 1 || true)
  if [ -n "$line" ] && minisign -V -m "$text" -x "$sig" -P "$line" >"$work/verify.txt" 2>&1; then
    verified=$key
    break
  fi
done
if [ -z "$verified" ]; then
  echo "::error::the update manifest's signature verifies against none of: ${keys[*]}" >&2
  exit 1
fi
if ! grep -qxF "Trusted comment: ${comment}" "$work/verify.txt"; then
  echo "::error::the signature does not name ${comment}" >&2
  exit 1
fi

"$python" - "$text" "$sig" "$out" <<'PY'
import json, sys
text = open(sys.argv[1], encoding="utf-8", newline="").read()
signature = open(sys.argv[2], encoding="utf-8", newline="").read()
with open(sys.argv[3], "w", encoding="utf-8", newline="") as f:
    json.dump({"format": 1, "manifest": text, "signature": signature}, f, indent=2)
PY
echo "signed the update manifest for ${version} (verified with ${verified})"
