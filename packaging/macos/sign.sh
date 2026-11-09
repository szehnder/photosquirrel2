#!/usr/bin/env bash
# Sign, notarize and staple the macOS app and DMG (ADR-0128).
#
#   packaging/macos/sign.sh <unsigned app .zip> <out dir>
#
# It runs in the release workflow's `sign-macos` job, the only job that can read the signing secrets. That job
# compiles nothing: this script runs Apple's tools only (security, codesign, notarytool, stapler, hdiutil, ditto).
# It writes <out>/PhotoSquirrel.app.zip (the signed, notarized, stapled app) and
# <out>/PhotoSquirrel-<version>-macos-universal.dmg (signed, notarized, stapled). `cargo xtask macos-signed`
# then checks both in a job without secrets.
#
# Environment (secrets of the `macos_signing` environment):
#   MACOS_CERTIFICATE_P12_BASE64  Developer ID Application certificate and key, .p12, base64
#   MACOS_CERTIFICATE_PASSWORD    its export password
#   MACOS_SIGNING_IDENTITY        "Developer ID Application: <Name> (<TEAMID>)"
#   APPLE_API_KEY_ID              App Store Connect team API key id (notarization)
#   APPLE_API_ISSUER_ID           its issuer id
#   APPLE_API_KEY_P8              the .p8 key text
#
# Never `set -x` here: commands carry the keychain and certificate passwords.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <unsigned app .zip> <out dir>" >&2
  exit 2
fi
app_zip=$1
out=$2

for var in MACOS_CERTIFICATE_P12_BASE64 MACOS_CERTIFICATE_PASSWORD MACOS_SIGNING_IDENTITY \
  APPLE_API_KEY_ID APPLE_API_ISSUER_ID APPLE_API_KEY_P8; do
  if [ -z "${!var:-}" ]; then
    echo "::error::$var is not set (a secret of the macos_signing environment)" >&2
    exit 1
  fi
done

umask 077
work=$(mktemp -d)
keychain="$work/signing.keychain-db"
old_keychains=$(security list-keychains -d user | tr -d '"' | xargs)

cleanup() {
  # Every outcome: delete the keychain, restore the search list, remove the certificate and key files.
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  # shellcheck disable=SC2086 # one argument per keychain
  security list-keychains -d user -s $old_keychains >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

# 1. A throwaway keychain holding only the Developer ID certificate.
printf '%s' "$MACOS_CERTIFICATE_P12_BASE64" | base64 --decode >"$work/cert.p12"
printf '%s\n' "$APPLE_API_KEY_P8" >"$work/AuthKey.p8"
keychain_password=$(openssl rand -hex 24)
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 3600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$work/cert.p12" -k "$keychain" -P "$MACOS_CERTIFICATE_PASSWORD" -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
# shellcheck disable=SC2086
security list-keychains -d user -s "$keychain" $old_keychains
if ! security find-identity -v -p codesigning "$keychain" | grep -qF "\"$MACOS_SIGNING_IDENTITY\""; then
  echo "::error::the certificate in MACOS_CERTIFICATE_P12_BASE64 is not a valid identity named MACOS_SIGNING_IDENTITY" >&2
  security find-identity -v -p codesigning "$keychain" | sed 's/^/  /' >&2
  exit 1
fi

sign() {
  codesign --force --timestamp --keychain "$keychain" --sign "$MACOS_SIGNING_IDENTITY" "$@"
}

notarize() {
  # Submit, wait, and fail with Apple's log unless the submission is Accepted.
  local file=$1 result id status
  result=$(xcrun notarytool submit "$file" --key "$work/AuthKey.p8" --key-id "$APPLE_API_KEY_ID" \
    --issuer "$APPLE_API_ISSUER_ID" --wait --timeout 45m --output-format json)
  id=$(printf '%s' "$result" | /usr/bin/python3 -c 'import json, sys; print(json.load(sys.stdin).get("id", ""))')
  status=$(printf '%s' "$result" | /usr/bin/python3 -c 'import json, sys; print(json.load(sys.stdin).get("status", ""))')
  echo "notarization of $(basename "$file"): $status (submission $id)"
  if [ "$status" != "Accepted" ]; then
    if [ -n "$id" ]; then
      xcrun notarytool log "$id" --key "$work/AuthKey.p8" --key-id "$APPLE_API_KEY_ID" \
        --issuer "$APPLE_API_ISSUER_ID" >&2 || true
    fi
    echo "::error::notarization of $(basename "$file") was not accepted: $status" >&2
    exit 1
  fi
}

# 2. Sign the app inside out: every Mach-O file (deepest first), then the bundle. Hardened runtime, secure
#    timestamp, no --deep, no entitlements.
mkdir -p "$work/app"
ditto -x -k "$app_zip" "$work/app"
app="$work/app/PhotoSquirrel.app"
if [ ! -d "$app" ]; then
  echo "::error::$app_zip does not contain PhotoSquirrel.app" >&2
  exit 1
fi
xattr -cr "$app"
version=$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$app/Contents/Info.plist")
find "$app/Contents" -type f -print0 |
  while IFS= read -r -d '' f; do
    if file -b "$f" | grep -q 'Mach-O'; then
      printf '%s\t%s\n' "$(printf '%s' "$f" | tr -cd '/' | wc -c)" "$f"
    fi
  done | sort -rn | cut -f2- >"$work/machos.txt"
if [ ! -s "$work/machos.txt" ]; then
  echo "::error::no Mach-O file in the app" >&2
  exit 1
fi
while IFS= read -r f; do
  sign --options runtime "$f"
done <"$work/machos.txt"
sign --options runtime "$app"
codesign --verify --strict --deep "$app"

# 3. Notarize and staple the app.
ditto -c -k --keepParent "$app" "$work/notarize-app.zip"
notarize "$work/notarize-app.zip"
xcrun stapler staple "$app"

# 4. The DMG: the stapled app and an Applications link; signed, notarized, stapled.
dmg_name="PhotoSquirrel-$version-macos-universal.dmg"
mkdir -p "$work/dmg"
ditto "$app" "$work/dmg/PhotoSquirrel.app"
ln -s /Applications "$work/dmg/Applications"
hdiutil create -volname PhotoSquirrel -srcfolder "$work/dmg" -fs HFS+ -format UDZO -ov "$work/$dmg_name"
sign "$work/$dmg_name"
notarize "$work/$dmg_name"
xcrun stapler staple "$work/$dmg_name"

mkdir -p "$out"
ditto -c -k --keepParent "$app" "$out/PhotoSquirrel.app.zip"
cp "$work/$dmg_name" "$out/$dmg_name"
echo "signed, notarized and stapled: PhotoSquirrel.app ($version) and $dmg_name"
