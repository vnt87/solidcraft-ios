//! The only exported entry symbols; compiled solely for Apple's iOS ABI.

/// Xcode links this symbol as its executable entry point.
// SAFETY: The unique `main` symbol belongs to this static library; the harness supplies no
// other main. UIKit invokes it on the main thread. argc/argv are intentionally never read,
// dereferenced, retained, or freed, so their ownership and validity remain with the caller.
#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: i32, _argv: *const *const std::ffi::c_char) -> i32 {
    solidcraft_ios_main()
}

/// Start the safe runner without allowing an unwinding panic to cross the C ABI.
// SAFETY: The unique symbol is the harness's SolidCraft entry, called on the main thread before
// any UIKit event loop exists. It takes no borrowed pointers or Objective-C objects.
#[unsafe(no_mangle)]
pub extern "C" fn solidcraft_ios_main() -> i32 {
    match std::panic::catch_unwind(super::run) {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            log::error!("{error}");
            eprintln!("{error}");
            1
        }
        Err(_) => {
            log::error!("SolidCraft iOS startup panicked");
            eprintln!("SolidCraft iOS startup panicked");
            1
        }
    }
}
