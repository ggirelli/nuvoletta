#!/usr/bin/env bash
# Build the release Android APK and sign it.
#
# Usage: scripts/release/android-apk.sh <output-dir>
#
# Needs: dx, the Android SDK (ANDROID_HOME, build-tools with apksigner and zipalign) and NDK
# (ANDROID_NDK_HOME), Java 17+, and the aarch64-linux-android Rust target.
#
# Signing:
#   ANDROID_KEYSTORE_BASE64, ANDROID_KEYSTORE_PASSWORD, ANDROID_KEY_ALIAS and ANDROID_KEY_PASSWORD
#   (all four set): sign with that release keystore; the APK is named nuvoletta-<version>-android.apk.
#   Otherwise: sign with a throwaway key generated for this build and discarded. The APK installs
#   but cannot be upgraded by a later release; it is named nuvoletta-<version>-android-unsigned.apk
#   so nobody mistakes it for an official signed build. The well-known Android debug key is never
#   used: anyone could forge an update signed with it.
#
# Why this script runs Gradle itself: `dx bundle` 0.7 builds the debug variant of the APK and
# hard-codes versionCode = 1, so dx only generates the Gradle project and the native library here.
set -euo pipefail

out_dir=$(realpath -m "${1:?usage: android-apk.sh <output-dir>}")
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
# shellcheck source=scripts/release/env.sh
. scripts/release/env.sh

: "${ANDROID_HOME:?set ANDROID_HOME}"
version=$(scripts/release/version.sh)
code=$(scripts/release/version.sh --android-code)
build_tools=$(ls -d "$ANDROID_HOME"/build-tools/*/ | sort -V | tail -n1)
target_dir="${CARGO_TARGET_DIR:-target}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# 1. Native library and Gradle project (dx also runs a debug Gradle build we do not use).
dx bundle --platform android --release --locked --target aarch64-linux-android \
  --no-default-features --features mobile --package nuvoletta-app \
  --package-types apk --out-dir "$tmp/dx-out"

# 2. Release variant with the real versionCode, minified (see the dx-generated build.gradle.kts).
project="$target_dir/dx/nuvoletta-app/release/android/app"
sed -i "s/versionCode = [0-9]*/versionCode = $code/" "$project/app/build.gradle.kts"
# Lint is a dev check, not part of the artifact, and its analyzer fails on very new JDKs.
(cd "$project" && ./gradlew --no-daemon --console=plain :app:assembleRelease \
  -x lintVitalAnalyzeRelease -x lintVitalReportRelease -x lintVitalRelease)
unsigned="$project/app/build/outputs/apk/release/app-release-unsigned.apk"

# 3. Check what we are about to ship: only INTERNET (ADR 0006), minSdk 28, the expected version.
aapt2="$build_tools/aapt2"
perms=$("$aapt2" dump permissions "$unsigned" | sed -n "s/^uses-permission: name='\(.*\)'.*/\1/p" \
  | grep -v '\.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION$' | sort)
[ "$perms" = "android.permission.INTERNET" ] || { echo "unexpected permissions: $perms" >&2; exit 1; }
badging=$("$aapt2" dump badging "$unsigned")
grep -q "^minSdkVersion:'28'" <<< "$badging"
grep -q "versionCode='$code' versionName='$version'" <<< "$badging" \
  || { echo "APK version does not match $version ($code)" >&2; exit 1; }
"$build_tools/zipalign" -c 4 "$unsigned"

# 4. Sign.
if [ -n "${ANDROID_KEYSTORE_BASE64:-}" ] && [ -n "${ANDROID_KEYSTORE_PASSWORD:-}" ] \
   && [ -n "${ANDROID_KEY_ALIAS:-}" ] && [ -n "${ANDROID_KEY_PASSWORD:-}" ]; then
  echo "signing with the release keystore" >&2
  base64 -d <<< "$ANDROID_KEYSTORE_BASE64" > "$tmp/release.jks"
  ks="$tmp/release.jks" alias="$ANDROID_KEY_ALIAS"
  export KS_PASS="$ANDROID_KEYSTORE_PASSWORD" KEY_PASS="$ANDROID_KEY_PASSWORD"
  name="nuvoletta-${version}-android.apk"
else
  echo "no release keystore: signing with a throwaway key (APK is labelled unsigned)" >&2
  ks="$tmp/throwaway.jks" alias=throwaway
  KS_PASS="$(head -c 18 /dev/urandom | base64)"
  export KS_PASS KEY_PASS="$KS_PASS"
  keytool -genkeypair -keystore "$ks" -storepass:env KS_PASS -keypass:env KEY_PASS -alias "$alias" \
    -keyalg RSA -keysize 3072 -validity 36500 -dname "CN=Nuvoletta throwaway build" >/dev/null
  name="nuvoletta-${version}-android-unsigned.apk"
fi
mkdir -p "$out_dir"
"$build_tools/apksigner" sign --ks "$ks" --ks-key-alias "$alias" \
  --ks-pass env:KS_PASS --key-pass env:KEY_PASS --v4-signing-enabled false --out "$out_dir/$name" "$unsigned"
"$build_tools/apksigner" verify --print-certs "$out_dir/$name" | head -n3 >&2
echo "$out_dir/$name"
