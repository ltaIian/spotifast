//! The iOS app: what the Xcode project's Swift code starts and calls back.
//!
//! winit's iOS event loop calls `UIApplicationMain` itself and refuses to
//! start once a `UIApplication` exists, so the app has no delegate of its
//! own: `ios/Sources/main.swift` sets up the audio session, asks to hear when
//! the app goes to the background, and calls [`run`], which never returns.
//! `run` does what the desktop's entry (`src/entrypoint.rs`) and
//! fastframe-shell do, for one window that lasts as long as the app.

pub mod auth_sheet;
mod background;
pub mod now_playing;
mod selftest;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::app::{App, AppOptions};
use crate::backend::Waker;
use crate::model::Action;
use crate::paths::AppDirs;
use crate::settings::Settings;

/// The log's default filter, as on the desktop.
const LOG_FILTER: &str = "warn,spotifast=info,fastframe_fonts=info";

thread_local! {
    /// The app, for the hooks Swift calls on the main thread while UIKit
    /// runs the window: `None` until the window exists.
    static APP: RefCell<Option<Rc<RefCell<App>>>> = const { RefCell::new(None) };
    /// The app's waker, for hooks that leave the app work to do.
    static WAKER: RefCell<Option<Waker>> = const { RefCell::new(None) };
}

/// Starts Spotifast: the log, the settings and the app, then UIKit's main
/// loop with the app in its one window. Call it once, on the main thread.
/// It never returns.
pub fn run() -> ! {
    // Before any output is listed or opened.
    crate::sink::name_in_mixers();
    let folders = AppDirs::discover();
    let dirs = folders.dirs.clone();
    let dirs_ready = dirs.ensure();
    // The log and the panic log go in the app's own container, where Xcode
    // or a sideloading tool can fetch them for a bug report.
    if let Err(error) = fastframe_log::Logging::new("spotifast", env!("CARGO_PKG_VERSION"))
        .filter(LOG_FILTER)
        .file(dirs.log_file())
        .panic_log(dirs.panic_log())
        .panic_message(fastframe_log::PanicMessage::Redacted(
            fastframe_log::redact::links,
        ))
        .init()
    {
        eprintln!("not logging: {error}");
    }
    folders.log();
    if let Err(error) = dirs_ready {
        log::warn!("unable to create the application directories: {error}");
    }
    let self_test = selftest::requested();
    match self_test {
        Some(selftest::Mode::Diagnostics) => selftest::start(),
        Some(selftest::Mode::Background) => selftest::start_background(),
        _ => {}
    }
    let mut settings = Settings::load(&dirs.settings_file());
    keep_to_one_window(&mut settings);
    crate::emoji::install(false);
    let storage = eframe_storage(&dirs);
    let waker = Waker::default();
    // The media controls are Now Playing and the remote commands, through
    // Swift (`now_playing`).
    let options = AppOptions {
        restore_sign_in: true,
        media_controls: true,
        tray: false,
        autostart: false,
    };
    let mut app = App::new(&waker, dirs, settings, options);
    app.load_custom_themes(&waker);
    WAKER.with(|slot| *slot.borrow_mut() = Some(waker.clone()));
    // The window is the screen: the app never asks it to grow.
    crate::window::set_fixed_size(true);
    let app = Rc::new(RefCell::new(app));
    let creator: eframe::AppCreator<'_> = Box::new(move |cc| {
        // The renderer actually chosen, as the desktop logs its OpenGL one.
        if let Some(render) = &cc.wgpu_render_state {
            let info = render.adapter.get_info();
            log::info!("wgpu adapter: {}, {:?}", info.name, info.backend);
        }
        waker.attach(&cc.egui_ctx);
        app.borrow_mut().attach(&cc.egui_ctx);
        APP.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&app)));
        let diagnostics = self_test == Some(selftest::Mode::Diagnostics);
        Ok(Box::new(Window {
            app,
            insets: diagnostics.then(selftest::Insets::default),
            sign_in: self_test.and_then(selftest::SignIn::for_mode),
        }))
    });
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Spotifast"),
        persistence_path: Some(storage),
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    // From here this thread draws the window: a durable write on it is
    // caught, as on the desktop.
    fastframe_file::mark_ui_thread();
    let result = eframe::run_native("Spotifast", options, creator);
    // UIKit's main loop never returns, so this is a failure to start it.
    match result {
        Ok(()) => log::error!("the event loop returned"),
        Err(error) => log::error!("Native window failed: {error}"),
    }
    std::process::exit(1)
}

/// The app has gone to the background, where iOS may suspend it at any
/// moment and end it without warning: save what quitting would. A sign-in
/// waiting on its sheet is given up: a suspended app cannot answer the
/// redirect, and its listener would hold the port. Until it comes back,
/// the app runs as the desktop does without a window: nobody sees it.
pub fn will_background() {
    background::set_in_background(true);
    with_app("background", |app| {
        auth_sheet::give_up(app);
        save(app);
        app.window_gone();
    });
}

