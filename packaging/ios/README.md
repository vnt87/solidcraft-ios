# SolidCraft iPad bundle

This Xcode target packages the Rust `solidcraft-ios` static library. It contains no
Swift or Objective-C product source. Open `SolidCraft.xcodeproj` and use the shared
`SolidCraft` scheme. Set your development team in Xcode for physical-device signing.

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
xcodebuild -project packaging/ios/SolidCraft.xcodeproj -scheme SolidCraft \
  -configuration Debug -destination 'generic/platform=iOS Simulator' build
python3 packaging/ios/validate-bundle.py path/to/SolidCraft.app
python3 -m unittest discover -s packaging/ios/tests   # plist, icon and build-phase contract
cargo xtask ios --sim
cargo xtask ios --sim --screenshot /tmp/solidcraft.png   # launch, wait, capture a PNG
```

`xtask ios --sim` builds, boots, installs, and launches through the simulator CLI. It targets the
named simulator (`--sim-name`, default `iPad Air 13-inch (M3)`), so the build only needs that
device's architecture, and it terminates an already-running copy before launching. The screenshot
step waits 12 s for Metal pipelines and fonts to settle; the first frames otherwise capture black.
For a physical iPad pass `--device --device-id <udid> --team <id>`; `--bundle-id <id>` overrides
the bundle identifier for both build and launch when the product id
(`io.tangencylabs.solidcraft`) cannot be registered to your team. It does not require a desktop
viewer. Xcode 27 provides Device Hub; `--show-simulator` currently targets the older Simulator
application.

The build phase resolves Cargo from `CARGO` or `$HOME/.cargo/bin` and builds only
`--lib`. It maps arm64 device, arm64 Simulator, and Intel Simulator architectures
and creates a universal archive when Xcode requests multiple architectures.
Debug uses Cargo's debug profile; Release uses `--release`. Artifacts default to
`target/ios-xcode`; `CARGO_TARGET_DIR` can override that location. Paths containing
spaces are supported. Rust targets must be installed before building.

The library must export Apple's ordinary C entry symbol `main`:
`extern "C" fn main(argc: i32, argv: *const *const c_char) -> i32`.
A dedicated link phase invokes Apple's clang driver because Xcode skips its
standard link phase for targets with no C/Swift/Objective-C sources. It links each
requested architecture using Xcode's SDK and deployment target, then creates the
bundle executable with lipo before signing. The linker pulls the Rust entry symbol
from the archive with `-u _main`. Release builds generate a dSYM.
The bundle validator checks that CFBundleExecutable exists, contains a Mach-O
EXECUTE header, and passes strict codesign verification. Use `--unsigned` only for
intentional unsigned builds. The runner owns UIKit startup and keeps its interop in
the Apple-only entry and scene modules.

The plist permits iPad multitasking and declares opening documents in place.
The scene manifest declares one SolidCraft window scene, required by iPadOS 27.
Multiple scenes remain disabled until independent window ownership is implemented.
The design document type advertises `.sc` (and the older `.solidcraft` and
`.solvecraft` extensions); the import formats the desktop app reads (STEP, STL, 3MF,
OBJ, …) are offered by the in-app document picker, which filters on the extensions the
command reads.
The entitlement file is deliberately empty: iOS already sandboxes applications;
macOS app-sandbox and user-selected-file entitlements do not belong here.

The asset catalog's `AppIcon` is the SolidCraft mark, drawn at 1024 pt by
`solidcraft-cli icon --size 1024` (the same drawing `cargo xtask app-icons` exports
everywhere else), flattened onto white with no alpha channel, since iOS applies its
own corner mask and rejects transparency.

## What works on iPadOS today

The first step is the window, the shell and documents, not the whole editor: the shared egui shell
starts on Metal, owns one UIKit window scene, hides the status bar, reports the safe area to egui,
keeps the desktop quit confirmation and shutdown hooks, persists preferences inside the app
container, and runs the full command registry. Open presents UIKit's document picker and copies
the choice into the container; Save writes to a reserved managed path and hands the file to an
export sheet the first time, then to the remembered destination silently. Files hands a `.sc`
design over through the scene (at launch or while the app runs) and it opens like a picked one,
a recent design reopens from the container, and Copy/Paste use the system pasteboard, so text
crosses to and from other apps. The iPad Air 13-inch (M3)
simulator boot has been verified: the ribbon, Browser, 3D viewport, orientation triad, view toolbar
and History timeline all draw, and the log shows egui and UIKit agreeing on the safe-area insets.
Still missing: multiple windows. `SolidCraftIos` logs to
`Library/Application Support/io.tangencylabs.solidcraft/logs/runner.log` inside the
app container, which `xcrun simctl get_app_container <sim> io.tangencylabs.solidcraft data`
locates. Preferences live next to it in `…/io.tangencylabs.solidcraft/app.ron`, and opened or
saved documents in `…/documents/<nanos-seq>/`.
