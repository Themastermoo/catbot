#!/usr/bin/env bash
# Installs Catbot's desktop app for you: builds it, and adds "Catbot" with its icon to the
# KDE menu (and any other desktop that follows the freedesktop.org standards).
#
#   bash scripts/install-app.sh            build the app and add it to the menu
#   bash scripts/install-app.sh --remove   take it out of the menu again
#
# The app is a window onto Catbot, so Catbot itself has to be running: as the background
# service (scripts/install-service.sh) or by hand (cargo run -- serve). Closing the window
# leaves your bots running.
#
# The menu entry runs the app from this repo's build folder, so a new build is what opens
# next time. Once the app is installed, scripts/build-ui.sh rebuilds it along with the
# interface, so the Claude helper keeps it current.
#
# Needs the desktop libraries, once:  sudo apt install libwebkit2gtk-4.1-dev libxdo-dev
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$(pwd)"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
# Named after the program: KDE matches a window to its menu entry (and so to its icon) by the
# program's name, catbot-ui; the window's class is Catbot-ui (StartupWMClass below).
ENTRY="$DATA/applications/catbot-ui.desktop"
ICONS="$DATA/icons/hicolor"
ICON_SRC="crates/catbot-ui/assets/icon"
APP="$REPO/target/dx/catbot-ui/release/linux/app/catbot-ui"
SIZES=(16 32 48 64 128 256 512)

say() { printf '\033[1;36m%s\033[0m\n' "$*"; }

# Tell the desktop the menu or the icons changed. Each of these is optional.
refresh() {
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DATA/applications" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && [[ -f "$ICONS/index.theme" ]] && gtk-update-icon-cache -q "$ICONS" 2>/dev/null || true
  for tool in kbuildsycoca6 kbuildsycoca5; do
    if command -v "$tool" >/dev/null 2>&1; then "$tool" --noincremental >/dev/null 2>&1 || true; break; fi
  done
}

if [[ "${1:-}" == "--remove" ]]; then
  rm -f "$ENTRY"
  for size in "${SIZES[@]}"; do rm -f "$ICONS/${size}x${size}/apps/catbot.png"; done
  rm -f "$ICONS/scalable/apps/catbot.svg"
  refresh
  say "Catbot's desktop app is out of the menu. (The build in target/ is left as it is.)"
  exit 0
fi

# The desktop libraries: WebKitGTK draws the window, libxdo is used for keyboard shortcuts.
missing=()
pkg-config --exists webkit2gtk-4.1 2>/dev/null || missing+=(libwebkit2gtk-4.1-dev)
[[ -f /usr/include/xdo.h ]] || missing+=(libxdo-dev)
if (( ${#missing[@]} )); then
  echo "The desktop app needs some libraries first. Install them with:" >&2
  echo "    sudo apt install ${missing[*]}" >&2
  echo "then run this again." >&2
  exit 1
fi
if ! command -v dx >/dev/null 2>&1; then
  echo "dx (the Dioxus tool) isn't installed. Run scripts/setup.sh, or see README.md." >&2
  exit 1
fi

say "Building Catbot's desktop app…"
dx build -p catbot-ui --platform desktop --release

say "Adding it to the menu…"
for size in "${SIZES[@]}"; do
  install -D -m 644 "$ICON_SRC/catbot-$size.png" "$ICONS/${size}x${size}/apps/catbot.png"
done
install -D -m 644 "$ICON_SRC/catbot.svg" "$ICONS/scalable/apps/catbot.svg"

mkdir -p "$(dirname "$ENTRY")"
# Spaces in the path are quoted for the Exec line; % is doubled (the entry format's escape).
exec_path="${APP//%/%%}"
cat >"$ENTRY" <<EOF
[Desktop Entry]
Type=Application
Name=Catbot
GenericName=Bot chat
Comment=Chat with your Catbot bots
Exec="$exec_path"
Icon=catbot
Terminal=false
Categories=Network;Chat;Utility;
Keywords=bot;chat;claude;catbot;
StartupNotify=true
StartupWMClass=Catbot-ui
X-Catbot-Installed-By=$REPO/scripts/install-app.sh
EOF
refresh

say "Catbot is in the menu, with its icon. Open it from the menu or search for \"Catbot\"."
echo "To pin it: open it, then right-click its icon in the taskbar and choose Pin to Task Manager."
if ! (exec 3<>/dev/tcp/127.0.0.1/8788) 2>/dev/null; then
  echo "Catbot itself isn't running yet, so the window will wait for it. Start it with:"
  echo "    bash scripts/install-service.sh      (in the background, from login)"
fi
