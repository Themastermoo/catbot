#!/usr/bin/env bash
# Builds Catbot's phone app (Android) as a signed APK in dist/catbot.apk.
#
#   bash scripts/build-android.sh
#
# Needs: the Android SDK and NDK (ANDROID_HOME, ANDROID_NDK_HOME; accepting Google's Android
# SDK licence is part of installing them), Java 17 or newer, `dx`, and Rust's Android target
# (rustup target add aarch64-linux-android).
#
# The app is signed with Catbot's own key, kept outside the repo in
# ${CATBOT_ANDROID_KEY_DIR:-~/.local/share/catbot/android-key} (made on the first build). Keep
# that folder: an update must be signed with the same key, or the phone refuses it until the
# old app is uninstalled (and the phone paired again).
#
# For releases (scripts/release/README.md), these can be set:
#   CATBOT_ANDROID_KEY_REQUIRED=1   stop if the key isn't there, rather than making a new one
#   CATBOT_ANDROID_CODE=<n>         Android's version code; by default the minutes since
#                                   2026-01-01, so every build is newer to the phone than the last
#   CATBOT_ANDROID_OUT=<path>       where the APK goes (default dist/catbot.apk)
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$(pwd)"
KEY_DIR="${CATBOT_ANDROID_KEY_DIR:-$HOME/.local/share/catbot/android-key}"
KEYSTORE="$KEY_DIR/catbot-release.keystore"
PASSFILE="$KEY_DIR/password"
APP="$REPO/target/dx/catbot-ui/release/android/app"
RES="$APP/app/src/main/res"
OUT="${CATBOT_ANDROID_OUT:-dist/catbot.apk}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
# Android refuses an APK whose code isn't higher than the installed one's, so the code counts
# time, not the version: a rebuilt older version (going back) is still "newer" to the phone.
CODE="${CATBOT_ANDROID_CODE:-$(( ($(date -u +%s) - $(date -u -d 2026-01-01 +%s)) / 60 ))}"
# The app knows its own code too, so it offers only an update the phone will accept.
export CATBOT_ANDROID_CODE="$CODE"

say() { printf '\033[1;36m%s\033[0m\n' "$*"; }
stop() { printf '\033[1;31m%s\033[0m\n' "$*" >&2; exit 1; }

[[ -n "${ANDROID_HOME:-}" && -d "$ANDROID_HOME" ]] || stop "ANDROID_HOME isn't set to the Android SDK."
[[ -n "${ANDROID_NDK_HOME:-}" && -d "$ANDROID_NDK_HOME" ]] || stop "ANDROID_NDK_HOME isn't set to the Android NDK."
command -v dx >/dev/null || stop "dx (the Dioxus tool) isn't installed; see README.md."
BUILD_TOOLS="$(ls -d "$ANDROID_HOME"/build-tools/*/ 2>/dev/null | sort -V | tail -1)"
[[ -n "$BUILD_TOOLS" ]] || stop "No Android build-tools in $ANDROID_HOME/build-tools."

say "Building the app's Rust and its Android project…"
dx build -p catbot-ui --platform android --release --target aarch64-linux-android

say "Naming it Catbot and giving it the teal cat icon…"
# dx names the app after the crate (CatbotUi) and uses Dioxus's own adaptive icon.
sed -i 's|<string name="app_name">[^<]*</string>|<string name="app_name">Catbot</string>|' "$RES/values/strings.xml"
mkdir -p "$RES/drawable-nodpi"
cp crates/catbot-ui/assets/icon/catbot-512.png "$RES/drawable-nodpi/catbot_icon.png"
cat >"$RES/drawable/catbot_foreground.xml" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<!-- The visor cat, inside the adaptive icon's safe zone. -->
<inset xmlns:android="http://schemas.android.com/apk/res/android"
    android:drawable="@drawable/catbot_icon" android:inset="22%" />
EOF
cat >"$RES/drawable/catbot_background.xml" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<shape xmlns:android="http://schemas.android.com/apk/res/android"><solid android:color="#FFFFFF" /></shape>
EOF
cat >"$RES/mipmap-anydpi-v26/ic_launcher.xml" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@drawable/catbot_background" />
    <foreground android:drawable="@drawable/catbot_foreground" />
