//! Opens links and folders with the desktop.
//!
//! Other platforms use the `open` crate. Windows uses `ShellExecuteW` because
//! the crate's PowerShell launcher can be unavailable (#107), while
//! `cmd /c start` expands percent-encoded URL segments as environment
//! variables. iOS opens nothing here: links go to UIKit on the UI thread
//! (`Action::OpenUrl`), and sign-in pages to a sheet over the app
//! (`src/ios/auth_sheet.rs`).

use std::ffi::OsStr;
use std::io;

/// Opens a URL or a path in whatever the desktop uses for it.
#[cfg(not(any(windows, target_os = "ios")))]
pub fn open(target: impl AsRef<OsStr>) -> io::Result<()> {
    open::that(target.as_ref())
}

/// An iOS app can neither start a launcher (the `open` crate's `uiopen`,
/// which the sandbox forbids) nor show a folder, so nothing is opened.
#[cfg(target_os = "ios")]
pub fn open(target: impl AsRef<OsStr>) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("iOS cannot open {}", target.as_ref().to_string_lossy()),
    ))
}

/// Opens a URL or a path in whatever the desktop uses for it.
#[cfg(windows)]
pub fn open(target: impl AsRef<OsStr>) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    // Shell extensions may require COM. Keep an existing apartment mode if
    // the worker thread was already initialized.
    unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };

    let file = wide(target.as_ref());
    let verb = wide(OsStr::new("open"));
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Anything above 32 opened. At or below it the value is one of the
    // old SE_ERR_ codes, which the thread's last error carries too.
    if result as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
