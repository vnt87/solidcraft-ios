# solidcraft-ios

The SolidCraft iPadOS runner: a static Rust library (`libsolidcraft_ios.a`) linked into the
Xcode bundle in [`packaging/ios`](../../packaging/ios), with Apple's `main` entry symbol
exported from `src/apple_entry.rs`.

| Module | Job |
|---|---|
| `apple_entry` | the exported `main`/`solidcraft_ios_main`; catches unwinding panics at the C ABI |
| `scenes` | one `UIWindowScene`, its `UIWindow`, the Files app's URLs, the system pasteboard, and the safe-area metrics egui needs |
| `safe_area` | `RunnerApp`, the runner's `eframe::App`: delegates the shell, feeds UIKit's insets to egui, mirrors copies to the pasteboard, opens what the document layer made ready, saves preferences, asks about unsaved designs before the scene closes |
| `documents` | managed copies of opened files, a reserved path for every save, the shared `publish` step, and the recent designs that reopen from the store; its `native` submodule is the audited UIKit boundary |
| `logging` | app-container `runner.log`, so a headless simulator run can be read afterwards |
| `lib` | Metal `eframe` options, preferences path, document services, then the shared `solidcraft_ui_egui::SolveApp` |

```sh
cargo xtask ios --sim --screenshot /tmp/solidcraft.png
cargo test -p solidcraft-ios        # host tests of the pure parts
```

The shell's desktop-only work is kept: the crash journal, the Close/quit confirmation, the
shutdown hooks (`agent_chat`, `save_job`, `capture`, recovery and open-elsewhere services) and
preference persistence. Preferences are written to `Library/Application Support/
io.tangencylabs.solidcraft/app.ron` inside the app container; `eframe` cannot derive that path
for iOS, so `lib.rs` supplies it. The status bar is hidden through winit's
`WindowExtIOS::set_prefers_status_bar_hidden`, so the shell starts at the top of the screen while
UIKit's bottom inset still reaches egui.

Documents work the way iPadOS requires. iOS hands an app a security-scoped URL, not a lasting
path, so Open copies the picked design (or mesh, HDR, CSV…) into `documents/<nanos-seq>/` under
`Application Support` and the engine opens that copy; the save picker reserves a fresh managed
path instead of asking for a location. Once the bytes are written, `Services::publish` takes the
file to the user: the first save shows one `UIDocumentPickerViewController` export sheet and
remembers the chosen folder as a bookmark beside the document, and later saves write to that
bookmark silently. Imports and exports are coordinated through `NSFileCoordinator` and balanced
by a security-scope guard, so an iCloud provider cannot pull the file out mid-write.

Still missing: multiple windows. The design document type and in-place opening are declared in
the bundle plist, so Files offers SolidCraft for `.sc` files.

The Files app opens a design into the app: the scene hands over its security-scoped URL (at a
cold launch through the connection options, later through `scene:openURLContexts:`), the URL is
copied into the managed store on a worker like a picked file, and the copy opens a frame later
through the engine's normal open path.

The clipboard is the system's. Copy puts egui's text (a field's selection, a report, the line a
design's Copy leaves) on `UIPasteboard.general`, so other apps can paste it; a paste event
carries the pasteboard's text when it has some, so text copied in another app reaches a text
field with ⌘V (iOS asks the first time). egui-winit's in-app clipboard still supplies the copy
and paste events themselves: with a keyboard, a paste only arrives once this app has copied
something since launch, and without one the field's context menu (a right-click or a trackpad's
secondary click) reads the pasteboard when it opens.

Recent designs reopen. `Services::can_restore` answers whether a recent design's managed copy is
still on disk and `Services::restore_file` opens it through the same open path, so a design from
an earlier launch comes back instead of a "missing" note.

Navigation starts on the **Touch (iPad)** preset: the shell picks it on the first run
(`preferences.touch_nav_seen`) and keeps whatever the user chooses afterwards in Preferences ›
Navigation. Fingers and the Pencil are read from the frame's touches before egui turns the first
one into a pointer (`crates/ui-egui/src/touch.rs`), and the camera still moves only through
`navigation::handle`:

| Input | What happens |
|---|---|
| One finger drag from empty space | Orbits the view |
| One finger drag on geometry or a sketch glyph | Selects and drags, as with a mouse |
| Two fingers drag | Pans the model; the pointer stays quiet, so nothing under the first finger moves |
| Pinch | Zooms around the centre of the fingers |
| Tap, long press | A click; the context menu and Select Other work as on the desktop |
| Apple Pencil drag | Selects, drags geometry, draws; it never moves the camera |
| Pencil down with the palm on the screen | The palm is not counted as a finger, so no gesture starts |

The preset still carries mouse bindings (left drag from empty space orbits, middle or right drag
pans), so a paired mouse or trackpad works. Not implemented: two-finger double tap, the Pencil's
double tap (tool switch), Pencil hover, and pressure-sensitive strokes. A paired trackpad used at
the same time as fingers on the screen is the one case the pointer suppression cannot tell apart;
use one at a time.
