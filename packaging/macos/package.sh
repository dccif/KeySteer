#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/../.." && pwd)"

target="${1:-}"
if [[ -z "$target" ]]; then
  case "$(uname -m)" in
    arm64) target="aarch64-apple-darwin" ;;
    x86_64) target="x86_64-apple-darwin" ;;
    *) echo "unsupported macOS architecture: $(uname -m)" >&2; exit 2 ;;
  esac
fi

case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin) ;;
  *) echo "unsupported macOS target: $target" >&2; exit 2 ;;
esac

for tool in cargo codesign ditto iconutil plutil sips shasum; do
  command -v "$tool" >/dev/null || {
    echo "required packaging tool is missing: $tool" >&2
    exit 1
  }
done

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"
(
  cd "$project_root"
  cargo build --locked --release --target "$target"
)

binary="$project_root/target/$target/release/keysteer"
icon_master="$project_root/assets/icons/keysteer-icon.png"
template="$project_root/packaging/macos/Info.plist.in"
dist="$project_root/dist/$target"
app="$dist/KeySteer.app"
payload="$dist/KeySteer"
iconset="$dist/KeySteer.iconset"
default_config="$project_root/keysteer.default.toml"

test -f "$binary"
test -f "$icon_master"
test -f "$template"
test -f "$default_config"
version="$(awk -F '"' '/^version = / { print $2; exit }' "$project_root/Cargo.toml")"
[[ -n "$version" ]] || { echo "cannot read package version" >&2; exit 1; }
archive="$dist/KeySteer-v$version-$target.zip"
checksum="$archive.sha256"
legacy_archive="$dist/KeySteer-$target.zip"
legacy_checksum="$legacy_archive.sha256"

mkdir -p "$dist"
rm -rf "$app" "$payload" "$iconset"
rm -f "$archive" "$checksum" "$legacy_archive" "$legacy_checksum"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$iconset"
trap 'rm -rf "$iconset"' EXIT

cp "$binary" "$app/Contents/MacOS/KeySteer"
chmod 755 "$app/Contents/MacOS/KeySteer"
sed "s/@VERSION@/$version/g; s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/g" \
  "$template" > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

# iconutil expects the canonical iconset names. The checked-in 256 px master
# stays small; the larger entries are generated only inside the release bundle.
while read -r pixels filename; do
  sips -z "$pixels" "$pixels" "$icon_master" --out "$iconset/$filename" >/dev/null
done <<'ICON_SIZES'
16 icon_16x16.png
32 icon_16x16@2x.png
32 icon_32x32.png
64 icon_32x32@2x.png
128 icon_128x128.png
256 icon_128x128@2x.png
256 icon_256x256.png
512 icon_256x256@2x.png
512 icon_512x512.png
1024 icon_512x512@2x.png
ICON_SIZES
iconutil -c icns "$iconset" -o "$app/Contents/Resources/KeySteer.icns"

plutil -lint "$app/Contents/Info.plist"

# An ad-hoc signature is enough for local double-click builds. Release builds
# should provide a stable Developer ID identity so TCC permissions survive
# upgrades and Gatekeeper can verify the publisher.
identity="${KEYSTEER_CODESIGN_IDENTITY:--}"
sign_args=(--force --sign "$identity")
if [[ "$identity" != "-" ]]; then
  sign_args+=(--options runtime --timestamp)
fi
codesign "${sign_args[@]}" "$app"
codesign --verify --deep --strict --verbose=2 "$app"

# Keep the editable shipped profile beside the app in the release archive.
# macOS application data itself remains in Application Support at runtime.
mkdir -p "$payload"
mv "$app" "$payload/KeySteer.app"
cp "$default_config" "$payload/keysteer.default.toml"
app="$payload/KeySteer.app"

make_archive() {
  rm -f "$archive" "$checksum"
  ditto -c -k --sequesterRsrc --keepParent "$payload" "$archive"
  (
    cd "$dist"
    shasum -a 256 "$(basename "$archive")" > "$(basename "$checksum")"
  )
}

make_archive

# Optional notarization. Either a keychain profile or all three Apple account
# variables may be supplied by a developer machine or GitHub Actions secrets.
if [[ -n "${KEYSTEER_NOTARY_PROFILE:-}" ]]; then
  [[ "$identity" != "-" ]] || {
    echo "notarization requires KEYSTEER_CODESIGN_IDENTITY" >&2
    exit 1
  }
  xcrun notarytool submit "$archive" \
    --keychain-profile "$KEYSTEER_NOTARY_PROFILE" --wait
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
  make_archive
elif [[ -n "${APPLE_ID:-}" || -n "${APPLE_TEAM_ID:-}" || -n "${APPLE_APP_PASSWORD:-}" ]]; then
  [[ "$identity" != "-" ]] || {
    echo "notarization requires KEYSTEER_CODESIGN_IDENTITY" >&2
    exit 1
  }
  [[ -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" ]] || {
    echo "APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD must be set together" >&2
    exit 1
  }
  xcrun notarytool submit "$archive" --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
  make_archive
fi

echo "$archive"
echo "$checksum"
