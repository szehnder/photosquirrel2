#!/usr/bin/env bash
# The update round trip of a tagged release (spec release-packaging "Update round trip before publishing";
# ADR-0126): install the previous published release that has an updater, let it update itself to the candidate
# through the signed manifest served from a local server, and check that the candidate is what is installed and
# that its trial start is recorded. Runs on each OS's runner before anything is published.
#
#   packaging/update-roundtrip.sh <linux|macos|windows> <candidate dir> [rehearsal]
#
# <candidate dir> holds the candidate's packages and its signed update-manifest.json. Needs GH_TOKEN (read-only)
# and GITHUB_REPOSITORY, GITHUB_REF_NAME and RUNNER_TEMP from Actions.
#
# With `rehearsal` (a dry run, whose manifest is signed by a throwaway key no release trusts) nothing can be
# updated, so the candidate itself is installed the way a user would and must (a) start and report its version,
# (b) refuse the served manifest for its signature, and (c) be unchanged afterwards. That runs the same install
# steps, server and `--update-now` as the real round trip, on every OS, before the first tag.
set -euo pipefail
os=$1
candidate=$(cd "$2" && pwd)
mode=${3:-roundtrip}
work="$RUNNER_TEMP/update-roundtrip"
rm -rf "$work"
mkdir -p "$work/prev"
python=$(command -v python3 || command -v python)

if [ "$mode" = rehearsal ]; then
  version=$("$python" -c 'import json,sys; print(json.loads(json.load(open(sys.argv[1]))["manifest"])["version"])' \
    "$candidate/update-manifest.json")
  previous="the candidate"
  download() {
    # shellcheck disable=SC2086  # the pattern is a glob on purpose
    cp "$candidate"/$1 "$work/prev/"
  }
else
  version=${GITHUB_REF_NAME#v}

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
  download() {
    gh release download "$previous" --repo "$GITHUB_REPOSITORY" --pattern "$1" --dir "$work/prev"
  }
fi
echo "${mode}: ${previous} -> v${version} on ${os}"

# Install the previous release the way a user would.
case "$os" in
  linux)
    download '*-linux-x86_64.AppImage'
    app=$(ls "$work"/prev/*.AppImage)
    chmod +x "$app"
    # A runner without FUSE runs the image unpacked; the image still knows its own path ($APPIMAGE).
    "$app" --version >/dev/null 2>&1 || export APPIMAGE_EXTRACT_AND_RUN=1
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
    # Git Bash rewrites arguments that start with a slash into Windows paths; the installer would then not see
    # its silent switch and would wait for a click that never comes.
    MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' "$(ls "$work"/prev/*-setup.exe)" /S /NS
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
"$python" -m http.server 8765 --bind 127.0.0.1 --directory "$candidate" >"$work/server.log" 2>&1 &
server=$!
trap 'kill $server 2>/dev/null || true' EXIT
# Wait until the server answers rather than for a fixed time: a cold Python start on a Windows runner (Defender
# scans the new process) can take longer than a few seconds, and the app would then find nothing listening.
ready=""
for _ in $(seq 1 120); do
  if ! kill -0 "$server" 2>/dev/null; then
    break
  fi
  if curl -fs -o /dev/null --max-time 5 http://127.0.0.1:8765/update-manifest.json 2>/dev/null; then
    ready=1
    break
  fi
  sleep 0.5
done
if [ -z "$ready" ]; then
  echo "::error::the local update server did not start serving the candidate"
  cat "$work/server.log" 2>/dev/null || true
  exit 1
fi
export PHOTOSQUIRREL_UPDATE_SERVER=http://127.0.0.1:8765
export PHOTOSQUIRREL_UPDATE_DIR="$work/updates"
export PHOTOSQUIRREL_DATA_DIR="$work/data"

if [ "$mode" = rehearsal ]; then
  before=$(installed_version)
  if [ "$before" != "photosquirrel ${version}" ]; then
    echo "::error::the installed candidate reports '${before}', expected photosquirrel ${version}"
    exit 1
  fi
  # The manifest is signed by a throwaway key: a release build must refuse it and change nothing.
  if run_app --update-now; then
    echo "::error::the candidate accepted a manifest that the release key did not sign"
    exit 1
  fi
  if ! grep -q "update manifest signature is not valid" "$work/updates/state.json"; then
    echo "::error::the check did not fail for the signature:"
    cat "$work/updates/state.json" 2>/dev/null || echo "(no update state was written)"
    exit 1
  fi
  if [ -e "$work/updates/ready" ] || [ -e "$work/updates/helper" ]; then
    echo "::error::something was downloaded or staged from an untrusted manifest"
    exit 1
  fi
  sleep 5
  after=$(installed_version)
  if [ "$after" != "$before" ]; then
    echo "::error::the installation changed: '${before}' -> '${after}'"
    exit 1
  fi
  echo "rehearsal on ${os}: v${version} installs, starts, and refuses a manifest the release key did not sign: OK"
  exit 0
fi

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
