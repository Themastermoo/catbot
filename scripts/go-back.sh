#!/usr/bin/env bash
# When a new build of Catbot won't start on this PC (stage 32.8): starts the last build that
# worked here instead, and asks whether to put back the chats as they were before the new build
# first opened them (today's are set aside, not deleted).
#
#   bash scripts/go-back.sh
#
# Catbot keeps a build once it has run for 20 seconds (in its data folder, updates/last-good/),
# and copies the chats before each new build opens them (updates/snapshots/). Nothing goes back
# by itself (Moo, 2026-11-27: "Keep asking first"): when the service gives up on a new build,
# the desktop says to run this. The Claude helper's next build replaces it again.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$(pwd)"
NAME=catbot
PORT=8788
DATA="${CATBOT_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/catbot}"
GOOD="$DATA/updates/last-good/catbot"
ABOUT="$DATA/updates/last-good/about.json"

say() { printf '\033[1;36m%s\033[0m\n' "$*"; }

if [[ ! -x "$GOOD" ]]; then
  echo "No build that worked has been kept yet: Catbot keeps one once a build has run for 20 seconds." >&2
  exit 1
fi
version=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1])).get("version", "?"))' "$ABOUT" 2>/dev/null || echo "?")
say "Going back to the last Catbot that worked here (version $version)…"
systemctl --user stop "$NAME" 2>/dev/null || true
mkdir -p "$REPO/target/debug"
cp "$GOOD" "$REPO/target/debug/catbot.going-back"
mv -f "$REPO/target/debug/catbot.going-back" "$REPO/target/debug/catbot"

# The copy of the chats taken just before the new build first opened them.
found=$(python3 - "$DATA/updates/snapshots" "$ABOUT" <<'PY'
import json, os, sys
folder, about = sys.argv[1], sys.argv[2]
try:
    good_at = json.load(open(about)).get("at", "")
except Exception:
    good_at = ""
best = None
for name in (os.listdir(folder) if os.path.isdir(folder) else []):
    if not name.endswith(".json"):
        continue
    try:
        meta = json.load(open(os.path.join(folder, name)))
    except Exception:
        continue
    if meta.get("kind") == "build" and meta.get("at", "") >= good_at and os.path.isfile(meta.get("file", "")):
        if best is None or meta["at"] > best["at"]:
            best = meta
if best:
    print(best["file"])
    print(best["at"])
PY
)
if [[ -n "$found" ]]; then
  file=$(sed -n 1p <<<"$found")
  at=$(sed -n 2p <<<"$found")
  echo "The new build may have changed how your chats are kept, and the build that worked may not read them."
  read -r -p "Put back your chats as they were before the new build ($at)? Today's are set aside, not deleted. [y/N] " answer
  if [[ "$answer" == [yY]* ]]; then
    mkdir -p "$DATA/updates"
    python3 -c 'import json, sys; json.dump({"file": sys.argv[1], "at": sys.argv[2]}, open(sys.argv[3], "w"))' "$file" "$at" "$DATA/updates/restore.json"
    echo "They'll be put back as Catbot starts."
  fi
fi

systemctl --user reset-failed "$NAME" 2>/dev/null || true
systemctl --user start "$NAME"
for _ in $(seq 1 60); do
  (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && { say "Catbot is running again (version $version): http://127.0.0.1:$PORT"; exit 0; }
  sleep 0.5
done
echo "It didn't come back either. What it said:" >&2
journalctl --user -u "$NAME" -n 20 --no-pager >&2 || true
exit 1
