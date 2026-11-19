#!/usr/bin/env bash
# patches-check.sh: rebuilds the vendored tree from its sources of record and compares it with what is here. It unpacks
# every tarball in tarballs/ (crates.io's files; verify.sh checks them), applies the patches in PATCHES.md's order, and
# diffs the result against each filmcraft-<x>/ folder. Prints one count line; exits non-zero on any difference.
# Usage: filmcraft/patches-check.sh [patch...]
#   With no argument: the patches in PATCHES.md's order. Naming fewer (or reordered) patches shows the check failing.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
order=(filmcraft-features.patch filmcraft-dng-avc-order.patch filmcraft-lints.patch)
[ $# -gt 0 ] && order=("$@")
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git init -q "$work"   # so that `git apply` applies at the top of $work, never in a repository above it
mkdir -p "$work/filmcraft"
for t in "$here"/tarballs/*.crate; do tar -xzf "$t" -C "$work/filmcraft"; done
for d in "$work"/filmcraft/filmcraft-*-[0-9]*; do mv "$d" "${d%-*}"; done   # <name>-<version>/ -> <name>/
applied="the tarballs"
for p in "${order[@]}"; do
  if ! git -C "$work" apply "$here/patches/$p"; then
    echo "FAIL: patches/$p does not apply on $applied" >&2
    exit 1
  fi
  applied="$applied + $p"
done
same=0
differ=0
for d in "$here"/filmcraft-*/; do
  n=$(basename "$d")
  if diff -r -q "$work/filmcraft/$n" "$d" > /dev/null 2>&1; then
    same=$((same + 1))
  else
    differ=$((differ + 1))
    echo "DIFFERS: $n ($(diff -r "$work/filmcraft/$n" "$d" 2>&1 | grep -c '^[<>]\|^Only in') lines)" >&2
  fi
done
echo "filmcraft: $same of $((same + differ)) crates equal the tarballs + ${#order[@]} patches, $differ differ"
[ "$differ" -eq 0 ] && [ "$same" -eq 26 ]