/// The app is coming back to the foreground: its window is seen again.
pub fn will_foreground() {
    background::set_in_background(false);
    with_app("foreground", |app| app.window_hidden = false);
    wake();
}

/// The timer `main.swift` runs while the app is in the background: a logic
/// pass when none ran lately (`background`).
pub fn background_tick() {
    background::tick();
}

/// The app is about to end: save what quitting would. The process ends right
/// after, with playback and the network, so nothing waits for them.
pub fn will_terminate() {
    with_app("terminate", save);
}

/// Saves the settings and the session, and waits a bounded time for the
/// files to reach the disk, as the desktop does when its window closes.
fn save(app: &mut App) {
    app.save_state();
    app.flush_files();
}

/// Asks the window for a pass, to apply what a hook left for it.
fn wake() {
    WAKER.with(|slot| {
        if let Some(waker) = slot.borrow().as_ref() {
            waker.wake();
        }
    });
}

/// Runs `act` on the app, if its window exists and is not drawing.
fn with_app(when: &str, act: impl FnOnce(&mut App)) {
    APP.with(|slot| {
        let Some(app) = slot.borrow().clone() else {
            log::info!("{when}: no window yet, nothing to save");
            return;
        };
        match app.try_borrow_mut() {
            Ok(mut app) => act(&mut app),
            Err(_) => log::warn!("{when}: the window is busy, nothing saved"),
        }
    });
}

/// Whether `action` belongs to the desktop alone, and does nothing on iOS.
/// An iOS app keeps one window, which the person leaves rather than closes:
/// quitting or closing it would end the app, and the Winamp mini player and
/// MilkDrop are windows of their own. There is no title bar, no login item
/// and no folder to show, and a sideloaded app cannot update itself.
pub fn inert(action: &Action) -> bool {
    matches!(
        action,
        Action::Quit
            | Action::CloseWindow
            | Action::HideWindow
            | Action::ToggleWinampWindow
            | Action::ToggleWinampOnTop
            | Action::SetWinampTaskbar(_)
            | Action::SetCustomTitlebar(_)
            | Action::ToggleWinampMilkdrop
            | Action::DownloadMilkdropPack(_)
            | Action::SetStartWithComputer(_)
            | Action::CheckForUpdates
            | Action::ShowUpdate
            | Action::DownloadUpdate
            | Action::InstallUpdate
            | Action::OpenThemesFolder
            | Action::OpenSkinsFolder
            | Action::OpenLocalFilesFolder
            | Action::OpenMilkdropFolder
    )
}

/// The settings that describe the desktop's other windows and its updates,
/// held off for this run in memory only: an iOS app has one window, and a
/// sideloaded one cannot replace itself. They are saved as they are only
/// when something else changes.
fn keep_to_one_window(settings: &mut Settings) {
    settings.winamp_window = false;
    settings.milkdrop_open = false;
    settings.check_for_updates = false;
    settings.download_updates_automatically = false;
}

/// Where eframe keeps its own state: where it would on macOS, as it finds no
/// folder of its own on iOS.
fn eframe_storage(dirs: &AppDirs) -> PathBuf {
    match std::env::home_dir() {
        Some(home) => home.join("Library/Application Support/spotifast/app.ron"),
        None => dirs.config.join("app.ron"),
    }
}

/// eframe's wgpu setup, with a device that asks only for what every iPhone
/// and the simulator offer: egui's default limits ask for 16 inter-stage
/// shader variables, the simulator's Metal has 15, and the window then
/// fails to open.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.device_descriptor = Arc::new(device_descriptor);
    eframe::egui_wgpu::WgpuConfiguration {
        wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

/// egui's device with wgpu's downlevel limits, which any GPU that runs
/// OpenGL ES 3.1 meets, and textures as large as egui's own default allows.
fn device_descriptor(_adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
    wgpu::DeviceDescriptor {
        label: Some("egui wgpu device"),
        required_limits: wgpu::Limits {
            max_texture_dimension_2d: 8192,
            ..wgpu::Limits::downlevel_defaults()
        },
        ..Default::default()
    }
}

/// eframe's side of the one window: each frame goes to the app, which lasts
/// as long as the process.
struct Window {
    app: Rc<RefCell<App>>,
    /// What the self-test reports of the window, when it runs.
    insets: Option<selftest::Insets>,
    /// The sign-in the self-test starts by itself, when it asks for one.
    sign_in: Option<selftest::SignIn>,
}

impl eframe::App for Window {
    /// egui's memory stays out of the saved state, as on the desktop: its
    /// options carry a zoom, and the app keeps its own.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        background::window_pass();
        if let Some(insets) = &mut self.insets {
            insets.report(ctx);
        }
        let mut app = self.app.borrow_mut();
        if let Some(sign_in) = &mut self.sign_in {
            sign_in.poll(&mut app);
        }
        app.background_frame(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.borrow_mut().frame_ui(ui);
    }

    /// iOS ends an app without calling this; [`will_background`] saves
    /// instead.
    fn on_exit(&mut self) {
        save(&mut self.app.borrow_mut());
    }
}
