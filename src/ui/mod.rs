//! Window layout: panels, overlays, keyboard shortcuts.

use std::sync::Arc;

pub mod artist;
pub mod collection;
pub(crate) mod devices;
mod dialogs;
pub mod equalizer;
pub(crate) mod friends;
pub mod home;
mod keys;
pub mod library;
pub mod login;
mod lyrics;
pub mod new_releases;
pub mod player_bar;
mod prerelease;
pub mod queue;
pub mod radio;
pub mod search;
pub mod settings;
pub mod show;
pub mod sidebar;
pub mod topbar;
mod update;
pub mod widgets;
pub mod winamp;

use egui::{Align2, Color32, Context, CornerRadius, Frame, Id, Margin, Rect, Stroke, vec2};

use crate::api::models::pick_image;
use crate::app::App;
use crate::backend::AuthStatus;
use crate::model::{Action, Loadable, Page, ToastKind};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let ctx = &ctx;
    theme::settle_layout(ctx);
    let mut safe = safe_area(ui, &app.palette);
    let ui = &mut safe;
    keys::handle(app, ctx);
    for path in winamp::dropped_skins(ctx) {
        app.actions.push(Action::InstallSkin(path));
    }
    let signed_in = app.is_connected() && app.user.is_some();
    let connecting = matches!(app.auth, AuthStatus::Connecting | AuthStatus::Starting)
        || (app.is_connected() && app.user.is_none());
    if !signed_in {
        player_bar::end_tint_session(ctx);
        set_library_open(ctx, false);
        set_now_playing_open(ctx, false);
        login::show(app, ui, connecting);
        update::show(app, ctx);
        toasts(app, ctx, 20.0);
        caption_buttons(ui, &app.palette, app.locale);
        fastframe_titlebar::resize(ui);
        return;
    }
    let compact = theme::compact(ctx);
    if compact {
        compact_layout(app, ui);
    } else {
        if titlebar_spans_window(cfg!(target_os = "macos"), fastframe_titlebar::enabled()) {
            fastframe_titlebar::drag(
                ui,
                Rect::from_min_size(
                    ui.max_rect().min,
                    vec2(
                        ui.max_rect().width(),
                        theme::top_bar_height() + theme::titlebar_inset(ui.ctx()),
                    ),
                ),
            );
        }
        player_bar::show(app, ui);
        if app.lyrics_fullscreen.is_some() {
            lyrics::fullscreen(app, ui);
        } else {
            if app.settings.sidebar_visible {
                sidebar::show(app, ui);
            }
            if app.show_queue_panel {
                queue::side_panel(app, ui);
            }
            if app.show_lyrics_panel {
                lyrics::side_panel(app, ui);
            }
            if app.show_friends_panel {
                friends::side_panel(app, ui);
            }
            central(app, ui);
            keep_room_for_panels(app, ctx);
        }
    }
    devices::popup(app, ctx);
    dialogs::show(app, ctx);
    update::show(app, ctx);
    widgets::drag_ghost(ctx, &app.palette, app.locale);
    let bar = if compact {
        theme::MINI_BAR_HEIGHT
    } else {
        theme::PLAYER_BAR_HEIGHT
    };
    toasts(app, ctx, bar + 16.0);
    caption_buttons(ui, &app.palette, app.locale);
    fastframe_titlebar::resize(ui);
}

/// The safe area. On a phone the status bar and the home indicator cover
/// the screen's edges: the panel colour fills behind them, and everything
/// else lays out in the child returned here, where nothing is covered.
/// Toasts, popups, menus and modals keep to the same area on their own. On
/// a desktop the two rects are the same and nothing changes.
fn safe_area(ui: &mut egui::Ui, palette: &theme::Palette) -> egui::Ui {
    let ctx = ui.ctx().clone();
    let content = theme::content_rect(&ctx);
    let viewport = ctx.viewport_rect();
    if content != viewport {
        ui.painter().rect_filled(viewport, 0.0, palette.panel);
    }
    ui.new_child(
        egui::UiBuilder::new()
            .id_salt("safe-area")
            .max_rect(content)
            .style(ctx.global_style()),
    )
}

