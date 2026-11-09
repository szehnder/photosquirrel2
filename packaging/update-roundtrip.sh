#!/usr/bin/env bash
# The update round trip of a tagged release (spec release-packaging "Update round trip before publishing";
# ADR-0126): install the previous published release that has an updater, let it update itself to the candidate
# through the signed manifest served from a local server, and check that the candidate is what is installed and
# that its trial start is recorded. Runs on each OS's runner before anything is published.
#
#   packaging/update-roundtrip.sh <linux|macos|windows> <candidate dir>
#
# <candidate dir> holds the candidate's packages and its signed update-manifest.json. Needs GH_TOKEN (read-only)
# and GITHUB_REPOSITORY, GITHUB_REF_NAME and RUNNER_TEMP from Actions.
set -euo pipefail
os=$1
candidate=$(cd "$2" && pwd)
version=${GITHUB_REF_NAME#v}
work="$RUNNER_TEMP/update-roundtrip"
rm -rf "$work"
mkdir -p "$work"

# The newest earlier release (not a prerelease) that publishes an update manifest.
previous=""
for tag in $(gh release list --repo "$GITHUB_REPOSITORY" --exclude-pre-releases --exclude-drafts \
  --limit 30 --json tagName --jq '.[].tagName'); do
  [ "$tag" = "$GITHUB_REF_NAME" ] && continue
  if gh release view "$tag" --repo "$GITHUB_REPOSITORY" --json assets --jq '.assets[].name' |
    grep -qx update-manifest.json; then
    previous=$tag
    break
  fi
done
if [ -z "$previous" ]; then
  echo "::notice::no earlier release has an updater: nothing to update from (this is the first release with one)"
  exit 0
fi
echo "updating ${previous} -> v${version} on ${os}"

download() {
  gh release download "$previous" --repo "$GITHUB_REPOSITORY" --pattern "$1" --dir "$work/prev"
}

# Install the previous release the way a user would.
case "$os" in
  linux)
    download '*-linux-x86_64.AppImage'
    app=$(ls "$work"/prev/*.AppImage)
    chmod +x "$app"
    run_app() { "$app" "$@"; }
    installed_version() { "$app" --version; }
    ;;
  macos)
    download '*-macos-universal.dmg'
    mnt="$work/mnt"
    mkdir -p "$mnt" "$work/Applications"
    printf 'Y\n' | PAGER='cat' hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$work"/prev/*.dmg >/dev/null
    ditto "$mnt/PhotoSquirrel.app" "$work/Applications/PhotoSquirrel.app"
    hdiutil detach "$mnt"
    bin="$work/Applications/PhotoSquirrel.app/Contents/MacOS/photosquirrel"
    run_app() { "$bin" "$@"; }
    installed_version() { "$bin" --version; }
    ;;
  windows)
    download '*-windows-x86_64-setup.exe'
    "$(ls "$work"/prev/*-setup.exe)" /S /NS
    exe="$LOCALAPPDATA/PhotoSquirrel/photosquirrel.exe"
    run_app() { "$exe" "$@"; }
    installed_version() { "$exe" --version; }
    ;;
  *)
    echo "unknown OS $os" >&2
    exit 2
    ;;
esac
echo "installed: $(installed_version)"

# Serve the candidate's packages and signed manifest locally; the old updater checks every signature and hash.
python=$(command -v python3 || command -v python)
"$python" -m http.server 8765 --bind 127.0.0.1 --directory "$candidate" >"$work/server.log" 2>&1 &
server=$!
trap 'kill $server 2>/dev/null || true' EXIT
sleep 2
export PHOTOSQUIRREL_UPDATE_SERVER=http://127.0.0.1:8765
export PHOTOSQUIRREL_UPDATE_DIR="$work/updates"
export PHOTOSQUIRREL_DATA_DIR="$work/data"

# The old app checks, downloads, verifies and starts the installer helper, then exits.
run_app --update-now

# The helper replaces the installation once the app has exited.
for _ in $(seq 1 120); do
  if installed_version | grep -qx "photosquirrel ${version}"; then
    break
  fi
  sleep 5
done
now=$(installed_version)
if [ "$now" != "photosquirrel ${version}" ]; then
  echo "::error::after the update the installed app reports '${now}', expected photosquirrel ${version}"
  cat "$work/updates/helper.log" 2>/dev/null || true
  cat "$work/updates/history.jsonl" 2>/dev/null || true
  exit 1
fi
if ! grep -q "\"trial\"" "$work/updates/state.json"; then
  echo "::error::the install did not record the new version's trial start"
  exit 1
fi
echo "round trip ${previous} -> v${version} on ${os}: OK"
cat "$work/updates/history.jsonl"
