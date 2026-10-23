//! The equalizer as a page of its own: the phone layout's, where the Winamp
//! skin and its equalizer window are not available. The same controls as
//! the Settings section, laid out for a narrow screen and a finger.

use egui::{Align, Layout, vec2};

use crate::app::App;
use crate::i18n::gettext;
use crate::model::Action;
use crate::theme;

use super::settings::{self, eq_bar_slider, eq_curve, eq_slider, eq_slider_width, hertz};
use super::widgets;

/// The gap between the columns of band sliders.
const BAND_GAP: f32 = 8.0;
/// The room a band's column takes in its grid, its gap included, before
/// the ten of them fit on one row.
const BAND_ROOM: f32 = 72.0;

#[cfg(feature = "demo")]
thread_local! {
    /// `--demo-show equalizer-end`: the equalizer page stays scrolled to its
    /// end, for screenshots of its bands.
    static DEMO_END: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Keeps the equalizer page scrolled to its end on this thread's windows.
#[cfg(feature = "demo")]
pub(crate) fn demo_scroll_to_end() {
    DEMO_END.with(|scroll| scroll.set(true));
}

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let title = gettext(locale, "Equalizer");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        theme::text(ui, title.as_ref(), theme::bold(28.0), palette.text);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let mut on = app.settings.eq_on;
            if widgets::switch(ui, &palette, &title, &mut on).changed() {
                app.actions.push(Action::ToggleEq);
            }
        });
    });
    ui.add_space(12.0);
    // One row of chips that scrolls sideways, so the curve and the bands
    // stay in view.
    let current = crate::autoscroll::show(
        ui,
        egui::ScrollArea::horizontal().id_salt("equalizer-presets"),
        egui::Vec2b::new(true, false),
        |ui| settings::eq_preset_chips(app, ui, true),
    )
    .inner;
    ui.add_space(8.0);
    settings::eq_preset_controls(app, ui, current.as_deref());
    ui.add_space(12.0);
    eq_curve(ui, &palette, &crate::app::eq_settings(&app.settings));
    ui.add_space(16.0);
    let on = app.settings.eq_on;
    bands_and_preamp(app, ui, on);
    ui.add_space(12.0);
    let width = ui.available_width();
    let mut boost = app.settings.eq_bass_boost_db;
    if eq_bar_slider(
        ui,
        &palette,
        &gettext(locale, "Bass boost"),
        &mut boost,
        0.0..=crate::eq::BASS_BOOST_MAX_DB,
        on,
        width,
    ) {
        app.actions.push(Action::SetEqBassBoost(boost));
    }
    #[cfg(feature = "demo")]
    if DEMO_END.with(std::cell::Cell::get) {
        ui.scroll_to_cursor(Some(Align::Max));
    }
}

/// The ten bands in a grid, then the preamp as a bar across the width: how
/// a phone shows the equalizer's row of eleven sliders, here and in
/// Settings. `on` dims them while the equalizer is off.
pub(super) fn bands_and_preamp(app: &mut App, ui: &mut egui::Ui, on: bool) {
    band_grid(app, ui, on);
    ui.add_space(12.0);
    let palette = app.palette;
    let width = ui.available_width();
    let range = crate::eq::RANGE_DB;
    let mut preamp = app.settings.eq_preamp_db;
    if eq_bar_slider(
        ui,
        &palette,
        &gettext(app.locale, "Preamp"),
        &mut preamp,
        -range..=range,
        on,
        width,
    ) {
        app.actions.push(Action::SetEqPreamp(preamp));
    }
}

/// The ten bands in a grid of equal columns: one row where the width holds
/// them all, two rows of five on a phone. No column is narrower than a
/// finger.
fn band_grid(app: &mut App, ui: &mut egui::Ui, on: bool) {
    let palette = app.palette;
    let bands = crate::eq::BANDS;
    let width = ui.available_width();
    let per_row = if width >= bands.len() as f32 * BAND_ROOM {
        bands.len()
    } else {
        bands.len() / 2
    };
    let gaps = BAND_GAP * (per_row - 1) as f32;
    let column = ((width - gaps) / per_row as f32).max(theme::TOUCH_TARGET);
    let slider = eq_slider_width(ui.ctx());
    for (row, hertzes) in bands.chunks(per_row).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = BAND_GAP;
            for (offset, hz) in hertzes.iter().enumerate() {
                let band = row * per_row + offset;
                ui.allocate_ui_with_layout(
                    vec2(column, 140.0),
                    Layout::left_to_right(Align::Min),
                    |ui| {
                        ui.set_min_width(column);
                        ui.add_space(((column - slider) / 2.0).max(0.0));
                        let mut gain = app.settings.eq_bands_db[band];
                        if eq_slider(ui, &palette, &hertz(*hz), &mut gain, on) {
                            app.actions.push(Action::SetEqBand(band, gain));
                        }
                    },
                );
            }
        });
        ui.add_space(8.0);
    }
}