/// The band above the safe area that a phone's status bar covers, if any.
/// Whatever is at the top of the screen fills it in its own colour.
pub(crate) fn status_bar_band(ctx: &Context) -> Option<Rect> {
    let content = theme::content_rect(ctx);
    let viewport = ctx.viewport_rect();
    (content.top() > viewport.top())
        .then(|| Rect::from_min_max(viewport.min, egui::pos2(viewport.right(), content.top())))
}

/// Fills `band`, outside the safe area, in `color`. A panel's own painter
/// stops at the panel's edge, so this one is clipped to the band instead.
pub(crate) fn fill_band(ui: &egui::Ui, band: Rect, color: Color32) {
    let mut painter = ui.painter().clone();
    painter.set_clip_rect(band);
    painter.rect_filled(band, 0.0, color);
}

/// The band below the safe area that a phone's home indicator covers, if
/// any. The player bar fills it in its own colour.
pub(crate) fn home_indicator_band(ctx: &Context) -> Option<Rect> {
    let content = theme::content_rect(ctx);
    let viewport = ctx.viewport_rect();
    (content.bottom() < viewport.bottom())
        .then(|| Rect::from_min_max(egui::pos2(viewport.left(), content.bottom()), viewport.max))
}

/// What fills the screen above the player bar in the compact layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    Page,
    Library,
    NowPlaying,
    Queue,
    Lyrics,
    Friends,
    LyricsFullscreen,
}

const LIBRARY_OPEN_ID: &str = "compact-library-open";
const NOW_PLAYING_OPEN_ID: &str = "compact-now-playing-open";

/// Whether the compact layout shows the library over the page: the
/// sidebar, which has no room beside the page on a phone.
pub(crate) fn library_open(ctx: &Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(Id::new(LIBRARY_OPEN_ID)))
        .unwrap_or(false)
}

pub(crate) fn set_library_open(ctx: &Context, open: bool) {
    ctx.data_mut(|data| data.insert_temp(Id::new(LIBRARY_OPEN_ID), open));
}

/// Whether the compact layout shows the playing song's own view, which the
/// player bar opens.
pub(crate) fn now_playing_open(ctx: &Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(Id::new(NOW_PLAYING_OPEN_ID)))
        .unwrap_or(false)
}

pub(crate) fn set_now_playing_open(ctx: &Context, open: bool) {
    ctx.data_mut(|data| data.insert_temp(Id::new(NOW_PLAYING_OPEN_ID), open));
}

/// The topmost surface: a right-hand panel over the playing song's view,
/// that over the library, and the library over the page.
fn compact_surface(app: &App, ctx: &Context) -> Surface {
    if app.lyrics_fullscreen.is_some() {
        Surface::LyricsFullscreen
    } else if app.show_queue_panel {
        Surface::Queue
    } else if app.show_lyrics_panel {
        Surface::Lyrics
    } else if app.show_friends_panel {
        Surface::Friends
    } else if now_playing_open(ctx) {
        Surface::NowPlaying
    } else if library_open(ctx) {
        Surface::Library
    } else {
        Surface::Page
    }
}

/// The compact layout, a phone's: one column. The library, the right-hand
/// panels and the playing song's view each take the whole screen over the
/// page, and the player bar shrinks to one row that opens that view.
fn compact_layout(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    // A new page, however it came, is shown with nothing laid over it.
    let surface = if page_changed(app, &ctx) {
        close_overlays(app, &ctx);
        Surface::Page
    } else {
        compact_surface(app, &ctx)
    };
    if surface != Surface::NowPlaying {
        player_bar::show(app, ui);
    }
    let before = app.actions.len();
    match surface {
        Surface::LyricsFullscreen => lyrics::fullscreen(app, ui),
        Surface::Queue => queue::side_panel(app, ui),
        Surface::Lyrics => lyrics::side_panel(app, ui),
        Surface::Friends => friends::side_panel(app, ui),
        Surface::NowPlaying => player_bar::now_playing(app, ui),
        Surface::Library => sidebar::show(app, ui),
        Surface::Page => central(app, ui),
    }
    // Going to a page from a screen laid over the page shows the page.
    if app.actions[before..].iter().any(navigates) {
        close_overlays(app, &ctx);
    }
}

