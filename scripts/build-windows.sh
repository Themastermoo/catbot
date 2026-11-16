#!/usr/bin/env bash
# Builds Catbot for Windows on Linux: dist/catbot-windows.zip, ready to copy to a Windows PC.
#
#   bash scripts/build-windows.sh
#
# Needs: Rust's Windows target (rustup target add x86_64-pc-windows-gnu), MinGW
# (sudo apt install gcc-mingw-w64-x86-64), and `dx`. Inside the zip:
#   Install.cmd, install.ps1       double-click Install.cmd on the PC
#   app/Catbot.exe                 the app (starts this PC's Catbot, switches to home's)
#   app/catbot-server.exe          this PC's Catbot
#   app/WebView2Loader.dll         Microsoft's loader for the window's web view
#   app/assets/, app/ui/           the app's files, and the interface the server serves
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$(pwd)"
TARGET=x86_64-pc-windows-gnu
OUT="$REPO/dist/catbot-windows"

say() { printf '\033[1;36m%s\033[0m\n' "$*"; }
stop() { printf '\033[1;31m%s\033[0m\n' "$*" >&2; exit 1; }

command -v x86_64-w64-mingw32-gcc >/dev/null || stop "MinGW isn't installed: sudo apt install gcc-mingw-w64-x86-64"
rustup target list --installed | grep -qx "$TARGET" || stop "Rust's Windows target is missing: rustup target add $TARGET"
command -v dx >/dev/null || stop "dx (the Dioxus tool) isn't installed; see README.md."

# dx passes MSVC-style linker flags for Windows apps; this wrapper turns them into MinGW's.
chmod +x scripts/windows/mingw-link.sh
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$REPO/scripts/windows/mingw-link.sh"

say "Building this PC's Catbot (catbot-server.exe)…"
cargo build -p catbot --release --target "$TARGET"
say "Building the interface it serves…"
dx build -p catbot-ui --platform web --release
say "Building the app (Catbot.exe)…"
dx build -p catbot-ui --platform windows --release --target "$TARGET"

say "Putting the package together…"
rm -rf "$OUT" dist/catbot-windows.zip
mkdir -p "$OUT/app"
APP="$REPO/target/dx/catbot-ui/release/windows/app"
cp "$APP/catbot-ui.exe" "$OUT/app/Catbot.exe"
cp -r "$APP/assets" "$OUT/app/assets"
cp "target/$TARGET/release/catbot.exe" "$OUT/app/catbot-server.exe"
cp -r target/dx/catbot-ui/release/web/public "$OUT/app/ui"
LOADER="$(find "target/$TARGET" -path '*webview2-com-sys*/out/x64/WebView2Loader.dll' | head -1)"
[[ -n "$LOADER" ]] || stop "WebView2Loader.dll wasn't found in the build."
cp "$LOADER" "$OUT/app/"
cp crates/catbot-ui/assets/icon/catbot.ico "$OUT/app/catbot.ico"
cp scripts/windows/install.ps1 scripts/windows/Install.cmd "$OUT/"
# Windows line endings, so cmd reads Install.cmd properly.
sed -i 's/\r*$/\r/' "$OUT/Install.cmd" "$OUT/install.ps1"
cp NOTICE "$OUT/app/NOTICE.txt"
(cd dist && rm -f catbot-windows.zip && zip -qr catbot-windows.zip catbot-windows)
say "Done: dist/catbot-windows.zip ($(du -h dist/catbot-windows.zip | cut -f1))"
