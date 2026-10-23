//! The title bar the app draws itself, from fastframe-titlebar.
//!
//! fastframe-titlebar brings eframe with its OpenGL renderer, which does not
//! build for iOS, so the iOS build leaves it out. An iOS window fills the
//! screen and has no title bar to move, double-click or resize: there these
//! are stand-ins with the same signatures that do nothing, and the caption
//! buttons take no room.

#[cfg(not(target_os = "ios"))]
pub use fastframe_titlebar::{
    Footprint, Labels, Reserve, Style, controls, drag, enabled, footprint, init, install, resize,
    set_enabled,
};

#[cfg(target_os = "ios")]
pub use inert::{
    Footprint, Labels, Reserve, Style, controls, drag, enabled, footprint, init, install, resize,
    set_enabled,
};

/// fastframe-titlebar's interface, doing nothing.
#[cfg(target_os = "ios")]
mod inert {
    use egui::Rect;

    /// How the caption buttons would look.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Style {
        pub icon: egui::Color32,
        pub button: egui::Color32,
        pub header_height: f32,
    }

    /// The caption buttons' names, in the app's language.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Labels<'a> {
        pub minimize: &'a str,
        pub maximize: &'a str,
        pub restore: &'a str,
        pub close: &'a str,
    }

    /// The room the caption buttons take at each end of the title bar.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Footprint {
        pub left: f32,
        pub right: f32,
        pub height: f32,
    }

    /// Where a header puts its own controls beside the buttons.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Reserve {
        pub inline_width: f32,
        pub push_down: f32,
    }

    impl Footprint {
        /// Where a header `width` wide, whose own controls need
        /// `min_content`, goes beside the right-hand buttons, as on the
        /// desktop.
        pub fn reserve(&self, width: f32, min_content: f32) -> Reserve {
            if self.right <= 0.0 {
                Reserve::default()
            } else if width < min_content + self.right {
                Reserve {
                    inline_width: 0.0,
                    push_down: self.height,
                }
            } else {
                Reserve {
                    inline_width: self.right,
                    push_down: 0.0,
                }
            }
        }
    }

    pub fn init() {}

    pub fn set_enabled(_on: bool) {}

    /// The app never draws a title bar on iOS.
    pub fn enabled() -> bool {
        false
    }

    pub fn install(_ctx: &egui::Context) {}

    /// No caption buttons, so no room for them.
    pub fn footprint(_ctx: &egui::Context, _header_height: f32) -> Footprint {
        Footprint::default()
    }

    pub fn drag(_ui: &mut egui::Ui, _rect: Rect) {}

    pub fn controls(_ui: &mut egui::Ui, _style: &Style, _labels: &Labels<'_>) {}

    pub fn resize(_ui: &mut egui::Ui) {}
}
