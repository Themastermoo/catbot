#!/usr/bin/env bash
# Gives Catbot a public https address with Tailscale Funnel, so a paired phone can reach it
# from anywhere: https://<this machine>.<your tailnet>.ts.net, carried to Catbot's remote door
# (127.0.0.1:8789). Only paired devices get past that door; see README.md, "Away from home".
#
#   bash scripts/remote-setup.sh         set it up (or check it), and show the address
#   bash scripts/remote-setup.sh --off   take the public address away again
#
# Before the first run, once:
#   1. Install Tailscale:      curl -fsSL https://tailscale.com/install.sh | sh
#   2. Sign in:                sudo tailscale up            (opens a browser page)
#   3. Let yourself run it:    sudo tailscale set --operator=$USER
# The first Funnel run may print a link to turn on HTTPS and Funnel for your tailnet: open it,
# approve, and run this script again.
set -euo pipefail
DOOR_PORT="${CATBOT_REMOTE_PORT:-8789}"

say() { printf '\033[1;36m%s\033[0m\n' "$*"; }
stop() { printf '\033[1;31m%s\033[0m\n' "$*" >&2; exit 1; }

command -v tailscale >/dev/null 2>&1 || stop "Tailscale isn't installed. Install it with:
    curl -fsSL https://tailscale.com/install.sh | sh
then sign in with:  sudo tailscale up
and run this again."

if [[ "${1:-}" == "--off" ]]; then
  tailscale funnel --https=443 off
  say "Catbot has no public address now. Paired devices stay paired for when it's back."
  exit 0
fi

status="$(tailscale status --json 2>/dev/null)" || stop "Tailscale isn't running. Start it with:  sudo tailscale up"
state="$(printf '%s' "$status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("BackendState",""))')"
[[ "$state" == "Running" ]] || stop "Tailscale isn't signed in (it says: ${state:-unknown}). Sign in with:  sudo tailscale up"
name="$(printf '%s' "$status" | python3 -c 'import json,sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))')"
[[ -n "$name" ]] || stop "Tailscale didn't say this machine's name."

# Catbot's remote door has to be there to carry anything to.
if ! (exec 3<>"/dev/tcp/127.0.0.1/$DOOR_PORT") 2>/dev/null; then
  stop "Catbot's remote door isn't open on 127.0.0.1:$DOOR_PORT. Start Catbot first:
    systemctl --user restart catbot       (or: bash scripts/install-service.sh)"
fi

say "Carrying https://$name to Catbot's remote door (127.0.0.1:$DOOR_PORT)…"
if ! tailscale funnel --bg "$DOOR_PORT"; then
  stop "Tailscale wouldn't start Funnel. If it printed a link, open it to turn on HTTPS and
Funnel for your tailnet, then run this again. If it said permission denied, run once:
    sudo tailscale set --operator=\$USER"
fi

# Through the public address, Catbot's door should answer "not paired" (401) to us.
say "Checking it from outside…"
code=""
for _ in 1 2 3 4 5 6; do
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "https://$name/api/device" || true)"
  [[ "$code" == "401" ]] && break
  sleep 5
done
if [[ "$code" == "401" ]]; then
  say "Catbot is at https://$name"
  echo "Pair your phone: open Catbot, click the phone button at the top of the sidebar, then Pair a phone."
else
  echo "Funnel is set up, but https://$name didn't answer as Catbot yet (got: ${code:-nothing})." >&2
  echo "A new Funnel address can take a minute or two to appear. Try again shortly:" >&2
  echo "    curl -i https://$name/api/device      (expect 401: Catbot's door, locked)" >&2
fi
