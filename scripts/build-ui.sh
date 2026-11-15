#!/usr/bin/env bash
# Builds Catbot's interface into target/dx/catbot-ui/release/web/public,
# where `catbot serve` finds it.
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
echo
echo "Built. If Catbot runs as a service: systemctl --user restart catbot. Otherwise: cargo run -- serve."
echo "Then open http://127.0.0.1:8788"
