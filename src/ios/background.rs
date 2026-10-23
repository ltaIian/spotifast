//! The app's logic while iOS has it in the background.
//!
//! On the desktop, fastframe-shell keeps calling the app without a window
//! (`Resident::headless_frame`): playback state, the queue and the media
//! controls stay current, at least every `HEADLESS_SLEEP`. On iOS the window
//! stays, but UIKit draws nothing in the background, so eframe never gets a
//! frame to paint. eframe still runs the window's logic alone when a frame
//! it asked for is overdue, which keeps the app going whenever something
//! wakes it. In case that stops, `ios/Sources/main.swift` ticks a timer on
//! the main thread while the app is in the background, and [`tick`] runs one
//! logic pass on a context without a window, as the desktop's headless loop
//! does, whenever no pass has run for two seconds.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The longest the app goes without a logic pass in the background, as on
/// the desktop without a window.
const LONGEST_GAP: Duration = Duration::from_secs(2);

thread_local! {
    /// When the last logic pass ran, the window's or the timer's.
    static LAST_PASS: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The context the timer's passes run on, made at the first one.
    static HEADLESS: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// Whether the app is in the background now.
static IN_BACKGROUND: AtomicBool = AtomicBool::new(false);
/// Logic passes eframe ran for the window, the timer's ticks, and the
/// passes those ran, for the self-test.
pub static WINDOW_PASSES: AtomicU64 = AtomicU64::new(0);
pub static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
pub static TIMER_PASSES: AtomicU64 = AtomicU64::new(0);

/// Whether the app is in the background now.
pub fn in_background() -> bool {
    IN_BACKGROUND.load(Ordering::Relaxed)
}

/// The app went to the background, or came back.
pub fn set_in_background(background: bool) {
    IN_BACKGROUND.store(background, Ordering::Relaxed);
}

/// eframe ran the window's logic.
pub fn window_pass() {
    WINDOW_PASSES.fetch_add(1, Ordering::Relaxed);
    LAST_PASS.set(Some(Instant::now()));
}

/// The background timer's tick: a logic pass, unless one ran lately.
pub fn tick() {
    TIMER_TICKS.fetch_add(1, Ordering::Relaxed);
    if LAST_PASS.get().is_some_and(|at| at.elapsed() < LONGEST_GAP) {
        return;
    }
    super::with_app("background tick", |app| {
        let ctx = HEADLESS.with_borrow_mut(|headless| {
            headless.get_or_insert_with(egui::Context::default).clone()
        });
        // One pass on a context nothing paints: what it asks for (a
        // repaint, a texture) is let go when it ends, as on the desktop.
        ctx.begin_pass(egui::RawInput::default());
        app.background_frame(&ctx);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        TIMER_PASSES.fetch_add(1, Ordering::Relaxed);
    });
    LAST_PASS.set(Some(Instant::now()));
}