</adaptive-icon>
EOF

say "Version $VERSION, code $CODE…"
sed -i -E "s/versionCode = [0-9]+/versionCode = $CODE/; s/versionName = \"[^\"]*\"/versionName = \"$VERSION\"/" "$APP/app/build.gradle.kts"

say "Letting it hand updates to Android's installer…"
# Updates with rollback (stage 28.5): the app downloads a release's APK into its private files
# and opens Android's installer on it, through a FileProvider sharing just that folder.
MANIFEST="$APP/app/src/main/AndroidManifest.xml"
if ! grep -q REQUEST_INSTALL_PACKAGES "$MANIFEST"; then
  sed -i 's|<uses-permission android:name="android.permission.INTERNET" />|&\n    <uses-permission android:name="android.permission.REQUEST_INSTALL_PACKAGES" />|' "$MANIFEST"
fi
if ! grep -q 'catbot_updates' "$MANIFEST"; then
  sed -i 's|</application>|    <provider android:name="androidx.core.content.FileProvider" android:authorities="${applicationId}.updates"\n            android:exported="false" android:grantUriPermissions="true">\n            <meta-data android:name="android.support.FILE_PROVIDER_PATHS" android:resource="@xml/catbot_updates" />\n        </provider>\n    </application>|' "$MANIFEST"
fi
mkdir -p "$RES/xml"
cat >"$RES/xml/catbot_updates.xml" <<'EOF2'
<?xml version="1.0" encoding="utf-8"?>
<!-- Only the downloaded updates, for Android's installer to read. -->
<paths><files-path name="updates" path="catbot/updates/downloads/" /></paths>
EOF2
grep -q REQUEST_INSTALL_PACKAGES "$MANIFEST" && grep -q catbot_updates "$MANIFEST" || stop "Couldn't add the installer's permission and FileProvider to $MANIFEST."

# No shrinking: the Rust side reaches Android classes by name, which a shrinker can't see.
sed -i 's/isMinifyEnabled = true/isMinifyEnabled = false/' "$APP/app/build.gradle.kts"

say "Packing the release build…"
(cd "$APP" && ./gradlew --quiet assembleRelease)
UNSIGNED="$(ls "$APP"/app/build/outputs/apk/release/*-unsigned.apk 2>/dev/null | head -1)"
[[ -n "$UNSIGNED" ]] || stop "Gradle didn't make a release APK."

if [[ ! -f "$KEYSTORE" && "${CATBOT_ANDROID_KEY_REQUIRED:-}" == 1 ]]; then
  stop "Catbot's signing key isn't in $KEY_DIR. A new key would make phones refuse the update."
fi
if [[ ! -f "$KEYSTORE" ]]; then
  say "Making Catbot's signing key in $KEY_DIR (keep this folder)…"
  mkdir -p "$KEY_DIR"
  chmod 700 "$KEY_DIR"
  head -c 24 /dev/urandom | base64 | tr -d '/+=' >"$PASSFILE"
  chmod 600 "$PASSFILE"
  keytool -genkeypair -keystore "$KEYSTORE" -storepass:file "$PASSFILE" -keypass:file "$PASSFILE" \
    -alias catbot -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=Catbot, O=Moo" >/dev/null
  chmod 600 "$KEYSTORE"
fi

say "Signing it…"
mkdir -p "$(dirname "$OUT")"
"$BUILD_TOOLS/zipalign" -f -p 4 "$UNSIGNED" "$OUT.aligned"
"$BUILD_TOOLS/apksigner" sign --ks "$KEYSTORE" --ks-pass "file:$PASSFILE" --ks-key-alias catbot \
  --out "$OUT" "$OUT.aligned"
rm -f "$OUT.aligned" "$OUT.idsig"
"$BUILD_TOOLS/apksigner" verify "$OUT"
say "Done: $OUT ($(du -h "$OUT" | cut -f1)), version $VERSION, code $CODE"
