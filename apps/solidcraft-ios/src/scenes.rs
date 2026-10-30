//! Single-scene UIKit ownership around winit's existing window and event loop.
//!
//! winit observes UIApplication's aggregate notifications, which UIKit continues to
//! post with scenes. Forwarding scene callbacks would deliver lifecycle events twice.
use objc2::rc::Retained;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class};
use objc2_foundation::{NSObjectProtocol, NSSet, NSString};
use objc2_ui_kit::{
    UICoordinateSpace, UIOpenURLContext, UIPasteboard, UIResponder, UIScene, UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIView, UIWindow,
    UIWindowScene, UIWindowSceneDelegate,
};
use std::cell::RefCell;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

thread_local! {
    // UIKit owns scene lifetime; retain the one connected scene while winit creates
    // its window after launch. Access never holds a borrow across a UIKit call.
    static SCENE: RefCell<Option<Retained<UIWindowScene>>> = const { RefCell::new(None) };
    // eframe keeps the same root window across suspension and scene reconnection.
    static WINDOW: RefCell<Option<Retained<UIWindow>>> = const { RefCell::new(None) };
}

define_class!(
    // SAFETY: UIResponder has no extra subclass invariants. UIKit creates this
    // delegate on the main thread and invokes the declared public signatures there.
    #[unsafe(super = UIResponder)]
    #[thread_kind = MainThreadOnly]
    #[name = "SolidCraftSceneDelegate"]
    struct SceneDelegate;
    // SAFETY: NSObjectProtocol adds no implementation invariants.
    unsafe impl NSObjectProtocol for SceneDelegate {}
    // SAFETY: All arguments are UIKit-owned and valid during these main-thread callbacks.
    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn connected(&self, scene: &UIScene, _session: &UISceneSession, options: &UISceneConnectionOptions) {
            // A document opened from the Files app before the window existed (a cold launch).
            // The generated accessor treats the set as non-null and panics on nil, which iPadOS 27
            // returns for an ordinary launch, so the property is read nil-tolerantly instead.
            // SAFETY: `options` is live for this callback; the property returns an NSSet or nil.
            let contexts: Option<Retained<NSSet<UIOpenURLContext>>> = unsafe { objc2::msg_send![options, URLContexts] };
            if let Some(contexts) = contexts {
                open_urls(&contexts);
            }
            let Some(scene) = scene.downcast_ref::<UIWindowScene>() else {
                log::error!("SolidCraft received a scene without a window");
                return;
            };
            // SAFETY: The callback supplies a live UIKit scene; retain before storing
            // beyond the callback. This object stays confined to the main thread.
            let retained = unsafe { Retained::retain(scene as *const UIWindowScene as *mut UIWindowScene) };
            let connected = SCENE.with(|slot| {
                let Ok(mut slot) = slot.try_borrow_mut() else {
                    log::error!("SolidCraft scene connection was reentrant");
                    return false;
                };
                if slot.is_some() {
                    log::error!("SolidCraft supports only one connected scene");
                    return false;
                }
                *slot = retained;
                true
            });
            if !connected {
                return;
            }
            if let Some(window) = WINDOW.with(|slot| slot.try_borrow().ok().and_then(|slot| slot.clone())) {
                associate(&window, scene);
            }
        }

        #[unsafe(method(sceneDidDisconnect:))]
        fn disconnected(&self, scene: &UIScene) {
            SCENE.with(|slot| {
                let Ok(mut slot) = slot.try_borrow_mut() else {
                    log::error!("SolidCraft scene disconnection was reentrant");
                    return;
                };
                let matches = slot.as_ref().is_some_and(|stored| std::ptr::eq(Retained::as_ptr(stored).cast::<UIScene>(), scene));
                if matches {
                    *slot = None;
                }
            });
        }

        #[unsafe(method(scene:openURLContexts:))]
        fn opened(&self, _scene: &UIScene, contexts: &NSSet<UIOpenURLContext>) {
            // The Files app hands a design over while the app runs.
            open_urls(contexts);
        }
    }
    // SAFETY: Single-window behavior uses the inherited UISceneDelegate callbacks;
    // all other UIWindowSceneDelegate methods are optional.
    unsafe impl UIWindowSceneDelegate for SceneDelegate {}
);

