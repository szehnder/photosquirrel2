#!/usr/bin/env bash
# Replace the unsigned macOS packages in <packages> with the signed ones in <signed>, when there are any
# (ADR-0128). Prints "signed" or "unsigned".
#
#   packaging/use-signed-macos.sh <packages dir> <signed dir>
set -euo pipefail
packages=$1
signed=$2
if [ -d "$signed" ] && compgen -G "$signed/PhotoSquirrel-*" >/dev/null; then
  rm -f "$packages"/PhotoSquirrel-*-macos-*
  mv "$signed"/* "$packages"/
  echo signed
else
  echo unsigned
fi
