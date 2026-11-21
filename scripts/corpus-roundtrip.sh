#!/usr/bin/env bash
# The open-and-save of every Microsoft Office file in corpus/SOURCES.tsv. Part names and counts only.
#
#   corpus-roundtrip.sh [OUT]         the UNEDITED save through the engines directly (report.txt, then
#                                     pkgdiff.py and celldiff.py); OUT defaults to /tmp/corpus-w02
#   corpus-roundtrip.sh edit [OUT]    the same corpus (all producers) through OUR edit API: open, three scripted
#                                     edits, save, reopen (tests/corpus_edit.rs, with --features edit-slides so
#                                     pptx is covered); OUT defaults to /tmp/corpus-w04. Writes report-edit.txt
#                                     (+ saved files), then pkgdiff.py over the pairs.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ "${1:-}" = edit ]; then
  export CORPUS_OUT="${2:-${CORPUS_OUT:-/tmp/corpus-w04}}"
  rm -rf "$CORPUS_OUT"; mkdir -p "$CORPUS_OUT"
  (cd "$ROOT/beebeeb-office" && cargo test --locked --features edit-slides -- --ignored corpus_edit --nocapture) | tee "$CORPUS_OUT/cargo-edit.txt" | grep -E '^(FILE|SUMMARY)' >/dev/null
  python3 -I "$ROOT/scripts/pkgdiff.py" --dir "$CORPUS_OUT" > "$CORPUS_OUT/pkgdiff-edit.txt"
  echo "report: $CORPUS_OUT/report-edit.txt  $CORPUS_OUT/pkgdiff-edit.txt"
  exit 0
fi
export CORPUS_OUT="${1:-${CORPUS_OUT:-/tmp/corpus-w02}}"
rm -rf "$CORPUS_OUT"; mkdir -p "$CORPUS_OUT"
(cd "$ROOT/beebeeb-office" && cargo test --locked -- --ignored corpus_roundtrip --nocapture) | tee "$CORPUS_OUT/cargo.txt" | grep -E '^(FILE|SUMMARY)' >/dev/null
python3 -I "$ROOT/scripts/pkgdiff.py" --dir "$CORPUS_OUT" > "$CORPUS_OUT/pkgdiff.txt"
python3 -I "$ROOT/scripts/celldiff.py" --dir "$CORPUS_OUT" > "$CORPUS_OUT/celldiff.txt"
echo "report: $CORPUS_OUT/report.txt  $CORPUS_OUT/pkgdiff.txt  $CORPUS_OUT/celldiff.txt"
