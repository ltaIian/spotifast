//! Spotify's sign-in pages, shown in a Safari sheet over the app
//! (`ios/Sources/AuthSheet.swift`).
//!
//! Both of Spotifast's client ids redirect to a loopback address, where
//! `crate::auth` waits for the code. Safari opened as an app of its own
//! would send Spotifast to the background, where iOS suspends it before the
//! redirect comes; a `SFSafariViewController` over the app keeps it in the
//! foreground, and the sign-in itself is the desktop's. The sheet does not
//! share Safari's cookies, so the person signs in to Spotify there once.

use std::ffi::{CString, c_char};

use crate::app::App;
use crate::backend::{AuthStatus, Command, LocalPlayback};
use crate::model::Action;

unsafe extern "C" {
    /// Shows `url` in the sheet, in place of one already shown. Any thread.
    fn spotifast_ios_open_auth_url(url: *const c_char);
    /// Takes the sheet away, if one is shown. Any thread.
    safe fn spotifast_ios_auth_finished();
}

/// Shows a sign-in page.
pub fn open(url: &str) {
    let Ok(url) = CString::new(url) else {
        log::warn!("sign-in: the authorization address holds a NUL");
        return;
    };
    log::info!("sign-in: showing Spotify's authorization page in a sheet");
    // SAFETY: a valid C string, which the Swift side copies before it
    // returns.
    unsafe { spotifast_ios_open_auth_url(url.as_ptr()) }
}

/// The browser part of a sign-in is over, whichever way it went: the sheet
/// goes.
pub fn finished() {
    spotifast_ios_auth_finished();
}

/// The person closed the sheet: the sign-in or the playback authorization
/// it showed is given up, as its Cancel would.
pub fn closed() {
    log::info!("sign-in: the sheet was closed");
    super::with_app("sign-in sheet closed", give_up);
}

/// Gives up the web sign-in, or the playback authorization, that waits on
/// the browser. The backend then takes the sheet away, if it is still up.
pub(super) fn give_up(app: &mut App) {
    if matches!(app.auth, AuthStatus::WaitingForBrowser { .. }) {
        app.actions.push(Action::CancelSignIn);
        super::wake();
    } else if matches!(app.local_playback, LocalPlayback::Authorizing) {
        app.backend.send(Command::CancelPlaybackAuthorization);
    }
}
