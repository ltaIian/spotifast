//! What the app uses of fastframe-shell outside the desktop's run loop.
//!
//! fastframe-shell keeps the app running between windows on the desktop
//! (`entrypoint.rs`). It brings eframe with its OpenGL renderer, which does
//! not build for iOS, and an iOS app has one window for as long as it runs
//! (`src/ios`), so the iOS build leaves it out. There the waker repaints that
//! one window, and the window's remembered geometry and zoom do nothing: an
//! iOS window always fills the screen.

#[cfg(not(target_os = "ios"))]
pub use fastframe_shell::{Waker, window};

#[cfg(target_os = "ios")]
pub use one_window::{Waker, window};

#[cfg(target_os = "ios")]
mod one_window {
    use std::sync::{Arc, Mutex, PoisonError};

    /// Repaints the window from any thread, and does nothing until the
    /// window exists.
    #[derive(Clone, Default)]
    pub struct Waker {
        window: Arc<Mutex<Option<egui::Context>>>,
    }

    impl std::fmt::Debug for Waker {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Waker")
                .field("attached", &self.context().is_some())
                .finish()
        }
    }

    impl Waker {
        /// Repaints `ctx` from now on: the window's context, once eframe has
        /// made it.
        pub fn attach(&self, ctx: &egui::Context) {
            *self.window.lock().unwrap_or_else(PoisonError::into_inner) = Some(ctx.clone());
        }

        /// Asks the window, if there is one yet, for a new frame.
        pub fn wake(&self) {
            if let Some(ctx) = self.context() {
                ctx.request_repaint();
            }
        }

        fn context(&self) -> Option<egui::Context> {
            self.window
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    pub mod window {
        /// The desktop window's size, position and maximized state, kept in
        /// the session file. Nothing on iOS reads or changes it.
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        pub struct Geometry {
            pub size: Option<[f32; 2]>,
            pub position: Option<[f32; 2]>,
            pub maximized: bool,
        }

        impl Geometry {
            /// An iOS window always fills the screen, so there is nothing to
            /// remember and nothing to save.
            pub fn remember(&mut self, _ctx: &egui::Context) -> bool {
                false
            }
        }

        /// The interface zoom stays egui's own: no saved zoom is applied,
        /// and none is saved.
        pub fn keep_zoom(_ctx: &egui::Context, _saved: f32) -> Option<f32> {
            None
        }

        /// There is no compositor to ask to raise the window.
        pub fn raise_on_compositor() -> bool {
            false
        }
    }
}