/// Where the compact layout keeps the page it showed last.
const SHOWN_PAGE_ID: &str = "compact-shown-page";

/// Whether the page is another than the one the compact layout showed the
/// frame before, whatever changed it.
fn page_changed(app: &App, ctx: &Context) -> bool {
    let page = Id::new(app.page());
    let id = Id::new(SHOWN_PAGE_ID);
    let shown = ctx.data(|data| data.get_temp::<Id>(id));
    if shown != Some(page) {
        ctx.data_mut(|data| data.insert_temp(id, page));
    }
    shown.is_some_and(|shown| shown != page)
}

/// Whether an action goes to a page.
fn navigates(action: &Action) -> bool {
    matches!(
        action,
        Action::Open(_) | Action::FocusSearch | Action::Back | Action::Forward
    )
}

/// Takes every screen laid over the page away: the library, the playing
/// song's view, the right-hand panels and the lyrics' full screen.
fn close_overlays(app: &mut App, ctx: &Context) {
    set_library_open(ctx, false);
    set_now_playing_open(ctx, false);
    close_panels(app);
}

/// Opens the playing song's view, as the player bar does when tapped. A
/// right-hand panel laid over the page closes, so the view shows at once
/// rather than once the panel is closed some other way.
pub(crate) fn open_now_playing(app: &mut App, ctx: &Context) {
    close_panels(app);
    set_now_playing_open(ctx, true);
}

/// Closes the right-hand panels and the lyrics' full screen, through the
/// actions their own close buttons send.
fn close_panels(app: &mut App) {
    let open = [
        (app.show_queue_panel, Action::ToggleQueuePanel),
        (app.show_lyrics_panel, Action::ToggleLyricsPanel),
        (app.show_friends_panel, Action::ToggleFriendsPanel),
        (
            app.lyrics_fullscreen.is_some(),
            Action::SetLyricsFullscreen(false),
        ),
    ];
    for (shown, close) in open {
        // Its own close button may have asked already this frame.
        let kind = std::mem::discriminant(&close);
        let same = |action: &Action| std::mem::discriminant(action) == kind;
        if shown && !app.actions.iter().any(same) {
            app.actions.push(close);
        }
    }
}

/// The main window's narrowest width with these panels open: their least
/// widths and the page's.
fn main_min_width(page: f32, sidebar: bool, right_panel: bool) -> f32 {
    let sidebar = if sidebar { SIDEBAR_MIN_WIDTH } else { 0.0 };
    let right = if right_panel {
        theme::SIDE_PANEL_MIN_WIDTH
    } else {
        0.0
    };
    (sidebar + right + page).max(crate::window::MAIN_MIN_SIZE[0])
}

/// The sidebar's narrowest width.
pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 210.0;

/// Raise the window's minimum width while the Queue or Lyrics panel is open,
/// so even at its narrowest the page beside the panels keeps the room its
/// top bar needs (#624), and lower it again once they close. Window managers
/// that tile ignore the minimum; the panels still give way there.
fn keep_room_for_panels(app: &App, ctx: &Context) {
    if crate::window::fixed_size() {
        return;
    }
    let width = main_min_width(
        topbar::least_width(ctx),
        app.settings.sidebar_visible,
        app.show_queue_panel || app.show_lyrics_panel || app.show_friends_panel,
    )
    .round();
    let id = Id::new("main-min-width");
    let sent = ctx.data(|data| data.get_temp::<f32>(id));
    // The window starts at MAIN_MIN_SIZE, so there is nothing to send until
    // the panels need more.
    if sent.unwrap_or(crate::window::MAIN_MIN_SIZE[0]) != width {
        ctx.data_mut(|data| data.insert_temp(id, width));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(
            width,
            crate::window::MAIN_MIN_SIZE[1],
        )));
    }
}

