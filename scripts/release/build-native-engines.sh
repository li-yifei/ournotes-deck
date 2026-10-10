#!/bin/sh
# Cross-build the ournotes-ffi shared library for desktop hosts from macOS arm64.
# Needs: rustup targets below, zig, cargo-zigbuild. Usage: build-native-engines.sh OUTDIR
set -eu
out=${1:?usage: build-native-engines.sh OUTDIR}
root=$(cd "$(dirname "$0")/../.." && pwd)
def="$root/crates/ournotes-ffi/ournotes_ffi.def"
target_dir=${CARGO_TARGET_DIR:-$root/target}
export CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_STRIP=symbols
# Linux builds are opt-in (WITH_LINUX=1) while the Linux release is deferred.
# glibc 2.28 floor: Debian 10+, Ubuntu 18.10+/20.04, RHEL/Rocky 8+.
glibc=2.28
mkdir -p "$out"
cd "$root"

RUSTFLAGS="-C link-arg=-Wl,-install_name,@rpath/libournotes_ffi.dylib" cargo build --release -p ournotes-ffi
cp "$target_dir/release/libournotes_ffi.dylib" "$out/libournotes_ffi-darwin-arm64.dylib"

for pair in x86_64:amd64 aarch64:arm64; do
  arch=${pair%%:*} name=${pair##*:}
  if [ "${WITH_LINUX:-0}" = 1 ]; then
    cargo zigbuild --release -p ournotes-ffi --target "$arch-unknown-linux-gnu.$glibc"
    cp "$target_dir/$arch-unknown-linux-gnu/release/libournotes_ffi.so" "$out/libournotes_ffi-linux-$name.so"
  fi
  # gnullvm needs compiler-rt builtins from zig, and an explicit export list.
  RUSTFLAGS="-C link-arg=-rtlib=compiler-rt -C link-arg=$def" \
    cargo zigbuild --release -p ournotes-ffi --target "$arch-pc-windows-gnullvm"
  cp "$target_dir/$arch-pc-windows-gnullvm/release/ournotes_ffi.dll" "$out/libournotes_ffi-windows-$name.dll"
done

cd "$out" && shasum -a 256 libournotes_ffi-* > SHA256SUMS && cat SHA256SUMS
