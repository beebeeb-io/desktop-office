#!/usr/bin/env bash
# The unedited open-and-save of every Microsoft Office file in corpus/SOURCES.tsv, through the engines
# directly. Writes report.txt (+ saved files) to OUT (default /tmp/corpus-w02), then pkgdiff.py and
# celldiff.py over the pairs. Part names and counts only.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export CORPUS_OUT="${1:-${CORPUS_OUT:-/tmp/corpus-w02}}"
rm -rf "$CORPUS_OUT"; mkdir -p "$CORPUS_OUT"
(cd "$ROOT/beebeeb-office" && cargo test --locked -- --ignored corpus_roundtrip --nocapture) | tee "$CORPUS_OUT/cargo.txt" | grep -E '^(FILE|SUMMARY)' >/dev/null
python3 -I "$ROOT/scripts/pkgdiff.py" --dir "$CORPUS_OUT" > "$CORPUS_OUT/pkgdiff.txt"
python3 -I "$ROOT/scripts/celldiff.py" --dir "$CORPUS_OUT" > "$CORPUS_OUT/celldiff.txt"
echo "report: $CORPUS_OUT/report.txt  $CORPUS_OUT/pkgdiff.txt  $CORPUS_OUT/celldiff.txt"
