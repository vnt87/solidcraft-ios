//! Audited UIKit boundary: the document pickers, security-scoped reads and writes, and
//! destination bookmarks. The UI thread only presents; imports and exports run on workers.
use super::{MAX_DOCUMENT_BYTES, ReservationBatch, bounded_read, bounded_read_limit, bookmark_path, file_name, reserve_path, support_directory, validate_managed_path, write_atomic};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{
    NSArray, NSData, NSDataWritingOptions, NSError, NSFileCoordinator, NSFileCoordinatorReadingOptions, NSFileCoordinatorWritingOptions, NSObject,
    NSObjectProtocol, NSString, NSURL, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions,
};
use objc2_ui_kit::{UIDocumentPickerDelegate, UIDocumentPickerViewController, UIViewController};
use objc2_uniform_type_identifiers::UTType;
use solidcraft_ui_egui::{Services, SolveApp};
use std::cell::RefCell;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::mpsc::{self, TryRecvError};
use std::task::{Context, Poll};

thread_local! {
    // UIKit's delegate is weak: hold both objects until its terminal callback.
    static ACTIVE: RefCell<Option<(Retained<PickerDelegate>, Retained<UIDocumentPickerViewController>)>> = const { RefCell::new(None) };
    // The export sheet already shows this file: publishing it again is the same export.
    static IN_FLIGHT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    // Managed designs waiting for the app to open them: the Files app's copies and the home
    // screen's recent designs. The UI takes them once a frame.
    static READY: RefCell<Option<(mpsc::Sender<String>, mpsc::Receiver<String>)>> = const { RefCell::new(None) };
    // The window, to draw a frame right after something became ready.
    static UI: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// The answer of an open picker: the managed copy's path, or `None` for cancel and failure.
struct Reply {
    answer: mpsc::Sender<Option<String>>,
    ui: egui::Context,
}

impl Reply {
    fn send(&self, path: Option<String>) {
        let _ = self.answer.send(path);
        self.ui.request_repaint();
    }
}

enum Completion {
    /// Copying the picked file into the container runs on a worker.
    Open { reply: Reply, root: PathBuf },
    /// Remember where the user put the staged export, then drop the staging file.
    Export { managed: PathBuf, stage: PathBuf },
}

struct PickerState {
    completion: RefCell<Option<Completion>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. This class is confined to the
    // main thread and owns only Rust ivars; UIKit callbacks use the declared signatures.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = PickerState]
    struct PickerDelegate;
    // SAFETY: NSObjectProtocol has no additional invariants.
    unsafe impl NSObjectProtocol for PickerDelegate {}
    // SAFETY: All callbacks execute on UIKit's main thread; arguments are borrowed only
    // during the call. Selected NSURLs are retained before crossing to a worker.
    unsafe impl UIDocumentPickerDelegate for PickerDelegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn picked(&self, _picker: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
            if let Some(completion) = self.ivars().completion.borrow_mut().take() {
                match completion {
                    Completion::Open { reply, root } => {
                        // A failed import must still answer, or the UI waits on the picker forever.
                        let Some(url) = urls.iter().next() else {
                            log::error!("The document picker returned no document");
                            reply.send(None);
                            release_picker();
                            return;
                        };
                        let spawned = std::thread::Builder::new().name("solidcraft-document-import".into()).spawn(move || {
                            let answer = import_document(url, &root).unwrap_or_else(|error| {
                                log::error!("Could not import the picked document: {error}");
                                None
                            });
                            reply.send(answer);
                        });
                        if let Err(error) = spawned {
                            log::error!("Could not start the document import: {error}");
                        }
                    }
                    Completion::Export { managed, stage } => {
                        let result = urls.iter().next().ok_or("The document picker returned no destination".to_string()).and_then(|url| remember_destination(&managed, &url));
                        if let Err(error) = result {
                            log::error!("Could not remember the export destination: {error}");
                        }
                        let _ = std::fs::remove_file(&stage);
                    }
                }
            }
            release_picker();
        }

        #[unsafe(method(documentPickerWasCancelled:))]
        fn cancelled(&self, _picker: &UIDocumentPickerViewController) {
            if let Some(completion) = self.ivars().completion.borrow_mut().take() {
                match completion {
                    Completion::Open { reply, .. } => reply.send(None),
                    Completion::Export { stage, .. } => {
                        let _ = std::fs::remove_file(&stage);
                    }
                }
            }
            release_picker();
        }
    }
);

