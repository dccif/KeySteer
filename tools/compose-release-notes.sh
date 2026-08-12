#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: compose-release-notes.sh VERSION OUTPUT [CHANGELOG] [FOOTER]}"
output="${2:?usage: compose-release-notes.sh VERSION OUTPUT [CHANGELOG] [FOOTER]}"
changelog="${3:-docs/releases/index.md}"
footer="${4:-.github/release-notes.md}"
heading="## $version"

[[ -f "$changelog" ]] || { echo "release changelog is missing: $changelog" >&2; exit 1; }
[[ -f "$footer" ]] || { echo "release footer is missing: $footer" >&2; exit 1; }

matches="$(grep -Fxc "$heading" "$changelog" || true)"
if [[ "$matches" != "1" ]]; then
  echo "expected exactly one '$heading' section in $changelog; found $matches" >&2
  exit 1
fi

section="$(mktemp)"
trap 'rm -f "$section"' EXIT
awk -v heading="$heading" '
  $0 == heading { capture = 1; next }
  capture && /^## / { exit }
  capture { print }
' "$changelog" > "$section"

if ! grep -q '[^[:space:]]' "$section"; then
  echo "release section '$heading' is empty in $changelog" >&2
  exit 1
fi

{
  printf '%s\n\n' "## 更新内容 / What's Changed"
  cat "$section"
  printf '\n'
  cat "$footer"
  printf '\n'
} > "$output"
