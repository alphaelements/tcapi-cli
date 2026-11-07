#!/usr/bin/env bash
# Checks packaging/udev/99-pochi.rules: it must pass `udevadm verify` (when udevadm
# supports it) and keep the four settings the Linux setup guide in README.md relies on.
#
# Usage: ./tests/test_udev_rules.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RULES="$ROOT/packaging/udev/99-pochi.rules"
fail=0

check() {
    local desc="$1" needle="$2"
    if grep -qF -- "$needle" "$RULES"; then
        echo "  ok - $desc"
    else
        echo "  NOT ok - $desc (missing: $needle)"
        fail=1
    fi
}

[[ -f "$RULES" ]] || { echo "NOT ok - $RULES does not exist"; exit 1; }

check "matches the Pochi VID" 'ATTRS{idVendor}=="2e8a"'
check "matches the Pochi PID" 'ATTRS{idProduct}=="000a"'
check "uaccess for logged-in users" 'TAG+="uaccess"'
check "dialout group for headless use" 'GROUP="dialout"'
check "ModemManager is told to ignore the device" 'ENV{ID_MM_DEVICE_IGNORE}="1"'
check "USB autosuspend is disabled" 'ATTR{power/control}="on"'

if udevadm verify --help >/dev/null 2>&1; then
    if udevadm verify "$RULES"; then
        echo "  ok - udevadm verify"
    else
        echo "  NOT ok - udevadm verify"
        fail=1
    fi
else
    echo "  skip - this udevadm has no 'verify' subcommand"
fi

exit "$fail"