fn release_picker() {
    // Retain the receiver until after UIKit has returned from the callback.
    dispatch2::DispatchQueue::main().exec_async(|| {
        ACTIVE.with(|active| {
            active.borrow_mut().take();
        });
        IN_FLIGHT.with(|export| {
            export.borrow_mut().take();
        });
    })
}

fn root_controller(mtm: objc2::MainThreadMarker) -> Result<Retained<UIViewController>, String> {
    let window = crate::scenes::window(mtm);
    let window = window.ok_or("No active UIKit window")?;
    let root = window.rootViewController().ok_or("The UIKit window has no root controller")?;
    if root.presentedViewController().is_some() {
        return Err("Another UIKit dialog is already open".into());
    }
    Ok(root)
}

fn fail_completion(completion: Completion, error: String) {
    match completion {
        Completion::Open { reply, .. } => {
            log::error!("The document picker could not open: {error}");
            reply.send(None);
        }
        Completion::Export { stage, .. } => {
            log::error!("The export picker could not open: {error}");
            IN_FLIGHT.with(|export| export.borrow_mut().take());
            let _ = std::fs::remove_file(stage);
        }
    }
}

fn show_picker(picker: Retained<UIDocumentPickerViewController>, completion: Completion, mtm: objc2::MainThreadMarker) {
    let root = match root_controller(mtm) {
        Ok(root) => root,
        Err(error) => {
            fail_completion(completion, error);
            return;
        }
    };
    if ACTIVE.with(|active| active.borrow().is_some()) {
        fail_completion(completion, "A document picker is already open".into());
        return;
    }
    let allocated = PickerDelegate::alloc(mtm).set_ivars(PickerState { completion: RefCell::new(Some(completion)) });
    // SAFETY: NSObject init initializes this allocated subclass, on the main thread;
    // Rust ivars have been initialized and ownership transfers to Retained.
    let delegate: Retained<PickerDelegate> = unsafe { msg_send![super(allocated), init] };
    picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    ACTIVE.with(|active| *active.borrow_mut() = Some((delegate, picker.clone())));
    root.presentViewController_animated_completion(&picker, true, None);
}

/// Security access is tied to the retained immutable URL and balanced even on read errors.
struct SecurityAccess {
    url: Retained<NSURL>,
    started: bool,
}

impl SecurityAccess {
    fn new(url: Retained<NSURL>) -> Self {
        // SAFETY: NSURL is retained and immutable (the binding implements Send + Sync).
        // Foundation permits security scope on worker threads; Drop balances only success.
        let started = unsafe { url.startAccessingSecurityScopedResource() };
        Self { url, started }
    }
}

impl Drop for SecurityAccess {
    fn drop(&mut self) {
        if self.started {
            // SAFETY: The retained URL is live, and this guard owns one successful start.
            unsafe { self.url.stopAccessingSecurityScopedResource() };
        }
    }
}

/// Copy one picked file into a fresh managed directory and remember where it came from.
fn import_document(url: Retained<NSURL>, root: &Path) -> Result<Option<String>, String> {
    let access = SecurityAccess::new(url);
    let path = access.url.path().ok_or("The selected document has no file path")?.to_string();
    let name = file_name(&path)?;
    let bytes = coordinated_read(&access.url)?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err("Selected document exceeds the 512 MiB import limit".into());
    }
    let mut batch = ReservationBatch::default();
    let managed = batch.reserve(root, &name)?;
    remember_destination(&managed, &access.url)?;
    write_atomic(&managed, &bytes)?;
    batch.commit();
    Ok(Some(managed.to_str().ok_or("Non-Unicode managed document path")?.to_owned()))
}