/// How a resizable side panel fits beside the page.
pub(crate) struct PanelFit {
    /// The widths the panel may take this frame.
    pub range: std::ops::RangeInclusive<f32>,
    /// Whether the room the page needs holds the panel under the width the
    /// person chose for it.
    pub yielding: bool,
}

/// Fit a side panel between `range` and the page beside it. The panel gives
/// up width, down to its minimum, so the page keeps `room_for_page`, the
/// least its top bar needs (#624). The width chosen for the panel stays in
/// the settings and comes back once the window widens again.
///
/// `room` is what the panel and everything after it share.
pub(crate) fn yielding_panel(
    ctx: &Context,
    id: &str,
    range: std::ops::RangeInclusive<f32>,
    chosen: f32,
    room: f32,
) -> PanelFit {
    let (min, max) = range.into_inner();
    let max = max.min(room).max(min);
    let width = chosen.clamp(min, max);
    // egui remembers the width the panel last drew at. While nobody drags
    // its handle, hand it back the chosen width, either way: a resize made
    // elsewhere (the fullscreen queue column shares the queue width) shows
    // here too, and the panel never writes a stale width back over it.
    let id = Id::new(id);
    if !ctx.is_being_dragged(id.with("__resize"))
        && let Some(mut state) = egui::containers::panel::PanelState::load(ctx, id)
        && (state.outer_rect.width() - width).abs() > 0.5
    {
        state.outer_rect.set_width(width);
        ctx.data_mut(|data| data.insert_persisted(id, state));
    }
    PanelFit {
        range: min..=max,
        yielding: max < chosen - 0.5,
    }
}

/// The library or a right-hand panel in the compact layout: the whole
/// width, over the page. It is not resizable there, and the width it is
/// drawn at is never one the person chose, so it never replaces the width
/// kept for the desktop.
pub(crate) fn full_width(ui: &egui::Ui) -> PanelFit {
    let width = ui.available_width();
    PanelFit {
        range: width..=width,
        yielding: true,
    }
}

/// Whether the person is dragging this panel's resize handle right now.
pub(crate) fn panel_dragged(ctx: &Context, id: &str) -> bool {
    ctx.is_being_dragged(Id::new(id).with("__resize"))
}

/// Whether a panel's new width is one the person chose: it moved while not
/// held back by the page, or by their own drag.
pub(crate) fn panel_width_chosen(ctx: &Context, id: &str, fit: &PanelFit) -> bool {
    !fit.yielding || panel_dragged(ctx, id)
}

/// Keeps the most recent loading preview of each metadata type available to
/// the loaded hero as an artwork fallback. The fixed typed slot bounds this to
/// one playlist, album, artist, and show instead of scanning known metadata on
/// every loaded frame.
fn loading_preview<T>(
    ctx: &Context,
    key: &str,
    details: &Loadable<T>,
    known: impl FnOnce() -> Option<T>,
) -> Option<Arc<T>>
where
    T: Send + Sync + 'static,
{
    let memory_id = Id::new("collection-loading-preview");
    let key = Id::new(key);
    if details.get().is_some() {
        ctx.data(|data| data.get_temp::<(Id, Arc<T>)>(memory_id))
            .filter(|(stored_key, _)| *stored_key == key)
            .map(|(_, preview)| preview)
    } else {
        let preview = known().map(Arc::new);
        ctx.data_mut(|data| match &preview {
            Some(preview) => {
                data.insert_temp(memory_id, (key, Arc::clone(preview)));
            }
            None => data.remove::<(Id, Arc<T>)>(memory_id),
        });
        preview
    }
}

