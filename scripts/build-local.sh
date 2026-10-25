#!/bin/sh
# Local arm64 CLI artifact only. Does not install or touch scan targets.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
toolchain_bin="$HOME/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin"
export RUSTC="$toolchain_bin/rustc"
export RUSTDOC="$toolchain_bin/rustdoc"
export RUSTC_WRAPPER=
export RUSTC_WORKSPACE_WRAPPER=
export CARGO_TARGET_DIR="$project_dir/.build"
export CARGO_BUILD_JOBS=4
export CARGO_INCREMENTAL=0
export MACOSX_DEPLOYMENT_TARGET=14.0
cd "$project_dir"
if [ ! -f .build/duckdb/libduckdb.dylib ]; then
  echo 'Run scripts/fetch-duckdb.sh once before the offline build.' >&2
  exit 3
fi
"$toolchain_bin/cargo" test --workspace --locked --offline
"$toolchain_bin/cargo" build -p macscan --release --locked --offline
if [ -L dist ] || [ -L dist/macscan ]; then
  echo 'Refusing symlink artifact destination' >&2
  exit 3
fi
mkdir -p dist
/usr/bin/install -m 755 .build/release/macscan dist/macscan
/usr/bin/install -m 755 .build/duckdb/libduckdb.dylib dist/libduckdb.dylib
/usr/bin/strip -S dist/macscan
/usr/bin/codesign --force --sign - dist/libduckdb.dylib
/usr/bin/codesign --force --sign - dist/macscan
/usr/bin/codesign --verify --strict dist/macscan
/usr/bin/stat -f '%z bytes %N' dist/macscan
/usr/bin/shasum -a 256 Cargo.lock dist/macscan dist/libduckdb.dylib