fn coordinated_read(url: &NSURL) -> Result<Vec<u8>, String> {
    let outcome = RefCell::new(None);
    let reader = block2::RcBlock::new(|target: std::ptr::NonNull<NSURL>| {
        // SAFETY: Foundation guarantees a live immutable NSURL during the synchronous
        // coordination accessor; no pointer or borrowed Objective-C object escapes.
        let target = unsafe { target.as_ref() };
        let result = target.path().ok_or("The provider returned no local file path".into()).and_then(|path| bounded_read(Path::new(&path.to_string())));
        *outcome.borrow_mut() = Some(result);
    });
    let mut error: Option<Retained<NSError>> = None;
    NSFileCoordinator::new().coordinateReadingItemAtURL_options_error_byAccessor(url, NSFileCoordinatorReadingOptions::empty(), Some(&mut error), &reader);
    if let Some(error) = error {
        return Err(error.to_string());
    }
    drop(reader);
    outcome.into_inner().ok_or("The file provider did not allow reading")?
}

fn remember_destination(path: &Path, url: &Retained<NSURL>) -> Result<(), String> {
    let access = SecurityAccess::new(url.clone());
    // iOS bookmarks preserve implicit security scope; macOS-only WithSecurityScope is
    // deliberately omitted. The picker-provided URL remains retained for this operation.
    let data = access
        .url
        .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(NSURLBookmarkCreationOptions::MinimalBookmark, None, None)
        .map_err(|e| e.to_string())?;
    let bookmark = bookmark_path(path)?;
    std::fs::create_dir_all(bookmark.parent().ok_or("Missing bookmark metadata directory")?).map_err(|e| e.to_string())?;
    write_atomic(&bookmark, &data.to_vec())
}

fn resolve_destination(path: &Path) -> Result<Option<Retained<NSURL>>, String> {
    let bookmark = bookmark_path(path)?;
    if !bookmark.exists() {
        return Ok(None);
    }
    let data = NSData::from_vec(bounded_read_limit(&bookmark, 1024 * 1024)?);
    // SAFETY: NSData and NSURL are retained, no borrowed objects escape, and the optional
    // stale output is null (we refresh the bookmark after each successful external write).
    let url = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(&data, NSURLBookmarkResolutionOptions::WithoutUI, None, std::ptr::null_mut())
    }
    .map_err(|e| format!("The saved destination is unavailable: {e}. Use Save As to choose it again."))?;
    Ok(Some(url))
}

fn coordinated_write(url: Retained<NSURL>, bytes: &[u8]) -> Result<(), String> {
    let access = SecurityAccess::new(url);
    let data = NSData::with_bytes(bytes);
    let outcome = RefCell::new(None);
    let writer = block2::RcBlock::new(|target: std::ptr::NonNull<NSURL>| {
        // SAFETY: NSFileCoordinator supplies a valid NSURL for the duration of its
        // synchronous accessor. It is never retained or used after the accessor returns.
        let target = unsafe { target.as_ref() };
        let result = data.writeToURL_options_error(target, NSDataWritingOptions::Atomic).map_err(|e| e.to_string());
        *outcome.borrow_mut() = Some(result);
    });
    let mut error: Option<Retained<NSError>> = None;
    NSFileCoordinator::new().coordinateWritingItemAtURL_options_error_byAccessor(&access.url, NSFileCoordinatorWritingOptions::ForReplacing, Some(&mut error), &writer);
    if let Some(error) = error {
        return Err(error.to_string());
    }
    drop(writer);
    outcome.into_inner().ok_or("The file provider did not allow writing")?
}

