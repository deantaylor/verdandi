#!/bin/bash
# Run after building the bridge and packaging the patched Verdandi app.
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 /path/to/Verdandi Patched.app" >&2
    exit 2
fi

repo="$(cd "$(dirname "$0")/.." && pwd)"
bridge="$repo/target/release/verdandi-ni-bridge"
app="$(cd "$1" && pwd)"
bin="$app/Contents/MacOS"
launcher="$app/Contents/Resources/Verdandi-upstream"
test -x "$bridge"
test -x "$bin/Verdandi"
test -f "$app/Contents/Info.plist"
if [ ! -e "$launcher" ]; then
    cp -p "$bin/Verdandi" "$launcher"
fi
install -m 755 "$bridge" "$bin/verdandi-ni-bridge"
install -m 755 "$repo/scripts/verdandi-patched-launcher.sh" "$bin/Verdandi"

codesign --force --sign - "$bin/verdandi-ni-bridge"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
