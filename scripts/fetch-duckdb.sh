#!/bin/sh
# Project-local pinned runtime dependency, no global install or remote script.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
asset_dir="$project_dir/.build/duckdb"
mkdir -p "$asset_dir"
if [ ! -f "$asset_dir/duckdb-v2.0.0-arm64.tar.gz" ]; then
  curl --fail --location --retry 2 --output "$asset_dir/duckdb-v2.0.0-arm64.tar.gz" \
    https://github.com/duckdb/duckdb/releases/download/v2.0.0/duckdb-shared-libs-osx-arm64.tar.gz
fi
expected_hash=c35d60080b31ba5d11a3ce505a6831be784f003b4c575d32a4c2badb3b41cdf0
actual_hash=$(/usr/bin/shasum -a 256 "$asset_dir/duckdb-v2.0.0-arm64.tar.gz")
actual_hash=${actual_hash%% *}
if [ "$actual_hash" != "$expected_hash" ]; then
  echo 'DuckDB archive checksum mismatch' >&2
  exit 3
fi
/usr/bin/tar -xzf "$asset_dir/duckdb-v2.0.0-arm64.tar.gz" -C "$asset_dir"
