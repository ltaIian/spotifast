//! Spotifast's iOS app as a static library: the C functions the Xcode
//! project's Swift code calls (`ios/Sources/main.swift`), each handing over
//! to `spotifast::ios`. On any other target the library is empty.

#![cfg(target_os = "ios")]

/// Runs Spotifast: UIKit's main loop, with the app in its window. Call it
/// once, on the main thread, once the audio session is set up. It never
/// returns.
#[unsafe(no_mangle)]
pub extern "C" fn spotifast_main() -> ! {
    spotifast::ios::run()
}

/// Saves what quitting would save. Call it when the app enters the
/// background, where iOS may suspend it and end it without warning.
#[unsafe(no_mangle)]
pub extern "C" fn spotifast_will_background() {
    spotifast::ios::will_background();
}

/// Saves what quitting would save. Call it when the app is about to
/// terminate.
#[unsafe(no_mangle)]
pub extern "C" fn spotifast_will_terminate() {
    spotifast::ios::will_terminate();
}

/// The person closed the sign-in sheet: the sign-in it showed is given up.
/// Call it on the main thread.
#[unsafe(no_mangle)]
pub extern "C" fn spotifast_auth_sheet_closed() {
    spotifast::ios::auth_sheet::closed();
}
