#!/usr/bin/env bash
# Builds Catbot for Linux as something anyone can download and open (stage 29.4):
#
#   dist/catbot_<version>_amd64.deb     Ubuntu, Kubuntu, Mint, Debian, Pop!_OS…: double-click it
#                                       (Discover or Software installs it, with what it needs)
#   dist/Catbot-<version>-x86_64.AppImage  any other distro: make it executable and open it
#                                       (with --appimage, and appimagetool on the PATH)
#
# Both hold the same app: the window (catbot-ui) with its own Catbot beside it (catbot-server)
# and the interface (ui/). Opening it starts that Catbot; closing the window keeps it in the
# tray; Quit stops it. No terminal, no helper.
#
# Laid out as /usr/lib/catbot/{catbot-ui, assets/, catbot-server, ui/, version.json}, with
# /usr/bin/catbot, the menu entry and the icons. The .deb asks apt for WebKitGTK (the window's
# web view), the tray library and libxdo. The AppImage uses the computer's own WebKitGTK and
# says how to install it when it's missing.
#
#   bash scripts/build-linux.sh              the .deb
#   bash scripts/build-linux.sh --appimage   the .deb and the AppImage
#
# Needs: the Rust toolchain, dx (scripts/setup.sh), dpkg-deb, and the desktop libraries
# (sudo apt install libwebkit2gtk-4.1-dev libxdo-dev; the tray library is found when it runs).
set -euo pipefail
cd "$(dirname "$0")/.."

say() { printf '\033[1;36m%s\033[0m\n' "$*"; }
stop() { printf '\033[1;31m%s\033[0m\n' "$*" >&2; exit 1; }

APPIMAGE=false
[[ "${1:-}" == "--appimage" ]] && APPIMAGE=true

command -v dx >/dev/null || stop "dx isn't installed (scripts/setup.sh)."
command -v dpkg-deb >/dev/null || stop "dpkg-deb isn't installed."
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
STORE="$(sed -n 's/^pub const STORE_VERSION: i64 = \([0-9]*\);/\1/p' crates/catbot/src/store.rs)"
ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"

say "Building the interface…"
dx build -p catbot-ui --platform web --release
say "Building the app…"
dx build -p catbot-ui --platform desktop --release
say "Building Catbot itself…"
cargo build --release -p catbot

APP_BUILD="target/dx/catbot-ui/release/linux/app"
UI_BUILD="target/dx/catbot-ui/release/web/public"
[[ -x "$APP_BUILD/catbot-ui" ]] || stop "No app in $APP_BUILD."
[[ -f "$UI_BUILD/index.html" ]] || stop "No interface in $UI_BUILD."

# ---- The tree both packages share ----
STAGE="dist/linux/catbot"
rm -rf dist/linux
LIB="$STAGE/usr/lib/catbot"
mkdir -p "$LIB" "$STAGE/usr/bin" "$STAGE/usr/share/applications"
cp -r "$APP_BUILD"/. "$LIB/"
install -m 755 target/release/catbot "$LIB/catbot-server"
cp -r "$UI_BUILD" "$LIB/ui"
printf '{ "version": "%s", "store": %s }\n' "$VERSION" "$STORE" > "$LIB/version.json"
ln -s ../lib/catbot/catbot-ui "$STAGE/usr/bin/catbot"
for size in 16 32 48 64 128 256 512; do
  install -D -m 644 "crates/catbot-ui/assets/icon/catbot-$size.png" "$STAGE/usr/share/icons/hicolor/${size}x${size}/apps/catbot.png"
done
install -D -m 644 crates/catbot-ui/assets/icon/catbot.svg "$STAGE/usr/share/icons/hicolor/scalable/apps/catbot.svg"
# The window's class is Catbot-ui (from the program's name), which ties it to this entry and so
# to its icon.
cat > "$STAGE/usr/share/applications/catbot.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Catbot
GenericName=AI bot team
Comment=Chat with a team of AI bots
Exec=catbot
Icon=catbot
Terminal=false
Categories=Network;Chat;Utility;
Keywords=bot;chat;ai;claude;catbot;
StartupNotify=true
StartupWMClass=Catbot-ui
EOF

# ---- The .deb ----
say "Packing the .deb…"
DEB_ROOT="dist/linux/deb"
cp -a "$STAGE" "$DEB_ROOT"
mkdir -p "$DEB_ROOT/DEBIAN"
SIZE_KB="$(du -sk "$DEB_ROOT" | cut -f1)"
cat > "$DEB_ROOT/DEBIAN/control" <<EOF
Package: catbot
Version: $VERSION
Section: net
Priority: optional
Architecture: $ARCH
Installed-Size: $SIZE_KB
Depends: libwebkit2gtk-4.1-0, libgtk-3-0, libayatana-appindicator3-1 | libappindicator3-1, libxdo3
Maintainer: Catbot <catbot@users.noreply.github.com>
Homepage: https://github.com/
Description: A team of AI bots you chat with
 Catbot runs AI bots on your computer: Claude through Claude Code, chat APIs
 with your own keys, or models on this computer. Open it from the menu; it
 keeps working in the tray when the window is closed.
EOF
# Refresh the menu and icons after installing or removing (each step optional).
for script in postinst postrm; do
  cat > "$DEB_ROOT/DEBIAN/$script" <<'EOF'
#!/bin/sh
set -e
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true
exit 0
EOF
  chmod 755 "$DEB_ROOT/DEBIAN/$script"
done
mkdir -p dist
DEB="dist/catbot_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group -Zxz --build "$DEB_ROOT" "$DEB" >/dev/null
say "Done: $DEB ($(du -h "$DEB" | cut -f1))"

# ---- The AppImage ----
if $APPIMAGE; then
  command -v appimagetool >/dev/null || stop "appimagetool isn't on the PATH (https://github.com/AppImage/appimagetool)."
  say "Packing the AppImage…"
  APPDIR="dist/linux/Catbot.AppDir"
  cp -a "$STAGE" "$APPDIR"
  cp "$STAGE/usr/share/applications/catbot.desktop" "$APPDIR/catbot.desktop"
  cp crates/catbot-ui/assets/icon/catbot-256.png "$APPDIR/catbot.png"
  install -m 755 scripts/linux/AppRun "$APPDIR/AppRun"
  IMAGE="dist/Catbot-${VERSION}-x86_64.AppImage"
  ARCH=x86_64 appimagetool --no-appstream "$APPDIR" "$IMAGE"
  say "Done: $IMAGE ($(du -h "$IMAGE" | cut -f1))"
fi
