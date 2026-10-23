//! Spotifast desktop command.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "ios"))]
mod entrypoint;

#[cfg(not(target_os = "ios"))]
fn main() -> eframe::Result<()> {
    entrypoint::run()
}

/// The iOS app has no command: the Xcode project's app starts in
/// `spotifast_main`, from the spotifast-ios static library (`src/ios`).
#[cfg(target_os = "ios")]
fn main() {}