/// Register the class before UIApplicationMain reads the scene manifest.
pub fn register() {
    let _ = SceneDelegate::class();
}

fn scene() -> Option<Retained<UIWindowScene>> {
    SCENE.with(|slot| slot.try_borrow().ok().and_then(|slot| slot.clone()))
}

/// Associate the winit-owned view hierarchy with the connected UIKit scene.
pub fn attach(cc: &eframe::CreationContext<'_>) -> Result<(), String> {
    let _mtm = MainThreadMarker::new().ok_or("UIKit scene attachment requires the main thread")?;
    let scene = scene().ok_or("No connected UIKit window scene")?;
    let native = cc.winit_window().ok_or("eframe did not supply its native window")?;
    let handle = native.window_handle().map_err(|error| error.to_string())?;
    let RawWindowHandle::UiKit(handle) = handle.as_raw() else {
        return Err("eframe supplied a non-UIKit window".into());
    };
    // SAFETY: The borrowed handle is valid while `native` and its handle guard live.
    // winit owns this UIView; UIKit access and attachment occur on the main thread.
    let view = unsafe { handle.ui_view.cast::<UIView>().as_ref() };
    let window = view.window().ok_or("winit's UIKit view has no window")?;
    associate(&window, &scene);
    WINDOW.with(|slot| {
        let mut slot = slot.try_borrow_mut().map_err(|_| "UIKit window attachment was reentrant")?;
        *slot = Some(window);
        Ok::<_, String>(())
    })
}

fn associate(window: &UIWindow, scene: &UIWindowScene) {
    window.setWindowScene(Some(scene));
    // This public coordinate-space API also exists at our iOS 17 deployment floor.
    #[allow(deprecated)]
    let bounds = scene.coordinateSpace().bounds();
    window.setFrame(bounds);
    window.makeKeyAndVisible();
}

/// The system pasteboard's text. Read only on a paste the user makes: iOS asks before handing an
/// app text it did not put there itself.
pub fn pasteboard_text() -> Option<String> {
    let _mtm = MainThreadMarker::new()?;
    // SAFETY: UIPasteboard's accessors are not thread-safe; this runs on the main thread.
    let text = unsafe { UIPasteboard::generalPasteboard().string() }?;
    (!text.is_empty()).then(|| text.to_string())
}

/// Put text on the system pasteboard, so other apps paste what this app copied.
pub fn set_pasteboard_text(text: &str) {
    if MainThreadMarker::new().is_none() {
        log::error!("The pasteboard was written off the main thread");
        return;
    }
    // SAFETY: UIPasteboard's accessors are not thread-safe; this runs on the main thread.
    unsafe { UIPasteboard::generalPasteboard().setString(Some(&NSString::from_str(text))) };
}

/// Hand the Files app's URLs to the document layer, which copies them in and opens them.
fn open_urls(contexts: &NSSet<UIOpenURLContext>) {
    for context in contexts.iter() {
        if let Err(error) = crate::documents::open_external(context.URL()) {
            log::error!("Could not open the design the Files app sent: {error}");
        }
    }
}

/// Find the window in our connected scene without deprecated application globals.
pub fn window(_mtm: MainThreadMarker) -> Option<Retained<UIWindow>> {
    let scene = scene()?;
    scene.keyWindow().or_else(|| scene.windows().iter().find(|window| window.isKeyWindow()))
}

/// Live UIKit metrics of the connected window in points: `[width, height, top, left, bottom, right]`.
/// The insets are the safe-area insets that winit subtracts for the surface size.
pub fn metrics(mtm: MainThreadMarker) -> Option<[f64; 6]> {
    let window = window(mtm)?;
    let bounds = window.bounds();
    let insets = window.safeAreaInsets();
    Some([bounds.size.width, bounds.size.height, insets.top, insets.left, insets.bottom, insets.right])
}
