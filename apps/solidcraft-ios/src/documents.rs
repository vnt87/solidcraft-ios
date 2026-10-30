//! Managed document identities inside the app container and the asynchronous UIKit pickers.
//!
//! iPadOS hands an app a security-scoped URL, not a lasting path: everything opened (through the
//! picker or from the Files app) is copied into `documents/`, and a save lands in a reserved
//! managed path whose directory also remembers where the user keeps the file. Recent designs
//! reopen from that store. Only a folder this batch just created is ever removed.
#![cfg_attr(not(target_os = "ios"), allow(dead_code))]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DOCUMENT: AtomicU64 = AtomicU64::new(0);
/// A design or mesh larger than this is refused instead of read into memory.
pub const MAX_DOCUMENT_BYTES: u64 = 512 * 1024 * 1024;

/// The app's own folder in `Library/Application Support`, the one `app.ron` lives in.
pub fn support_directory() -> Result<PathBuf, String> {
    support_directory_from(std::env::var_os("HOME"))
}

fn support_directory_from(home: Option<std::ffi::OsString>) -> Result<PathBuf, String> {
    let home = home.filter(|home| !home.is_empty()).ok_or("iOS app container HOME is unavailable")?;
    let home = PathBuf::from(home);
    if !home.is_absolute() || home == Path::new("/") || home.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("iOS app container HOME is not an absolute directory".into());
    }
    Ok(home.join("Library/Application Support/io.tangencylabs.solidcraft"))
}

/// Keep the original extension (it selects the encoder), but never accept directories.
pub fn file_name(suggested: &str) -> Result<String, String> {
    let name = Path::new(suggested).file_name().and_then(|s| s.to_str()).filter(|s| !s.is_empty()).ok_or("The document needs a file name")?;
    if matches!(name, "." | "..") || name.contains('\0') {
        return Err("Invalid document name".into());
    }
    Ok(name.to_owned())
}

