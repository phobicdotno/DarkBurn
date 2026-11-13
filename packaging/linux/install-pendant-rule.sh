#!/bin/sh
# Installs the udev rule that lets the logged-in user open the XHC wireless pendant receiver
# (USB 10ce:eba0), then reloads udev. Unplug and plug in the receiver afterwards if it was in.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
rule="$here/70-darkburn-pendant.rules"
dest=/etc/udev/rules.d/70-darkburn-pendant.rules
if [ "$(id -u)" -eq 0 ]; then
    run() { "$@"; }
elif command -v pkexec >/dev/null 2>&1 && [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    run() { pkexec "$@"; }
else
    run() { sudo "$@"; }
fi
run install -m 0644 "$rule" "$dest"
run udevadm control --reload
run udevadm trigger --subsystem-match=hidraw
echo "Installed $dest. Unplug the pendant receiver and plug it in again if DarkBurn still cannot open it."
