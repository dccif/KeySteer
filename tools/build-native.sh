#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
case "$(uname -m)" in
  arm64) target="aarch64-apple-darwin" ;;
  x86_64) target="x86_64-apple-darwin" ;;
  *) echo "unsupported host architecture" >&2; exit 2 ;;
esac
export CARGO_TARGET_DIR="$root/target-native/$target"
export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=native"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct)}"
cargo build --manifest-path "$root/Cargo.toml" --locked --release --target "$target"
printf '%s\n' "$CARGO_TARGET_DIR/$target/release/keysteer"