fn page_tint(app: &mut App) -> Option<Color32> {
    let page = app.page().clone();
    let image = match &page {
        Page::Playlist(id) => app
            .playlist_pages
            .get(id)
            .and_then(|page| page.playlist.get())
            .or_else(|| app.known_playlist(id))
            .and_then(|playlist| pick_image(&playlist.images, 64))
            .map(str::to_string),
        Page::Album(id) => app
            .album_pages
            .get(id)
            .and_then(|page| page.album.get())
            .or_else(|| app.known_album(id))
            .and_then(|album| pick_image(&album.images, 64))
            .map(str::to_string),
        Page::Artist(id) => app
            .artist_pages
            .get(id)
            .and_then(|page| page.artist.get())
            .or_else(|| app.known_artist(id))
            .and_then(|artist| pick_image(&artist.images, 64))
            .map(str::to_string),
        Page::Show(id) => app
            .show_pages
            .get(id)
            .and_then(|page| page.show.get())
            .or_else(|| app.known_show(id))
            .and_then(|show| pick_image(&show.images, 64))
            .map(str::to_string),
        Page::Radio(seed) => pick_image(&app.radio_images(seed), 64).map(str::to_string),
        Page::LikedSongs => {
            return app
                .settings
                .accent_from_art
                .then_some(Color32::from_rgb(0x50, 0x38, 0xc8));
        }
        _ => None,
    };
    if !app.settings.accent_from_art && image.is_some() {
        return None;
    }
    match image {
        Some(url) => app.tint_for(Some(&url)).or_else(|| app.now_playing_tint()),
        None => app.now_playing_tint(),
    }
}

fn central(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let tint = page_tint(app);
    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.window))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            let page = app.page().clone();
            let overview = matches!(
                page,
                Page::Home | Page::Search | Page::Settings | Page::Queue | Page::Equalizer
            );
            // The wash belongs to the page's hero, so it scrolls away with
            // it. The page's offset is known once it has been laid out, so
            // the previous frame's serves.
            let scrolled_id = Id::new(("page-scrolled", page.encode()));
            let scrolled = ui
                .ctx()
                .data(|data| data.get_temp::<f32>(scrolled_id))
                .unwrap_or(0.0);
            let wash = header_wash(&palette, overview, tint);
            if let Some((top, bottom)) = wash {
                widgets::paint_vertical_gradient(ui, header_rect(rect, scrolled), top, bottom);
            }
            // On a phone the status bar sits over the band above the page,
            // which takes the colour of the page's top edge.
            if let Some(band) = status_bar_band(ui.ctx()) {
                fill_band(ui, band, palette.window);
                if let Some((top, bottom)) = wash {
                    let shown = (scrolled / HEADER_WASH_HEIGHT).clamp(0.0, 1.0);
                    fill_band(ui, band, top.lerp_to_gamma(bottom, shown));
                }
            }
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            // egui fades a scrolled page's edge into the panel's plain
            // colour, which shows as a pale band over a cover's tint; the
            // page casts a shadow under the header instead.
            ui.spacing_mut().scroll.fade.strength = 0.0;
            topbar::show(app, ui);
            let scroll = crate::autoscroll::show(
                ui,
                egui::ScrollArea::vertical()
                    .id_salt(("page", page.encode()))
                    .auto_shrink([false, false]),
                egui::Vec2b::new(false, true),
                |ui| {
                    let padding = widgets::page_padding(ui.ctx()) as i8;
                    Frame::new()
                        .inner_margin(Margin {
                            left: padding,
                            right: padding,
                            top: 4,
                            bottom: 48,
                        })
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            match page {
                                Page::Home => home::show(app, ui),
                                Page::TopSongs => collection::top_songs(app, ui),
                                Page::NewReleases => new_releases::page(app, ui),
                                Page::Search => search::show(app, ui),
                                Page::LikedSongs => collection::liked(app, ui),
                                Page::Albums | Page::Artists | Page::Podcasts | Page::Episodes => {
                                    library::show(app, ui, page)
                                }
                                Page::Playlist(id) => collection::playlist(app, ui, &id),
                                Page::Album(id) => collection::album(app, ui, &id),
                                Page::Artist(id) => artist::show(app, ui, &id),
                                Page::Show(id) => show::show(app, ui, &id),
                                Page::Radio(seed) => radio::radio(app, ui, &seed),
                                Page::Queue => queue::page(app, ui),
                                Page::Settings => settings::show(app, ui),
                                Page::Equalizer => equalizer::page(app, ui),
                            }
                        });
                },
            );
            ui.ctx()
                .data_mut(|data| data.insert_temp(scrolled_id, scroll.state.offset.y));
            header_shadow(ui, scroll.inner_rect, scroll.state.offset.y, palette.dark);
        });
}

