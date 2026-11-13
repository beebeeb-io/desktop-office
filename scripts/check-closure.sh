#!/usr/bin/env bash
# Fails when a crate that can open a network connection is in the beebeeb-office dependency closure.
# Reads the banned names from deny.toml (the single source of truth for the network ban list).
# Run from the repository root.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DENY_TOML="$REPO_ROOT/deny.toml"
MANIFEST="$REPO_ROOT/beebeeb-office/Cargo.toml"

if [ ! -f "$DENY_TOML" ]; then
  echo "FAIL: deny.toml not found at $DENY_TOML" >&2
  exit 1
fi
if [ ! -f "$MANIFEST" ]; then
  echo "FAIL: beebeeb-office/Cargo.toml not found at $MANIFEST" >&2
  exit 1
fi

# Extract the banned crate names from deny.toml: lines like { crate = "reqwest" }
NET=$(grep -oP 'crate\s*=\s*"\K[^"]+' "$DENY_TOML" | sort -u)
if [ -z "$NET" ]; then
  echo "FAIL: no banned crates found in $DENY_TOML" >&2
  exit 1
fi

bad=0
total_packages=0
for target in x86_64-pc-windows-msvc x86_64-apple-darwin aarch64-apple-darwin x86_64-unknown-linux-gnu; do
  names=$(cargo tree \
    --manifest-path "$MANIFEST" \
    -e normal \
    --prefix none \
    --target "$target" \
    --locked \
    -p beebeeb-office \
    -f '{p}' | awk '{print $1}' | sort -u)
  n=$(echo "$names" | wc -l)
  if [ "$n" -lt 10 ]; then
    echo "FAIL [$target]: only $n packages — instrument is broken" >&2
    bad=1
    continue
  fi
  total_packages=$n
  for c in $NET; do
    if echo "$names" | grep -qx "$c"; then
      echo "FAIL [$target]: $c is in the closure"
      bad=1
    fi
  done
  echo "[$target] beebeeb-office: $n packages checked"
done

if [ $bad -ne 0 ]; then
  exit 1
fi
echo "closure: $total_packages packages, 0 network crates"
