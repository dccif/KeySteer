#!/usr/bin/env bash
# Package an already cross-compiled EXE with the same layout as package.ps1.
set -euo pipefail

target="${1:-}"
variant="${2:-avx2}"
if [[ "$target" == "aarch64-pc-windows-msvc" && -z "${2:-}" ]]; then
  variant="compatible"
fi
case "$target" in
  x86_64-pc-windows-msvc|aarch64-pc-windows-msvc) ;;
  *) echo "unsupported Windows target: $target" >&2; exit 2 ;;
esac
case "$target:$variant" in
  x86_64-pc-windows-msvc:avx2|aarch64-pc-windows-msvc:compatible) suffix="" ;;
  x86_64-pc-windows-msvc:compatible) suffix="-compatible" ;;
  x86_64-pc-windows-msvc:avx512) suffix="-avx512" ;;
  *) echo "unsupported Windows CPU variant: $target / $variant" >&2; exit 2 ;;
esac
project_root="$(cd "$(dirname "$0")/../.." && pwd)"
binary="$project_root/target/$target/release/keysteer.exe"
default_config="$project_root/keysteer.default.toml"
test -s "$binary"
test -f "$default_config"
: "${SIGNING_PFX_BASE64:?WINDOWS_SIGNING_PFX_BASE64 is required for Windows packages}"
export KEYSTEER_SIGNING_PASSWORD="${KEYSTEER_SIGNING_PASSWORD:-}"
timestamp_url="${KEYSTEER_TIMESTAMP_URL:-http://timestamp.digicert.com}"
version="$(sed -n 's/^version = "\([^" ]*\)"/\1/p' "$project_root/Cargo.toml" | head -n 1)"
test -n "$version"

# Secrets stay outside both build caches and uploaded artifacts.
umask 077
signing_dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/keysteer-signing.XXXXXX")"
trap 'rm -f "$signing_dir/certificate.pfx" "$signing_dir/password" "$signing_dir/chain.pem" "$signing_dir/leaf.pem"; rmdir "$signing_dir"' EXIT
printf '%s' "$SIGNING_PFX_BASE64" | base64 --decode > "$signing_dir/certificate.pfx"
# A newline also supports an empty password with osslsigncode -readpass.
printf '%s\n' "$KEYSTEER_SIGNING_PASSWORD" > "$signing_dir/password"
openssl pkcs12 -legacy -in "$signing_dir/certificate.pfx" -nokeys \
  -passin env:KEYSTEER_SIGNING_PASSWORD -out "$signing_dir/chain.pem"
openssl pkcs12 -legacy -in "$signing_dir/certificate.pfx" -nokeys -clcerts \
  -passin env:KEYSTEER_SIGNING_PASSWORD -out "$signing_dir/leaf.pem"
signer_hash="$(openssl x509 -in "$signing_dir/leaf.pem" -outform DER | sha256sum | cut -d ' ' -f 1)"
cat /etc/ssl/certs/ca-certificates.crt >> "$signing_dir/chain.pem"

dist="$project_root/dist/$target"
payload="$dist/KeySteer"
# The default AVX2 filename stays stable for existing updaters and website links.
# Optional variants end AFTER the target, so they cannot match that default link.
archive="$dist/KeySteer-v$version-$target$suffix.zip"
mkdir -p "$payload"
# Only these two files are archived, even if the output directory already exists.
rm -f "$payload/KeySteer.exe" "$archive"
osslsigncode sign -pkcs12 "$signing_dir/certificate.pfx" \
  -readpass "$signing_dir/password" -h sha256 -n KeySteer \
  -ts "$timestamp_url" -in "$binary" -out "$payload/KeySteer.exe"
# Trust the supplied chain explicitly, including the project's development
# certificate, and require the exact PFX leaf identity used by the updater.
osslsigncode verify -CAfile "$signing_dir/chain.pem" \
  -TSA-CAfile /etc/ssl/certs/ca-certificates.crt \
  -require-leaf-hash "sha256:$signer_hash" -in "$payload/KeySteer.exe"
cp "$default_config" "$payload/keysteer.default.toml"
chmod 644 "$payload/KeySteer.exe" "$payload/keysteer.default.toml"
(
  cd "$dist"
  zip -X -9 "$archive" KeySteer/KeySteer.exe KeySteer/keysteer.default.toml
)
echo "$archive"