/// The gradient behind a page's header, top to bottom: the art's colour
/// blended into the window, stronger on a cover's own page than on an
/// overview, or else the palette's `tint` fading out, whose alpha is its
/// strength. `None` leaves the plain window.
fn header_wash(
    palette: &theme::Palette,
    overview: bool,
    art: Option<Color32>,
) -> Option<(Color32, Color32)> {
    if let Some(art) = art {
        let strength = if overview { 0.45 } else { 0.85 };
        return Some((blend(palette.window, art, strength), palette.window));
    }
    (palette.tint.a() > 0).then_some((palette.tint, Color32::TRANSPARENT))
}

/// How far down the page the header's wash fades out.
const HEADER_WASH_HEIGHT: f32 = 340.0;

/// Where the header's wash is painted: the top of the panel, moved up by
/// how far the page has scrolled, so it leaves with the hero.
fn header_rect(panel: Rect, scrolled: f32) -> Rect {
    Rect::from_min_size(
        panel.min - vec2(0.0, scrolled.max(0.0)),
        vec2(panel.width(), HEADER_WASH_HEIGHT),
    )
}

/// The shadow the header casts on a page scrolled under it, deepening over
/// the first few points of scrolling. Dark in both themes, lighter over a
/// light page.
fn header_shadow(ui: &egui::Ui, page: Rect, scrolled: f32, dark: bool) {
    let depth = (scrolled / 24.0).clamp(0.0, 1.0);
    if depth <= 0.0 {
        return;
    }
    let strength = if dark { 110.0 } else { 36.0 };
    let rect = Rect::from_min_size(page.min, vec2(page.width(), 14.0));
    widgets::paint_vertical_gradient(
        ui,
        rect,
        egui::Color32::from_black_alpha((strength * depth) as u8),
        egui::Color32::TRANSPARENT,
    );
}

/// Without a native title bar, the top of every panel drags the window, not
/// only the page's top bar.
const fn titlebar_spans_window(on_macos: bool, custom_titlebar: bool) -> bool {
    on_macos || custom_titlebar
}

/// The room the top bar's own controls need beside the caption buttons: the
/// 760-point minimum with the default 250-point sidebar leaves 510 points.
const TOPBAR_MIN_CONTENT: f32 = 510.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WindowControlsReservation {
    pub topbar_width: f32,
    pub topbar_top: f32,
    pub queue_top: f32,
    pub lyrics_top: f32,
}

/// The caption buttons on the right belong to the outermost header: the
/// queue's or the lyrics' when either panel is open, otherwise the top bar,
/// beside its controls or above them when the page is too narrow.
fn controls_reservation(
    footprint: fastframe_titlebar::Footprint,
    queue: bool,
    lyrics: bool,
    topbar_width: f32,
) -> WindowControlsReservation {
    let mut space = WindowControlsReservation {
        topbar_width: 0.0,
        topbar_top: 0.0,
        queue_top: 0.0,
        lyrics_top: 0.0,
    };
    if footprint.right <= 0.0 {
        return space;
    }
    if queue {
        space.queue_top = footprint.height;
    } else if lyrics {
        space.lyrics_top = footprint.height;
    } else {
        let reserve = footprint.reserve(topbar_width, TOPBAR_MIN_CONTENT);
        space.topbar_width = reserve.inline_width;
        space.topbar_top = reserve.push_down;
    }
    space
}

/// Left-hand Linux controls sit above the sidebar's navigation rows.
pub(super) fn linux_left_controls_inset(ctx: &egui::Context) -> f32 {
    if !cfg!(target_os = "linux") {
        return 0.0;
    }
    let footprint = fastframe_titlebar::footprint(ctx, theme::CUSTOM_TOP_BAR_HEIGHT);
    if footprint.left > 0.0 {
        footprint.height
    } else {
        0.0
    }
}

