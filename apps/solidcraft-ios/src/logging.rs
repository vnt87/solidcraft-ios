//! App-container logging. Unavailable logging never blocks launch.
#[cfg(target_os = "ios")]
use std::io::Write;
#[cfg(target_os = "ios")]
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "ios")]
struct Logger(Mutex<std::fs::File>);

#[cfg(target_os = "ios")]
impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let mut file = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Err(error) = writeln!(file, "{} {}: {}", record.level(), record.target(), record.args()) {
                eprintln!("SolidCraft log write failed: {error}");
            }
        }
    }

    fn flush(&self) {
        let mut file = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(error) = file.flush() {
            eprintln!("SolidCraft log flush failed: {error}");
        }
    }
}

#[cfg(target_os = "ios")]
static LOGGER: OnceLock<Logger> = OnceLock::new();

#[cfg(target_os = "ios")]
pub(super) fn install() -> Result<(), String> {
    let home = std::env::var_os("HOME");
    let directory = log_directory(home.as_deref())?;
    std::fs::create_dir_all(&directory).map_err(|error| format!("create log directory: {error}"))?;
    let file = std::fs::OpenOptions::new().create(true).append(true).open(directory.join("runner.log")).map_err(|error| format!("open runner log: {error}"))?;
    let logger = LOGGER.get_or_init(|| Logger(Mutex::new(file)));
    log::set_logger(logger).map_err(|error| format!("install logger: {error}"))?;
    log::set_max_level(log::LevelFilter::Info);
    Ok(())
}

/// `Library/Application Support/io.tangencylabs.solidcraft/logs` inside the app container.
fn log_directory(home: Option<&std::ffi::OsStr>) -> Result<std::path::PathBuf, String> {
    let home = home.filter(|home| !home.is_empty()).ok_or("iOS app container HOME is unavailable")?;
    Ok(std::path::PathBuf::from(home).join("Library/Application Support/io.tangencylabs.solidcraft/logs"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn log_directory_stays_inside_the_supplied_app_container() {
        let home = std::ffi::OsStr::new("/private/app-container");
        assert_eq!(
            super::log_directory(Some(home)).ok(),
            Some(std::path::PathBuf::from("/private/app-container/Library/Application Support/io.tangencylabs.solidcraft/logs"))
        );
    }

    #[test]
    fn missing_or_empty_container_is_an_error() {
        assert!(super::log_directory(None).is_err());
        assert!(super::log_directory(Some(std::ffi::OsStr::new(""))).is_err());
    }
}