/// The document types a picker offers: the extensions of the first filter, or any data when
/// none of them names a known type.
fn content_types(extensions: &[&str]) -> Retained<NSArray<UTType>> {
    let mut types: Vec<Retained<UTType>> = extensions.iter().filter_map(|extension| UTType::typeWithFilenameExtension(&NSString::from_str(extension))).collect();
    if types.is_empty() {
        types.extend(UTType::typeWithIdentifier(&NSString::from_str("public.data")));
    }
    NSArray::from_retained_slice(&types)
}

/// A future that answers once the picker's worker has copied the file (or the user cancelled).
struct AnswerFuture {
    answer: mpsc::Receiver<Option<String>>,
}

impl Future for AnswerFuture {
    type Output = Option<String>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<String>> {
        // The file-picker loop repaints while a pick is pending, so a poll per frame is enough;
        // the reply also asks for a repaint for the frame right after it.
        match self.answer.try_recv() {
            Ok(path) => Poll::Ready(path),
            Err(TryRecvError::Empty) => Poll::Pending,
            Err(TryRecvError::Disconnected) => Poll::Ready(None),
        }
    }
}

fn open_picker(root: PathBuf, ui: egui::Context, extensions: &[&str]) -> solidcraft_ui_egui::FilePickerFuture {
    let (answer, receiver) = mpsc::channel();
    let reply = Reply { answer, ui };
    let extensions: Vec<String> = extensions.iter().map(|extension| extension.to_string()).collect();
    dispatch2::DispatchQueue::main().exec_async(move || {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            fail_completion(Completion::Open { reply, root }, "UIKit main queue unavailable".into());
            return;
        };
        let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
        let types = content_types(&extensions);
        let picker = UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(UIDocumentPickerViewController::alloc(mtm), &types, false);
        picker.setAllowsMultipleSelection(false);
        show_picker(picker, Completion::Open { reply, root }, mtm);
    });
    Box::pin(AnswerFuture { answer: receiver })
}

/// Reserve a managed document for the save; the host publishes it once the bytes are written.
fn save_picker(root: PathBuf, suggested: &str) -> solidcraft_ui_egui::FilePickerFuture {
    let suggested = if suggested.is_empty() { "Untitled.sc" } else { suggested };
    let answer = match reserve_path(&root, suggested).and_then(|path| path.to_str().map(str::to_owned).ok_or_else(|| "Non-Unicode managed document path".to_string())) {
        Ok(path) => Some(path),
        Err(error) => {
            log::error!("Could not reserve a document for saving: {error}");
            None
        }
    };
    Box::pin(std::future::ready(answer))
}

/// Stage the saved file and let the user pick where it goes. Runs on the UI thread, so it only
/// copies when the file has no remembered destination yet.
fn stage_export(root: &Path, path: &Path) -> Result<(), String> {
    IN_FLIGHT.with(|export| *export.borrow_mut() = Some(path.to_owned()));
    let result = stage_export_copy(root, path);
    if result.is_err() {
        IN_FLIGHT.with(|export| export.borrow_mut().take());
    }
    result
}

fn stage_export_copy(root: &Path, path: &Path) -> Result<(), String> {
    let bytes = bounded_read(path)?;
    let stage_dir = root.join("exports");
    std::fs::create_dir_all(&stage_dir).map_err(|e| e.to_string())?;
    let name = file_name(path.to_str().ok_or("Non-Unicode document path")?)?;
    let stage = stage_dir.join(name);
    write_atomic(&stage, &bytes)?;
    let managed = path.to_owned();
    let picker_stage = stage.clone();
    dispatch2::DispatchQueue::main().exec_async(move || {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            log::error!("UIKit main queue unavailable; the export was not offered");
            fail_completion(Completion::Export { managed, stage: picker_stage }, "UIKit main queue unavailable".into());
            return;
        };
        let Some(stage_text) = picker_stage.to_str() else {
            log::error!("Non-Unicode export path; the export was not offered");
            fail_completion(Completion::Export { managed, stage: picker_stage }, "Non-Unicode export path".into());
            return;
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(stage_text));
        let urls = NSArray::from_retained_slice(&[url]);
        let picker = UIDocumentPickerViewController::initForExportingURLs_asCopy(UIDocumentPickerViewController::alloc(mtm), &urls, true);
        show_picker(picker, Completion::Export { managed, stage: picker_stage }, mtm);
    });
    Ok(())
}