pub(super) fn window_controls_reservation(
    ctx: &egui::Context,
    queue: bool,
    lyrics: bool,
    topbar_width: f32,
) -> WindowControlsReservation {
    controls_reservation(
        fastframe_titlebar::footprint(ctx, theme::CUSTOM_TOP_BAR_HEIGHT),
        queue,
        lyrics,
        topbar_width,
    )
}

/// Draws the caption buttons over the window's top corners in the palette's
/// colours: Windows' glyphs in the secondary text colour, GNOME's round
/// buttons in the text colour on the hover surface.
fn caption_buttons(ui: &mut egui::Ui, palette: &theme::Palette, locale: crate::i18n::Locale) {
    use crate::i18n::gettext;
    let style = fastframe_titlebar::Style {
        icon: if cfg!(target_os = "linux") {
            palette.text
        } else {
            palette.secondary
        },
        button: palette.surface_hover,
        header_height: theme::CUSTOM_TOP_BAR_HEIGHT,
    };
    // Literal calls, so xgettext extracts each label.
    let minimize = gettext(locale, "Minimize");
    let maximize = gettext(locale, "Maximize");
    let restore = gettext(locale, "Restore");
    let close = gettext(locale, "Close");
    let labels = fastframe_titlebar::Labels {
        minimize: &minimize,
        maximize: &maximize,
        restore: &restore,
        close: &close,
    };
    fastframe_titlebar::controls(ui, &style, &labels);
}

pub fn blend(base: Color32, tint: Color32, amount: f32) -> Color32 {
    let a = egui::Rgba::from(base);
    let b = egui::Rgba::from(tint);
    let mixed = a * (1.0 - amount) + b * amount;
    let mut color = Color32::from(mixed);
    color[3] = 255;
    color
}

fn toasts(app: &mut App, ctx: &egui::Context, bottom_offset: f32) {
    if app.toasts.is_empty() {
        return;
    }
    let palette = app.palette;
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -bottom_offset))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            for toast in &app.toasts {
                let age = toast.created.elapsed().as_secs_f32();
                let alpha = if age < 0.15 {
                    age / 0.15
                } else if age > 2.8 {
                    ((3.2 - age) / 0.4).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                ui.set_opacity(alpha);
                Frame::new()
                    .fill(palette.overlay)
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(theme::RADIUS))
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: palette.shadow,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (icon, color) = match toast.kind {
                                ToastKind::Info => (Icon::CircleCheck, palette.accent),
                                ToastKind::Error => (Icon::CircleAlert, palette.danger),
                            };
                            theme::icon(ui, icon, 16.0, color);
                            // Laid out at its own width. The area
                            // remembers its size, so after a short toast a
                            // label left to wrap at the area's width broke
                            // long messages on every word.
                            let galley = ui.painter().layout(
                                toast.message.clone(),
                                theme::medium(13.5),
                                palette.text,
                                280.0,
                            );
                            ui.add(egui::Label::new(galley));
                        });
                    });
            }
        });
}

#[cfg(test)]
mod window_chrome_tests {
    use super::*;
    use fastframe_titlebar::Footprint;

    /// Windows' three buttons in its 48-point header.
    const BUTTONS: Footprint = Footprint {
        left: 0.0,
        right: 3.0 * 36.0 + 5.0,
        height: theme::CUSTOM_TOP_BAR_HEIGHT,
    };

    #[test]
    fn the_side_panels_drag_a_window_without_a_native_title_bar() {
        assert!(titlebar_spans_window(true, false));
        assert!(titlebar_spans_window(false, true));
        assert!(!titlebar_spans_window(false, false));
    }

    #[test]
    fn caption_space_belongs_to_the_outermost_header() {
        let values = |footprint, queue, lyrics| {
            let space = controls_reservation(footprint, queue, lyrics, f32::INFINITY);
            [
                space.topbar_width,
                space.topbar_top,
                space.queue_top,
                space.lyrics_top,
            ]
        };
        let [width, height] = [BUTTONS.right, BUTTONS.height];
        assert_eq!(values(BUTTONS, false, false), [width, 0.0, 0.0, 0.0]);
        assert_eq!(values(BUTTONS, true, false), [0.0, 0.0, height, 0.0]);
        assert_eq!(values(BUTTONS, false, true), [0.0, 0.0, 0.0, height]);
        assert_eq!(values(BUTTONS, true, true), [0.0, 0.0, height, 0.0]);
        // No buttons on the right: full screen, the native frame, macOS's
        // traffic lights, or a GNOME layout with them all on the left.
        let none = Footprint {
            left: 50.0,
            ..Footprint::default()
        };
        for (queue, lyrics) in [(false, false), (true, false), (false, true)] {
            assert_eq!(values(none, queue, lyrics), [0.0; 4]);
        }
    }

