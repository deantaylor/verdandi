#!/bin/bash
set -eu

bundle_bin="$(cd "$(dirname "$0")" && pwd)"
launcher="$bundle_bin/../Resources/Verdandi-upstream"

# An explicit socket lets Terminal users run the bridge with custom options.
if [ "${VERDANDI_FRAME_SOCKET+x}" = x ]; then
    exec "$launcher" "$@"
fi

# Check before the upstream launcher rewrites the runtime symlink in ~/norns.
if /usr/bin/pgrep -x 'norns|Verdandi' >/dev/null; then
    echo "Close the running Verdandi window before opening Verdandi Patched." >&2
    exit 1
fi

bridge="$bundle_bin/verdandi-ni-bridge"
export VERDANDI_FRAME_SOCKET="$( "$bridge" --print-socket )"
log_dir="$HOME/Library/Logs/Verdandi Patched"
mkdir -p "$log_dir"

RUST_LOG="${VERDANDI_NI_LOG:-info}" "$bridge" > "$log_dir/bridge.log" 2>&1 &
bridge_pid=$!
trap 'kill "$bridge_pid" 2>/dev/null || true; wait "$bridge_pid" 2>/dev/null || true' EXIT

"$launcher" "$@" &
launcher_pid=$!
trap 'kill "$launcher_pid" 2>/dev/null || true; wait "$launcher_pid" 2>/dev/null || true; exit 143' TERM INT
wait "$launcher_pid"