/// Take a just-saved file to the user's destination: a remembered one is updated in place,
/// otherwise the export picker asks once.
pub fn publish(root: &Path, path: &str) -> Result<(), String> {
    let path = Path::new(path);
    validate_managed_path(root, path)?;
    if IN_FLIGHT.with(|export| export.borrow().as_deref() == Some(path)) {
        return Ok(());
    }
    match resolve_destination(path)? {
        Some(destination) => {
            let bytes = bounded_read(path)?;
            coordinated_write(destination.clone(), &bytes)?;
            remember_destination(path, &destination)
        }
        None => stage_export(root, path),
    }
}

fn ready_sender() -> mpsc::Sender<String> {
    READY.with(|ready| {
        let mut ready = ready.borrow_mut();
        let (sender, _) = ready.get_or_insert_with(mpsc::channel);
        sender.clone()
    })
}

/// Ask the app to open a managed design at the next frame (a recent design's click).
fn request_open(path: String) {
    let _ = ready_sender().send(path);
    if let Some(ui) = UI.with(|ui| ui.borrow().clone()) {
        ui.request_repaint();
    }
}

/// Copy a design the Files app opened into the managed store on a worker, then open the copy.
pub fn open_external(url: Retained<NSURL>) -> Result<(), String> {
    let root = support_directory()?;
    std::fs::create_dir_all(root.join("documents")).map_err(|error| error.to_string())?;
    std::thread::Builder::new()
        .name("solidcraft-document-import".into())
        .spawn(move || match import_document(url, &root) {
            Ok(Some(path)) => request_open(path),
            Ok(None) => log::error!("The design the Files app opened could not be imported"),
            Err(error) => log::error!("Could not import the design the Files app opened: {error}"),
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Open the designs that became ready since the last frame, as a picked file does.
pub fn open_ready(app: &mut SolveApp) {
    let paths: Vec<String> = READY.with(|ready| {
        let Ok(ready) = ready.try_borrow() else { return Vec::new() };
        let Some((_, receiver)) = ready.as_ref() else { return Vec::new() };
        std::iter::from_fn(|| receiver.try_recv().ok()).collect()
    });
    for path in paths {
        // Off the start page, as the file dialog's own pick leaves it.
        app.home.open = false;
        app.open_path_async(&path);
    }
}

/// Install the pickers, the publish step, the recent designs and the pasteboard. Every answer
/// wakes the UI, and the app opens what became ready when it next draws.
pub fn install(services: &mut Services, ui: egui::Context) -> Result<(), String> {
    let root = support_directory()?;
    std::fs::create_dir_all(root.join("documents")).map_err(|e| e.to_string())?;
    let open_root = root.clone();
    let open_ui = ui.clone();
    services.pick_open = Some(Box::new(move |extensions| open_picker(open_root.clone(), open_ui.clone(), extensions)));
    let save_root = root.clone();
    services.pick_save = Some(Box::new(move |name, _extensions| save_picker(save_root.clone(), name)));
    let publish_root = root;
    services.publish = Some(Box::new(move |path| publish(&publish_root, path)));
    // Designs live in the app's own store, so a recent one is there unless its copy was removed.
    services.can_restore = Some(Box::new(|path| Path::new(path).is_file()));
    services.restore_file = Some(Box::new(|path| request_open(path.to_owned())));
    // egui's clipboard only feeds this app; the pasteboard is the one other apps use.
    services.clipboard_get = Some(Box::new(crate::scenes::pasteboard_text));
    services.clipboard_set = Some(Box::new(crate::scenes::set_pasteboard_text));
    UI.with(|slot| *slot.borrow_mut() = Some(ui));
    Ok(())
}