/// A fresh directory per saved or imported document, so files never share an identity.
pub fn reserve_path(root: &Path, suggested: &str) -> Result<PathBuf, String> {
    let name = file_name(suggested)?;
    let documents = root.join("documents");
    std::fs::create_dir_all(&documents).map_err(|e| e.to_string())?;
    for _ in 0..128 {
        let sequence = NEXT_DOCUMENT.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
        let dir = documents.join(format!("{nanos}-{sequence}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir.join(name)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Could not reserve a document identity".into())
}

/// Imported files are ready together; a failed import removes the directories it made.
#[derive(Default)]
pub struct ReservationBatch {
    directories: Vec<PathBuf>,
    committed: bool,
}

impl ReservationBatch {
    pub fn reserve(&mut self, root: &Path, suggested: &str) -> Result<PathBuf, String> {
        let path = reserve_path(root, suggested)?;
        if let Some(parent) = path.parent() {
            self.directories.push(parent.to_owned());
        }
        Ok(path)
    }

    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for ReservationBatch {
    fn drop(&mut self) {
        if !self.committed {
            for directory in &self.directories {
                // These exact fresh directories belong to this batch, never preexisting files.
                let _ = std::fs::remove_dir_all(directory);
            }
        }
    }
}

pub fn bounded_read(path: &Path) -> Result<Vec<u8>, String> {
    bounded_read_limit(path, MAX_DOCUMENT_BYTES)
}

pub fn bounded_read_limit(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > limit {
        return Err("File exceeds the allowed size".into());
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("File exceeds the allowed size".into());
    }
    Ok(bytes)
}

/// Only a file directly inside a reserved `documents/<batch>/` may be published.
pub fn validate_managed_path(root: &Path, path: &Path) -> Result<(), String> {
    let documents = std::fs::canonicalize(root.join("documents")).map_err(|e| e.to_string())?;
    let parent = path.parent().ok_or("Missing managed document directory")?;
    let parent = std::fs::canonicalize(parent).map_err(|e| e.to_string())?;
    if parent.parent() != Some(documents.as_path()) || path.file_name().is_none() || path.is_symlink() {
        return Err("Use Save As to select an iPadOS document destination".into());
    }
    Ok(())
}

/// Where the destination bookmark for a managed document is kept.
pub fn bookmark_path(path: &Path) -> Result<PathBuf, String> {
    Ok(path.parent().ok_or("Missing managed document directory")?.join("metadata").join("destination.bookmark"))
}

/// Copy a file into place with a temporary name and one rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("{}.tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("part")));
    std::fs::write(&temporary, bytes).map_err(|e| format!("{}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("{}: {e}", path.display())
    })
}

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod native;
#[cfg(target_os = "ios")]
pub use native::{install, open_external, open_ready};

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("solidcraft-ios-documents-{name}-{}", std::process::id()))
    }

    #[test]
    fn names_keep_the_encoder_extension_and_strip_directories() {
        assert_eq!(file_name("/external/保存.step").expect("name"), "保存.step");
        assert!(file_name("/").is_err());
        assert!(file_name("../").is_err());
        assert!(file_name("bad\0.sc").is_err());
    }

    #[test]
    fn reservations_never_share_an_identity() {
        let root = scratch("reservation");
        let first = reserve_path(&root, "part.sc").expect("first reservation");
        let second = reserve_path(&root, "part.sc").expect("second reservation");
        assert_ne!(first, second);
        assert!(!first.exists());
        assert!(first.parent() != second.parent());
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn oversized_files_are_refused_without_allocating_them() {
        let root = scratch("limit");
        let path = reserve_path(&root, "large.sc").expect("reservation");
        std::fs::File::create(&path).expect("sparse fixture").set_len(MAX_DOCUMENT_BYTES + 1).expect("sparse length");
        assert!(bounded_read(&path).is_err());
        assert!(validate_managed_path(&root, &path).is_ok());
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn only_managed_documents_may_be_published() {
        let root = scratch("managed");
        let path = reserve_path(&root, "part.sc").expect("reservation");
        assert!(validate_managed_path(&root, &path).is_ok());
        assert!(validate_managed_path(&root, &root.join("documents/../prefs.json")).is_err());
        assert!(validate_managed_path(&root, &root.join("app.ron")).is_err());
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn a_failed_batch_removes_only_the_directories_it_made() {
        let root = scratch("rollback");
        std::fs::create_dir_all(root.join("documents")).expect("documents");
        let kept = reserve_path(&root, "kept.sc").expect("kept");
        std::fs::write(&kept, b"keep me").expect("kept bytes");
        {
            let mut batch = ReservationBatch::default();
            let first = batch.reserve(&root, "first.sc").expect("first");
            std::fs::write(&first, b"discard me").expect("first bytes");
            let second = batch.reserve(&root, "second.sc").expect("second");
            assert!(first.is_file() && !second.exists());
        }
        assert!(kept.is_file(), "an earlier document must survive a failed batch");
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn atomic_writes_replace_the_destination_and_leave_no_temporary() {
        let root = scratch("atomic");
        std::fs::create_dir_all(&root).expect("dir");
        let path = root.join("part.sc");
        write_atomic(&path, b"first").expect("first write");
        write_atomic(&path, b"second").expect("second write");
        assert_eq!(std::fs::read(&path).expect("read"), b"second");
        assert!(!root.join("part.tmp").exists());
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn bookmark_metadata_lives_beside_the_document() {
        let root = scratch("bookmark");
        let path = reserve_path(&root, "part.sc").expect("reservation");
        assert_eq!(bookmark_path(&path).ok(), path.parent().map(|parent| parent.join("metadata/destination.bookmark")));
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn the_support_folder_is_derived_only_from_the_app_container() {
        let container = std::ffi::OsStr::new("/private/app-container");
        assert_eq!(
            support_directory_from(Some(container.to_owned())).ok(),
            Some(PathBuf::from("/private/app-container/Library/Application Support/io.tangencylabs.solidcraft"))
        );
        assert!(support_directory_from(None).is_err());
        assert!(support_directory_from(Some(std::ffi::OsString::new())).is_err());
        assert!(support_directory_from(Some(std::ffi::OsString::from("relative"))).is_err());
        assert!(support_directory_from(Some(std::ffi::OsString::from("/"))).is_err());
        assert!(support_directory_from(Some(std::ffi::OsString::from("/app/../elsewhere"))).is_err());
    }
}
