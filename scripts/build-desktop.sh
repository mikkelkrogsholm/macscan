#!/bin/sh
# Builds a local app; never installs globally or alters scan targets.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
toolchain_bin="$HOME/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin"
export RUSTC="$toolchain_bin/rustc"
export RUSTDOC="$toolchain_bin/rustdoc"
export CARGO_TARGET_DIR="$project_dir/.build"
export CARGO_BUILD_JOBS=4
export CARGO_INCREMENTAL=0
export MACOSX_DEPLOYMENT_TARGET=14.0
export PATH="$toolchain_bin:$HOME/.cargo/bin:$PATH"
cd "$project_dir"
/opt/homebrew/bin/node --test ui/sunburst.test.mjs
"$toolchain_bin/cargo" test --workspace --locked --offline
"$toolchain_bin/cargo" build -p macscan-desktop --release --locked --offline
cd crates/desktop
"$HOME/.cargo/bin/cargo-tauri" bundle --bundles app --ci
cd "$project_dir"
if [ -L dist ] || [ -L dist/Macscan.app ]; then
  echo 'Refusing symlink artifact destination' >&2
  exit 3
fi
mkdir -p dist
/usr/bin/ditto .build/release/bundle/macos/Macscan.app dist/Macscan.app
/usr/bin/codesign --force --sign - dist/Macscan.app/Contents/Frameworks/libduckdb.dylib
/usr/bin/codesign --force --sign - dist/Macscan.app
/usr/bin/codesign --verify --deep --strict dist/Macscan.app
/usr/bin/du -sh dist/Macscan.app
/usr/bin/shasum -a 256 dist/Macscan.app/Contents/MacOS/macscan-desktop
