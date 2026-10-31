#!/usr/bin/env bash
# Build the local release .deb from this checkout and install it.
#
#   scripts/release-install.sh              # build + install
#   scripts/release-install.sh --launch     # …and start the app
#   scripts/release-install.sh --no-build   # install the newest existing .deb
#   scripts/release-install.sh --dry-run    # print what would happen
#
# Version comes from package.json (kept in sync with tauri.conf.json,
# Cargo.toml and Cargo.lock). Nothing is published — this is the local
# install path, not the tag-driven release workflow.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

LAUNCH=0
BUILD=1
DRY=0
for arg in "$@"; do
    case "$arg" in
        --launch) LAUNCH=1 ;;
        --no-build) BUILD=0 ;;
        --dry-run) DRY=1 ;;
        -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

VERSION="$(python3 -c 'import json;print(json.load(open("package.json"))["version"])')"

if [ "$BUILD" = 1 ]; then
    echo "==> building ZeroAny Workbench $VERSION (deb, no updater artifacts)"
    [ "$DRY" = 1 ] && echo "    (dry run — skipped)" || bunx tauri build --bundles deb --config '{"bundle":{"createUpdaterArtifacts":false}}'
fi

DEB="$(ls -t src-tauri/target/release/bundle/deb/*.deb 2>/dev/null | head -1 || true)"
if [ -z "${DEB:-}" ]; then
    echo "no .deb under src-tauri/target/release/bundle/deb — run without --no-build" >&2
    exit 1
fi
echo "==> package: $DEB"

if pgrep -f 'bin/zeroany-workbench' >/dev/null 2>&1; then
    if [ "$DRY" = 1 ]; then
        echo "==> would stop the running app"
    else
        echo "==> stopping the running app"
        pkill -f 'bin/zeroany-workbench' || true
        sleep 2
    fi
fi

echo "==> installing (needs sudo)"
if [ "$DRY" = 1 ]; then
    echo "    would run: sudo dpkg -i $DEB"
else
    sudo dpkg -i "$DEB"
fi

if [ "$LAUNCH" = 1 ]; then
    uid="$(id -u)"
    export XDG_RUNTIME_DIR="/run/user/$uid"
    export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus"
    # Plasma here is Wayland; fall back to Xwayland when that socket is absent.
    if [ -S "/run/user/$uid/wayland-0" ]; then
        export WAYLAND_DISPLAY=wayland-0
    else
        export DISPLAY="${DISPLAY:-:0}"
        xauth="$(ls -t "/run/user/$uid"/xauth_* 2>/dev/null | head -1 || true)"
        if [ -n "$xauth" ]; then export XAUTHORITY="$xauth"; fi
    fi
    echo "==> launching"
    setsid /usr/bin/zeroany-workbench >/dev/null 2>&1 &
    sleep 3
    pgrep -f 'bin/zeroany-workbench' >/dev/null && echo "==> running" || {
        echo "launch failed — start it from the app menu" >&2
        exit 1
    }
fi

echo "==> done ($VERSION installed)"