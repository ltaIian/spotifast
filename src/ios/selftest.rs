//! Diagnostics for the simulator's smoke test, asked for in the app's
//! environment with `SPOTIFAST_SELFTEST`:
//!
//! - `1`: each check logs what it found, on lines starting `selftest:`, and
//!   the app carries on: none of them changes what the app does, and none
//!   can end it;
//! - `signin-web`: the Web API sign-in starts by itself once the sign-in
//!   screen is up, as its button starts it, so its sheet shows what Spotify
//!   answers this client id with;
//! - `signin-playback`: the playback authorization, the same way.

use std::time::Duration;

use crate::app::App;
use crate::backend::AuthStatus;
use crate::model::Action;

/// A request that needs no sign-in, over TLS to Spotify.
const APRESOLVE: &str = "https://apresolve.spotify.com/?type=accesspoint";
/// How long a network check waits for its answer.
const TIMEOUT: Duration = Duration::from_secs(20);
/// How many changes of the window's safe area are logged.
const INSET_REPORTS: u8 = 6;

/// What `SPOTIFAST_SELFTEST` asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `1`: the diagnostics below.
    Diagnostics,
    /// `signin-web`: the Web API sign-in, started at launch.
    SignInWeb,
    /// `signin-playback`: the playback authorization, started at launch.
    SignInPlayback,
}

/// What this run was asked for, if anything.
pub fn requested() -> Option<Mode> {
    let asked = std::env::var("SPOTIFAST_SELFTEST").ok()?;
    match asked.as_str() {
        "1" => Some(Mode::Diagnostics),
        "signin-web" => Some(Mode::SignInWeb),
        "signin-playback" => Some(Mode::SignInPlayback),
        _ => None,
    }
}

/// Starts a sign-in once the sign-in screen is up, with the action its
/// button sends: Sign in with Spotify for the Web API, Set up playback for
/// the playback authorization.
pub struct SignIn {
    action: Option<Action>,
}

impl SignIn {
    pub fn for_mode(mode: Mode) -> Option<Self> {
        let action = match mode {
            Mode::SignInWeb => Action::SignIn,
            Mode::SignInPlayback => Action::EnablePlayback,
            Mode::Diagnostics => return None,
        };
        Some(Self {
            action: Some(action),
        })
    }

    pub fn poll(&mut self, app: &mut App) {
        if matches!(app.auth, AuthStatus::SignedOut)
            && let Some(action) = self.action.take()
        {
            log::info!("selftest: starting the sign-in with {action:?}, as its button does");
            app.actions.push(action);
        }
    }
}

/// Runs the checks on a thread of their own while the window opens.
pub fn start() {
    let spawned = std::thread::Builder::new()
        .name("selftest".into())
        .spawn(|| {
            check("time zone", time_zone);
            check("TLS through reqwest", reqwest_round_trip);
            check("TLS through librespot", librespot_round_trip);
            check("Keychain", keychain_round_trip);
            check("default output", default_output);
            log::info!("selftest: done");
        });
    if let Err(error) = spawned {
        log::warn!("selftest: not started: {error}");
    }
}

/// Runs one check and logs how it went. A panic is caught and logged as a
/// failure, so it cannot end the app (in the debug build the smoke test
/// runs; a release build aborts on any panic).
fn check(name: &str, run: fn() -> Result<String, String>) {
    match std::panic::catch_unwind(run) {
        Ok(Ok(found)) => log::info!("selftest: {name}: ok: {found}"),
        Ok(Err(error)) => log::warn!("selftest: {name}: failed: {error}"),
        Err(_) => log::warn!("selftest: {name}: failed: it panicked"),
    }
}

/// The zone main.swift named in TZ, and what jiff makes of it.
fn time_zone() -> Result<String, String> {
    let named = std::env::var("TZ").unwrap_or_default();
    let zone = jiff::tz::TimeZone::system();
    Ok(format!("TZ {named:?}, jiff {:?}", zone.iana_name()))
}

/// apresolve through the app's own reqwest client.
fn reqwest_round_trip() -> Result<String, String> {
    let client = crate::http::build_blocking(&crate::settings::ProxyConfig::System, TIMEOUT)?;
    let response = client
        .get(APRESOLVE)
        .send()
        .map_err(|error| describe(&error))?;
    let status = response.status();
    let body = response.bytes().map_err(|error| describe(&error))?;
    Ok(format!("HTTP {status}, {} bytes", body.len()))
}

/// apresolve through librespot's HTTP client, which signs the player in.
fn librespot_round_trip() -> Result<String, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| describe(&error))?;
    runtime.block_on(async {
        let client = librespot_core::http_client::HttpClient::new(None);
        let request = http::Request::builder()
            .uri(APRESOLVE)
            .body(Default::default())
            .map_err(|error| describe(&error))?;
        let body = tokio::time::timeout(TIMEOUT, client.request_body(request))
            .await
            .map_err(|_| "no answer in time".to_owned())?
            .map_err(|error| describe(&error))?;
        Ok::<_, String>(format!("{} bytes", body.len()))
    })
}

/// A throwaway entry written, read back and deleted through the store the
/// grants use.
fn keychain_round_trip() -> Result<String, String> {
    crate::credentials::self_test().map(str::to_owned)
}

/// The output cpal offers, from the audio session main.swift configured:
/// what the player opens.
fn default_output() -> Result<String, String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no default output device")?;
    let name = device
        .description()
        .map_err(|error| describe(&error))?
        .name()
        .to_owned();
    let config = device
        .default_output_config()
        .map_err(|error| describe(&error))?;
    Ok(format!(
        "{name}: {} Hz, {} channels, {}",
        config.sample_rate(),
        config.channels(),
        config.sample_format()
    ))
}

/// An error with the errors that caused it, which say what went wrong with
/// TLS.
fn describe(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Logs the window's whole area and the part safe for content, each time
/// they change, a few times at most: the safe area is egui-winit's, read
/// from the window's scene.
#[derive(Default)]
pub struct Insets {
    last: Option<(egui::Rect, egui::Rect)>,
    reported: u8,
}

impl Insets {
    pub fn report(&mut self, ctx: &egui::Context) {
        if self.reported >= INSET_REPORTS {
            return;
        }
        let rects = (ctx.viewport_rect(), ctx.content_rect());
        if self.last == Some(rects) {
            return;
        }
        self.last = Some(rects);
        self.reported += 1;
        let (viewport, content) = rects;
        let insets = ctx.input(|input| input.raw.safe_area_insets);
        let scale = ctx.pixels_per_point();
        log::info!("selftest: window {viewport:?}, content {content:?}, scale {scale}");
        log::info!("selftest: safe area insets {insets:?}");
    }
}
