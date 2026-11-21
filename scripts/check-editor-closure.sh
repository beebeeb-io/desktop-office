#!/usr/bin/env bash
# Fails when a crate that can open a connection is in what the editor links.
#   1. WordCraft's engine without its `font-store` feature (the patch in patches/), in WordCraft's workspace.
#   2. beebeeb-office with the editor on, both feature sets: `edit` (the default) and `edit-slides`.
# Run after every upstream bump: re-apply the patch (git apply --directory=wordcraft patches/*.patch), then this.
# EDITOR_FLAGS= (empty) checks WordCraft's default features instead: that run must FAIL (ureq, rustls);
# it proves the check can fail.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# The banned names come from deny.toml, the single source of truth for the network ban list.
NET=$(grep -oP 'crate\s*=\s*"\K[^"]+' "$ROOT/deny.toml" | sort -u)
[ -n "$NET" ] || { echo "FAIL: no banned crates in deny.toml" >&2; exit 1; }
TARGETS="x86_64-pc-windows-msvc x86_64-apple-darwin aarch64-apple-darwin x86_64-unknown-linux-gnu"
bad=0

# check <label> <min packages> <cargo tree args...>
check() {
  local label=$1 min=$2 target=$3; shift 3
  local names n c
  names=$(cargo tree -e normal --prefix none --target "$target" -f '{p}' "$@" | awk '{print $1}' | sort -u)
  n=$(echo "$names" | wc -l)
  if [ "$n" -lt "$min" ]; then echo "FAIL [$target]: $label: only $n packages, instrument is broken"; bad=1; return; fi
  for c in $NET; do
    if grep -qx -- "$c" <<<"$names"; then echo "FAIL [$target]: $c is in the editor's closure ($label)"; bad=1; fi
  done
  echo "[$target] $label: $n packages checked"
}

FLAGS=${EDITOR_FLAGS---no-default-features}
cd "$ROOT/wordcraft"
for t in $TARGETS; do
  # shellcheck disable=SC2086
  check "wordcraft-engine $FLAGS" 100 "$t" -p wordcraft-engine $FLAGS
done
cd "$ROOT"
# Only the part above carries the EDITOR_FLAGS override; the next part is skipped for the must-fail run.
if [ -z "${EDITOR_FLAGS+x}" ]; then
  M="$ROOT/beebeeb-office/Cargo.toml"
  for t in $TARGETS; do
    check "beebeeb-office (edit)" 100 "$t" --manifest-path "$M" --locked -p beebeeb-office
    check "beebeeb-office (edit-slides)" 100 "$t" --manifest-path "$M" --locked -p beebeeb-office --features edit-slides
  done
fi

if [ $bad -ne 0 ]; then exit 1; fi
echo "editor closure: no network crate"
