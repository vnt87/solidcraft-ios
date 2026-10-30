#!/bin/sh
# Source-empty Xcode targets skip Ld. Link the Rust archive with Apple's driver.
set -eu
mkdir -p "${TARGET_TEMP_DIR}/rust-linked" "${TARGET_BUILD_DIR}/${WRAPPER_NAME}"
set --
for arch in ${ARCHS}; do
    case "${PLATFORM_NAME}:${arch}" in
        iphonesimulator:arm64) triple="arm64-apple-ios${IPHONEOS_DEPLOYMENT_TARGET}-simulator" ;;
        iphonesimulator:x86_64) triple="x86_64-apple-ios${IPHONEOS_DEPLOYMENT_TARGET}-simulator" ;;
        iphoneos:arm64) triple="arm64-apple-ios${IPHONEOS_DEPLOYMENT_TARGET}" ;;
        *) echo "error: Unsupported iOS link architecture: ${PLATFORM_NAME}:${arch}" >&2; exit 1 ;;
    esac
    output="${TARGET_TEMP_DIR}/rust-linked/SolidCraft-${arch}"
    xcrun --sdk "${PLATFORM_NAME}" clang -target "$triple" -isysroot "${SDKROOT}" \
        -Wl,-u,_main "${BUILT_PRODUCTS_DIR}/rust/libsolidcraft_ios.a" \
        -framework UIKit -framework Foundation -framework Metal -framework MetalKit \
        -framework QuartzCore -framework CoreGraphics -framework CoreText \
        -framework Security -framework SystemConfiguration -framework CoreFoundation \
        -framework CoreVideo -framework IOSurface -lc++ -lz -lobjc -liconv \
        -o "$output"
    if [ "$#" -eq 0 ]; then set -- "$output"; else set -- "$@" "$output"; fi
done
if [ "$#" -eq 0 ]; then echo 'error: Xcode supplied no ARCHS to linker.' >&2; exit 1; fi
executable="${TARGET_BUILD_DIR}/${EXECUTABLE_PATH}"
xcrun lipo -create "$@" -output "$executable"
chmod +x "$executable"
# This catches accidental dylibs/archives immediately, before Xcode signs a bundle.
xcrun otool -hv "$executable" | /usr/bin/grep -q EXECUTE

if [ "${DEBUG_INFORMATION_FORMAT:-dwarf}" = dwarf-with-dsym ]; then
    xcrun dsymutil "$executable" -o "${DWARF_DSYM_FOLDER_PATH}/${DWARF_DSYM_FILE_NAME}"
fi
