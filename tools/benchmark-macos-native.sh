#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "benchmark-macos-native.sh must run on macOS 14 or later" >&2
  exit 2
fi

baseline_ref="${1:-709815c}"
rounds="${2:-5}"
if ! [[ "$rounds" =~ ^[1-9][0-9]*$ ]]; then
  echo "rounds must be a positive integer" >&2
  exit 2
fi
repo="$(git rev-parse --show-toplevel)"
python_bin="$(command -v python3 || true)"
if [[ -z "$python_bin" ]]; then
  echo "python3 is required to summarize the native probe logs" >&2
  exit 2
fi
output="$repo/target/macos-native-bench"
temporary="$(mktemp -d "${TMPDIR:-/tmp}/keysteer-macos-bench.XXXXXX")"
baseline="$temporary/baseline"

cleanup() {
  if [[ -d "$baseline" ]]; then
    git -C "$repo" worktree remove --force "$baseline" >/dev/null 2>&1 || true
  fi
  rm -rf "$temporary"
}
trap cleanup EXIT INT TERM

mkdir -p "$output"
rm -f "$output"/baseline-*.log "$output"/optimized-*.log "$output/summary.csv"

git -C "$repo" worktree add --detach "$baseline" "$baseline_ref"
mkdir -p "$baseline/examples"
cp "$repo/examples/macos_native_probe.rs" "$baseline/examples/macos_native_probe.rs"

echo "Build and grant Accessibility + Screen Recording to the responsible terminal/app when prompted."
cargo build --manifest-path "$repo/Cargo.toml" --release --example macos_native_probe
CARGO_TARGET_DIR="$temporary/baseline-target" \
  cargo build --manifest-path "$baseline/Cargo.toml" --release --example macos_native_probe

optimized_binary="$repo/target/release/examples/macos_native_probe"
baseline_binary="$temporary/baseline-target/release/examples/macos_native_probe"

for ((round = 1; round <= rounds; round++)); do
  if ((round % 2 == 1)); then
    "$baseline_binary" | tee "$output/baseline-$round.log"
    "$optimized_binary" | tee "$output/optimized-$round.log"
  else
    "$optimized_binary" | tee "$output/optimized-$round.log"
    "$baseline_binary" | tee "$output/baseline-$round.log"
  fi
done

"$python_bin" "$repo/tools/summarize-macos-native.py" "$output" "$rounds" \
  | tee "$output/summary.csv"

echo "Raw logs and the aggregate report are in $output"
