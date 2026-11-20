#!/usr/bin/env bash
# Checks the vendored FilmCraft against what crates.io published: every tarball in tarballs/ hashes to its line in
# CHECKSUMS.sha256, and that hash is the `cksum` crates.io's index gives for the version (index-snapshot/, the index
# entries as cargo cached them when the tarballs were taken). Prints one count line; exits non-zero on any mismatch.
# Usage: filmcraft/verify.sh [--checksums <file>]   (a different checksum file shows the check failing)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
sums="$here/CHECKSUMS.sha256"
if [ "${1:-}" = "--checksums" ]; then sums="$2"; fi
(cd "$here/tarballs" && sha256sum --quiet -c "$sums") || { echo "FAIL: a tarball does not match $sums" >&2; exit 1; }
ok=0; bad=0
while read -r sum file; do
  name="${file%-*.crate}"; vers="${file%.crate}"; vers="${vers##*-}"
  idx=$(tr '\0' '\n' < "$here/index-snapshot/$name" | grep "\"vers\":\"$vers\"" | grep -o '"cksum":"[0-9a-f]*"' | cut -d'"' -f4)
  if [ "$idx" = "$sum" ]; then ok=$((ok + 1)); else bad=$((bad + 1)); echo "MISMATCH: $file" >&2; fi
done < "$sums"
echo "filmcraft: $ok of $((ok + bad)) tarballs match crates.io's index, $bad mismatched"
[ "$bad" -eq 0 ] && [ "$ok" -eq 26 ]
