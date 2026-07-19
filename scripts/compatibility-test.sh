#!/usr/bin/env bash
set -euo pipefail

calibredb_bin="${CALIBREDB:-$(command -v calibredb || true)}"
if [[ -z "$calibredb_bin" ]]; then
  echo "CALIBREDB must name a Calibre calibredb executable" >&2
  exit 2
fi

"$calibredb_bin" --version
CALIBREDB="$calibredb_bin" cargo test --test calibre_oracle -- --ignored --nocapture
