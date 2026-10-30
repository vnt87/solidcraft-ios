#!/bin/sh
# Xcode does not inherit the interactive shell's Rust PATH.
set -eu
export PATH="${HOME}/.cargo/bin:${PATH}"
repository_root=$(CDPATH= cd -- "${SRCROOT}/../.." && pwd)
if [ -n "${CARGO:-}" ]; then
    cargo_bin=$CARGO
else
    cargo_bin=$(command -v cargo) || { echo 'error: Install Rust or set CARGO to its executable path.' >&2; exit 1; }
fi
case "${PLATFORM_NAME}" in
    iphonesimulator) platform=simulator ;;
    iphoneos) platform=device ;;
    *) echo "error: Unsupported Apple platform: ${PLATFORM_NAME}" >&2; exit 1 ;;
esac
case "${CONFIGURATION}" in
    Debug) profile=debug; release_flag= ;;
    Release) profile=release; release_flag=--release ;;
    *) echo "error: Unsupported configuration: ${CONFIGURATION}" >&2; exit 1 ;;
esac
# Xcode runs build phases from packaging/ios, so a relative override would land inside the
# project instead of the repository; make it relative to the repository root, where the user
# typed it.
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    case "${CARGO_TARGET_DIR}" in
        /*) ;;
        *) CARGO_TARGET_DIR="${repository_root}/${CARGO_TARGET_DIR}" ;;
    esac
fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${repository_root}/target/ios-xcode}"
mkdir -p "${BUILT_PRODUCTS_DIR}/rust"
set --
for arch in ${ARCHS}; do
    case "$platform:$arch" in
        simulator:arm64) rust_target=aarch64-apple-ios-sim ;;
        simulator:x86_64) rust_target=x86_64-apple-ios ;;
        device:arm64) rust_target=aarch64-apple-ios ;;
        *) echo "error: Unsupported iOS architecture: $platform:$arch" >&2; exit 1 ;;
    esac
    # Intentionally build only the static library, never the standalone Cargo binary.
    "$cargo_bin" build --manifest-path "${repository_root}/Cargo.toml" --package solidcraft-ios --lib --target "$rust_target" $release_flag
    library="${CARGO_TARGET_DIR}/${rust_target}/${profile}/libsolidcraft_ios.a"
    if [ ! -f "$library" ]; then
        echo "error: Missing Rust static library: $library" >&2
        exit 1
    fi
    # Keep paths as distinct arguments even when the checkout contains spaces.
    if [ "$#" -eq 0 ]; then
        set -- "$library"
    else
        set -- "$@" "$library"
    fi
done
if [ "$#" -eq 0 ]; then
    echo 'error: Xcode supplied no ARCHS.' >&2
    exit 1
fi
xcrun lipo -create "$@" -output "${BUILT_PRODUCTS_DIR}/rust/libsolidcraft_ios.a"
