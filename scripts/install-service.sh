#!/usr/bin/env bash
# Runs Catbot in the background as your own user service: it starts when you log in, comes
# back by itself if it ever stops unexpectedly, and needs no terminal or window open.
#
#   bash scripts/install-service.sh            build Catbot, install the service and start it
#   bash scripts/install-service.sh --remove   stop it and take the service away
#
# Run it from your normal terminal: the service gets this terminal's PATH, so it finds
# `claude` (and the Node it needs) wherever they're installed for you. Run it again after
# moving or reinstalling Claude Code.
#
# Afterwards:  systemctl --user status catbot      is it running?
#              journalctl --user -u catbot -f      what Catbot is saying
#              systemctl --user restart catbot     restart it (the Claude helper does this)
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$(pwd)"
NAME=catbot
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT="$UNIT_DIR/$NAME.service"
PORT=8788

say() { printf '\033[1;36m%s\033[0m\n' "$*"; }

if [[ "${1:-}" == "--remove" ]]; then
  systemctl --user disable --now "$NAME.service" 2>/dev/null || true
  rm -f "$UNIT"
  systemctl --user daemon-reload
  say "Catbot's service is gone. Start Catbot by hand with: cargo run -- serve"
  exit 0
fi

if ! command -v claude >/dev/null 2>&1; then
  echo "Can't find 'claude' on this terminal's PATH. Run this from the terminal you use Claude Code in." >&2
  exit 1
fi

# Something already on Catbot's port: a Catbot started by hand, or by the Claude helper.
already_running() { (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; }
if already_running && ! systemctl --user is-active --quiet "$NAME.service" 2>/dev/null; then
  echo "Something is already using port $PORT: probably a Catbot started by hand or by the Claude helper." >&2
  echo "Stop it first (Ctrl+C in its tab), then run this again." >&2
  exit 1
fi

say "Building Catbot's interface and server…"
bash scripts/build-ui.sh
cargo build

say "Installing the service…"
mkdir -p "$UNIT_DIR"
# Catbot's own settings, if this terminal has them, go with it.
# systemd reads % in a unit as the start of a specifier, so each % is written as %%.
unit_value() { local v="$1"; printf '%s' "${v//%/%%}"; }
extra=""
for var in CATBOT_HOME CATBOT_CODEX; do
  if [[ -n "${!var:-}" ]]; then
    extra+="Environment=\"$var=$(unit_value "${!var}")\""$'\n'
  fi
done
cat >"$UNIT" <<EOF
# Made by $REPO/scripts/install-service.sh. Run that again rather than editing this file.
[Unit]
Description=Catbot: your bots, running in the background
After=network.target

[Service]
Type=simple
WorkingDirectory=$REPO
ExecStart=$REPO/target/debug/catbot serve
Environment="PATH=$(unit_value "$PATH")"
${extra}Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
EOF

systemctl --user daemon-reload
systemctl --user enable "$NAME.service"
# Restart rather than start, so running this again picks up the new build and settings.
systemctl --user restart "$NAME.service"

for _ in $(seq 1 30); do
  already_running && break
  sleep 0.5
done
if already_running; then
  say "Catbot is running in the background: http://127.0.0.1:$PORT"
  echo "It starts by itself when you log in. To watch it: journalctl --user -u $NAME -f"
else
  echo "The service didn't come up. What it said:" >&2
  journalctl --user -u "$NAME" -n 20 --no-pager >&2 || true
  exit 1
fi