    #[test]
    fn minimum_windows_window_stacks_caption_space_above_the_topbar() {
        let available = 760.0 - 250.0;
        let space = controls_reservation(BUTTONS, false, false, available);
        assert_eq!(space.topbar_width, 0.0);
        assert_eq!(space.topbar_top, BUTTONS.height);

        let inline =
            controls_reservation(BUTTONS, false, false, TOPBAR_MIN_CONTENT + BUTTONS.right);
        assert_eq!(inline.topbar_width, BUTTONS.right);
        assert_eq!(inline.topbar_top, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page fades from its cover's colour while art tints the interface,
    /// otherwise from the palette's own wash, and from nothing when the
    /// palette has none.
    #[test]
    fn the_header_fades_from_the_art_or_the_palettes_wash() {
        let mut palette = theme::Palette::dark();
        assert_eq!(header_wash(&palette, false, None), None);

        let art = Color32::from_rgb(0x50, 0x38, 0xc8);
        let (page, _) = header_wash(&palette, false, Some(art)).unwrap();
        let (overview, _) = header_wash(&palette, true, Some(art)).unwrap();
        assert_eq!(page, blend(palette.window, art, 0.85));
        assert_eq!(overview, blend(palette.window, art, 0.45));

        palette.tint = Color32::from_black_alpha(24);
        assert_eq!(
            header_wash(&palette, false, None),
            Some((palette.tint, Color32::TRANSPARENT))
        );
        assert_eq!(
            header_wash(&palette, false, Some(art)).unwrap().0,
            page,
            "art wins over the wash"
        );
    }

    /// The wash sits at the top of an unscrolled page and leaves with the
    /// hero as the page scrolls, instead of staying behind the page.
    #[test]
    fn the_header_wash_scrolls_away_with_the_page() {
        let panel = Rect::from_min_size(egui::pos2(0.0, 80.0), vec2(800.0, 600.0));
        assert_eq!(header_rect(panel, 0.0).min, panel.min);
        assert_eq!(header_rect(panel, 0.0).width(), 800.0);
        assert_eq!(header_rect(panel, 120.0).min.y, panel.min.y - 120.0);
        assert_eq!(header_rect(panel, 120.0).max.y, panel.min.y + 220.0);
        assert!(
            header_rect(panel, 400.0).max.y < panel.min.y,
            "gone once scrolled past"
        );
        assert_eq!(
            header_rect(panel, -5.0).min,
            panel.min,
            "no overscroll bounce"
        );
    }

    /// The header casts a shadow only on a page scrolled under it, and it
    /// is black in both themes, never the page's own colour.
    #[test]
    fn the_header_shadow_appears_once_the_page_scrolls() {
        let ctx = egui::Context::default();
        let page = Rect::from_min_size(egui::pos2(0.0, 80.0), vec2(800.0, 600.0));
        let shadows = |scrolled: f32, dark: bool| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                header_shadow(ui, page, scrolled, dark);
            });
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh.vertices.clone()),
                    _ => None,
                })
                .flatten()
                .collect::<Vec<_>>()
        };
        assert!(
            shadows(0.0, true).is_empty(),
            "nothing at the top of the page"
        );
        for dark in [true, false] {
            let vertices = shadows(40.0, dark);
            let top = vertices
                .iter()
                .find(|vertex| vertex.pos.y == page.top())
                .expect("a shadow along the page's top edge");
            assert!(top.color.a() > 0);
            assert_eq!((top.color.r(), top.color.g(), top.color.b()), (0, 0, 0));
        }
    }
}
