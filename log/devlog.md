## 2026-11-17 — Upstream sync (SolidCraft, sync/upstream-2026-11-17)

- `Cargo.lock` version bump committed (`814f1ac40` on the previous branch tip), then `upstream/main` (`69aeb3213`,
  504 commits) merged in `568d0c482`. Branch `sync/upstream-2026-11-17` pushed to `origin`. No device deploy.
- Conflicts: `crates/ui-egui/src/file_picker.rs` ×4 and `crates/ui-egui/src/lib.rs` ×1.
  - Upstream rewrote the async picker: `Pending` now holds `Pin<Box<dyn Future<Output = Ready>>>` and answers go
    through the new `begin_file_task`. Adopted it, and re-ported the fork's publish-on-export hook into that
    completion (`1c68a4d02`) instead of `tick()`. The `Box<FnOnce>` version did not compile (E0525), so the hook is
    an inline `FnOnce` closure; `pick_folder` now comes from upstream (the fork's own copy was dropped).
  - `Services`: kept both sides — the fork's `clipboard_get`/`clipboard_set` (pasteboard bridge in
    `apps/solidcraft-ios`) and upstream's `tickets_http`/`ticket_secrets`.
- Verification: `cargo xtask layers` ok (38 crates), Tier 1 `cargo check -p solidcraft-ios --lib --target
  aarch64-apple-ios-sim` ok, `cargo xtask wasm` ok (30 crates), Tier 2 `cargo test -p solidcraft-ui-egui --lib`
  5940 passed, 7 failed — all seven reproduce identically on a clean `upstream/main` worktree on this Mac:
  five `Dropped TexturesDelta ... unapplied deltas` panics (issue_report ×2, library quick-focus, navigation
  trackpad, prefs Windows drag) and two `HostUnavailable: CPU raytracing is not compiled in; enable raytrace-cpu`
  (the `ui-egui` test target does not turn on `solidcraft-engine/raytrace-cpu`, which the app enables by default).
- Still open: those seven for upstream/CI; nothing carried broke. `target/agent-sync` (48 GB) deleted to reclaim disk.

## 2026-11-19 — upstream/main sync (615 commits)

- Merged `upstream/main` (`95ed35824`) into `sync/upstream-2026-11-19` off `5a0a68ab3`; five conflicts
  resolved: `ROADMAP.md` (upstream short rewrite; fork's Touch (iPad) navigation paragraph migrated into
  `docs/roadmap-detail.md`), `crates/ui-egui/src/lib.rs` (upstream `raw_input_hook*` delegation plus fork
  `touch.update`), `crates/ui-egui/src/navigation.rs` (`touch.active || app.touch.gesture()`),
  `crates/ui-egui/src/prefs.rs` (both `touchscreen_navigation` and `touch_nav_seen`), and
  `crates/ui-kit/src/lib.rs` (both `app_bar_layout` and `clipboard`).
- Verification: `cargo xtask layers` ok (39 crates), `cargo xtask wasm` ok (31 crates), `cargo check -p
  solidcraft-ios --all-targets --target aarch64-apple-ios-sim` ok, `cargo test -p solidcraft-ios --lib`
  26 passed, `cargo test -p solidcraft-ui-egui --lib` 6266 passed / 13 failed, clippy on
  `solidcraft-ui-egui`/`solidcraft-ui-kit`/`solidcraft-ios` with `-D warnings` ok.
- Twelve of the thirteen ui-egui failures reproduce on pristine `upstream/main`: vendored-egui
  `Dropped TexturesDelta with 1 unapplied deltas` panics. The thirteenth
  (`stand_up_memory_reopens_changed_files_and_can_be_disabled_or_forgotten`) passes on both pristine and
  the pre-merge fork tip; it fails only while the merge is uncommitted, because `crates/io/build.rs`
  hashes `crates/develop` from `HEAD` (still the fork tip, where that crate does not exist), making
  `SOLIDCRAFT_EVALUATION_SOURCE=unknown` and the import cache untrusted. Re-run after the merge commit.
- Still open: confirm the thirteenth test after the commit, then vectorcraft sync; IPA builds and NAS
  copy are waiting on the user's go-ahead and NAS credentials.
