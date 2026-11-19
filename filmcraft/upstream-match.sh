#!/usr/bin/env bash
# upstream-match.sh <fork-clone> <tag-or-commit>...
# For each vendored filmcraft-<x>/ here: how many lines its src/ differs from crates/<x>/src at each ref of the public
# FilmCraft repository, both sides formatted alike (rustfmt, edition 2024, width 100). 0 = equal; "absent" = no such
# crate at that ref. Reads the vendored trees as they are (after PATCHES.md's two patches, so filmcraft-codecs,
# filmcraft-ac3 and filmcraft-media show them); for the unmodified sources unpack tarballs/ first and pass that
# folder as VENDOR (its <name>-0.9.0/ folders are read as they unpack).
set -u
fork=${1:?usage: upstream-match.sh <fork-clone> <ref>...}; shift
[ $# -gt 0 ] || { echo "usage: upstream-match.sh <fork-clone> <ref>..." >&2; exit 2; }
vend=${VENDOR:-$(cd "$(dirname "$0")" && pwd)}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fmt() { find "$1" -name '*.rs' | while read -r f; do rustfmt --edition 2024 --config max_width=100 < "$f" > "$f.n" 2>/dev/null && mv "$f.n" "$f"; done; }
for d in "$vend"/filmcraft-*/; do
  crate=$(basename "$d"); short=${crate#filmcraft-}; short=${short%-0.9.0}
  rm -rf "$work/tar"; mkdir -p "$work/tar"; cp -r "$d/src" "$work/tar/"; fmt "$work/tar"
  line="$crate:"
  for ref in "$@"; do
    rm -rf "$work/f"; mkdir -p "$work/f"
    if git -C "$fork" archive "$ref" "crates/$short/src" 2>/dev/null | tar -x -C "$work/f" 2>/dev/null \
      && [ -d "$work/f/crates/$short/src" ]; then
      fmt "$work/f"
      line="$line $ref=$(diff -r "$work/tar/src" "$work/f/crates/$short/src" | grep -c '^[<>]')"
    else
      line="$line $ref=absent"
    fi
  done
  echo "$line"
done
