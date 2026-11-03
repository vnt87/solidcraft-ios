//! Hardware-keyboard source: GameController's `GCKeyboard` reports every key down and up as a USB HID
//! usage code, independent of which view is first responder. The codes are queued here and drained by
//! [`crate::safe_area::RunnerApp`], which translates them with [`crate::hw_keys`].
use std::collections::VecDeque;
use std::ptr::NonNull;
use std::sync::Mutex;

use block2::RcBlock;
use eframe::egui;
use objc2::runtime::Bool;
use objc2_foundation::{NSNotification, NSNotificationCenter};
use objc2_game_controller::{GCControllerButtonInput, GCKeyCode, GCKeyboard, GCKeyboardDidConnectNotification, GCKeyboardInput};

/// Key transitions waiting for the next frame, as `(HID usage, pressed)`. Capped so a stalled UI
/// cannot grow it without bound.
static QUEUE: Mutex<VecDeque<(u32, bool)>> = Mutex::new(VecDeque::new());
const QUEUE_CAP: usize = 256;

/// Take every key transition since the last call.
pub fn drain() -> Vec<(u32, bool)> {
    match QUEUE.lock() {
        Ok(mut queue) => queue.drain(..).collect(),
        Err(poisoned) => poisoned.into_inner().drain(..).collect(),
    }
}

fn push(code: u32, pressed: bool) {
    let mut queue = match QUEUE.lock() {
        Ok(queue) => queue,
        Err(poisoned) => poisoned.into_inner(),
    };
    if queue.len() >= QUEUE_CAP {
        queue.pop_front();
    }
    queue.push_back((code, pressed));
}

/// Start listening: attach to the keyboard now if one is connected, and to any connected later.
/// `ctx` is woken on every key so the UI draws without waiting for another event.
pub fn install(ctx: egui::Context) {
    attach(&ctx);
    let on_connect = RcBlock::new({
        let ctx = ctx.clone();
        move |_: NonNull<NSNotification>| {
            log::info!("Hardware keyboard connected");
            attach(&ctx);
        }
    });
    // SAFETY: GCKeyboardDidConnectNotification is a valid, immortal NSString constant; the block
    // is copied by the center and `Fn(NonNull<NSNotification>)` matches the notification block.
    // The returned observer token is leaked on purpose: the observation lasts for the process.
    unsafe {
        let center = NSNotificationCenter::defaultCenter();
        let token = center.addObserverForName_object_queue_usingBlock(Some(GCKeyboardDidConnectNotification), None, None, &on_connect);
        std::mem::forget(token);
    }
}

fn attach(ctx: &egui::Context) {
    // SAFETY: plain GameController getters, callable from any thread.
    let Some(keyboard) = (unsafe { GCKeyboard::coalescedKeyboard() }) else {
        log::info!("No hardware keyboard attached to GameController yet");
        return;
    };
    // SAFETY: as above.
    let Some(input) = (unsafe { keyboard.keyboardInput() }) else {
        log::warn!("The hardware keyboard exposes no key input");
        return;
    };
    let ctx = ctx.clone();
    let handler = RcBlock::new(move |_: NonNull<GCKeyboardInput>, _: NonNull<GCControllerButtonInput>, code: GCKeyCode, pressed: Bool| {
        if let Ok(code) = u32::try_from(code) {
            push(code, pressed.as_bool());
            ctx.request_repaint();
        }
    });
    // SAFETY: the handler matches `GCKeyboardValueChangedHandler`; GameController copies the block,
    // so it outlives `handler`. It replaces any earlier handler, so reconnects never double up.
    unsafe { input.setKeyChangedHandler(&*handler as *const _ as *mut _) };
    log::info!("Hardware keyboard attached");
}
