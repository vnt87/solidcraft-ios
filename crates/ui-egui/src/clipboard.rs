//! The system clipboard's text.
//!
//! egui's own clipboard is a string inside the window layer: on the desktop it reaches the system
//! clipboard, but on iPadOS it only feeds this process, so a host that has one (the runner's
//! `UIPasteboard` bridge) hands over its reader and writer here ([`install`]). Without a host the
//! desktop reads the clipboard through arboard, and the browser has none.

use std::cell::RefCell;

/// Reads the system clipboard's text; `None` when it holds none.
pub type GetText = Box<dyn Fn() -> Option<String>>;
/// Replaces the system clipboard with one text.
pub type SetText = Box<dyn Fn(&str)>;

thread_local! {
    /// The host's bridge, installed as the app is built (the clipboard is per process anyway).
    static HOST: RefCell<Option<(Option<GetText>, Option<SetText>)>> = const { RefCell::new(None) };
}

/// Install the host's clipboard bridge (`None, None` on a host with no system clipboard).
pub fn install(get: Option<GetText>, set: Option<SetText>) {
    HOST.with(|host| *host.borrow_mut() = Some((get, set)));
}

/// The system clipboard's text, if it holds any.
pub fn text() -> Option<String> {
    let answer = HOST.with(|host| {
        let host = host.try_borrow().ok()?;
        let get = host.as_ref()?.0.as_ref()?;
        Some(get())
    });
    match answer {
        Some(text) => text.filter(|text| !text.is_empty()),
        None => system_text(),
    }
}

/// The desktop's clipboard, read directly when the host has no bridge.
#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn system_text() -> Option<String> {
    arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()).ok().filter(|text| !text.is_empty())
}

/// No system clipboard to read: the browser only hands over a paste the user makes.
#[cfg(any(test, target_arch = "wasm32"))]
fn system_text() -> Option<String> {
    None
}

/// Copy text: the host's clipboard when it has one, and egui's (what egui's own widgets paste and
/// what its window layer keeps, so a following paste carries it).
pub fn copy(ctx: &egui::Context, text: String) {
    HOST.with(|host| {
        let Ok(host) = host.try_borrow() else { return };
        if let Some((_, Some(set))) = host.as_ref() {
            set(&text);
        }
    });
    ctx.copy_text(text);
}
