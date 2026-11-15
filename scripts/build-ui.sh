#!/usr/bin/env bash
# Builds Catbot's interface into target/dx/catbot-ui/release/web/public,
# where `catbot serve` finds it. If the desktop app is installed (scripts/install-app.sh),
# it's rebuilt too, so the menu's Catbot opens the new build (close and reopen its window).
#
# Run from anywhere: dx 0.7.10 has to be started from the repo root (it resolves the
# workspace's default members from the current folder), so this script goes there first.
set -euo pipefail
cd "$(dirname "$0")/.."
if ! command -v dx >/dev/null 2>&1; then
  echo "dx (the Dioxus tool) isn't installed. Run scripts/setup.sh, or see README.md." >&2
  exit 1
fi
dx build -p catbot-ui --platform web --release
if [[ -f "${XDG_DATA_HOME:-$HOME/.local/share}/applications/catbot-ui.desktop" ]]; then
  echo
  echo "Rebuilding the desktop app too…"
  dx build -p catbot-ui --platform desktop --release
fi
echo
echo "Built. If Catbot runs as a service: systemctl --user restart catbot. Otherwise: cargo run -- serve."
echo "Then open http://127.0.0.1:8788"
