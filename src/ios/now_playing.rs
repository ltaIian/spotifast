//! Now Playing on the lock screen and in Control Center, and the remote
//! commands they and headphones send back, through the Swift side
//! (`ios/Sources/NowPlaying.swift`).
//!
//! fastframe-now-playing has no iOS controls of its own, so this has the
//! same interface as its `NowPlaying`, which `crate::app` uses on the
//! desktop, and takes the same `State` and gives the same `Command`s.

use std::cell::Cell;
use std::ffi::{CString, c_char};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use fastframe_now_playing::{App, Command, Playback, State, Track};

/// How far the position may stray from where iOS has it before it is sent
/// again. iOS moves the elapsed time on by itself while playing.
const DRIFT: Duration = Duration::from_millis(1500);

/// The codes `spotifast_ios_command` takes, as in NowPlaying.swift.
const PLAY: i32 = 1;
const PAUSE: i32 = 2;
const PLAY_PAUSE: i32 = 3;
const NEXT: i32 = 4;
const PREVIOUS: i32 = 5;
/// Move to a position, in seconds from the start of the track.
const SET_POSITION: i32 = 6;

unsafe extern "C" {
    /// Turns the remote commands on, sending each to `spotifast_ios_command`.
    safe fn spotifast_ios_now_playing_start();
    /// Shows a track. `artwork` is an image file, or empty. The strings are
    /// copied before it returns.
    fn spotifast_ios_now_playing_update(
        title: *const c_char,
        artist: *const c_char,
        album: *const c_char,
        artwork: *const c_char,
        duration: f64,
        position: f64,
        playing: bool,
    );
    /// Shows nothing.
    safe fn spotifast_ios_now_playing_clear();
}

/// What the remote commands share with the app.
struct Shared {
    commands: Vec<Command>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The track shown, which a position from the scrubber belongs to.
    track_id: Option<String>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    commands: Vec::new(),
    wake: None,
    track_id: None,
});

fn shared() -> MutexGuard<'static, Shared> {
    SHARED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Now Playing and the remote commands, for as long as the app keeps this.
pub struct NowPlaying {
    /// What was last shown, and when.
    published: Option<(State, Instant)>,
    /// Playback jumped: the next update shows the new position at once.
    seeked: Cell<bool>,
}

impl std::fmt::Debug for NowPlaying {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NowPlaying").finish_non_exhaustive()
    }
}

impl NowPlaying {
    /// Turns the remote commands on. `wake` is called whenever a command
    /// arrives, so the app reads [`commands`](Self::commands). Call it on
    /// the main thread.
    pub fn start(_app: App, wake: impl Fn() + Send + Sync + 'static) -> Self {
        shared().wake = Some(Arc::new(wake));
        spotifast_ios_now_playing_start();
        Self {
            published: None,
            seeked: Cell::new(false),
        }
    }

    /// The commands that arrived since the last call, oldest first.
    pub fn commands(&self) -> Vec<Command> {
        std::mem::take(&mut shared().commands)
    }

    /// Shows `state`. Cheap to call every pass: a change is sent at once,
    /// and the position only when it strays from where iOS has it.
    pub fn update(&mut self, state: State) {
        let now = Instant::now();
        if !self.due(&state, now) {
            return;
        }
        publish(&state);
        self.published = Some((state, now));
        self.seeked.set(false);
    }

    /// Playback jumped to `position`, by a seek rather than by playing.
    pub fn seeked(&self, _position: Duration) {
        self.seeked.set(true);
    }

    fn due(&self, state: &State, now: Instant) -> bool {
        let Some((published, at)) = &self.published else {
            return true;
        };
        if self.seeked.get()
            || published.playback != state.playback
            || published.track != state.track
        {
            return true;
        }
        let expected = if published.playback == Playback::Playing {
            published.position + now.duration_since(*at)
        } else {
            published.position
        };
        state.position.abs_diff(expected) > DRIFT
    }
}

/// Sends `state` to the Swift side.
fn publish(state: &State) {
    let Some(track) = &state.track else {
        shared().track_id = None;
        spotifast_ios_now_playing_clear();
        return;
    };
    shared().track_id = Some(track.id.clone());
    let title = text(&track.title);
    let artist = text(&track.artists.join(", "));
    let album = text(&track.album);
    let artwork = track
        .art_file
        .as_ref()
        .map(|file| file.to_string_lossy().into_owned())
        .unwrap_or_default();
    let artwork = text(&artwork);
    let duration = track.duration.map_or(0.0, |time| time.as_secs_f64());
    let playing = state.playback == Playback::Playing;
    // SAFETY: valid C strings, which the Swift side copies before it
    // returns.
    unsafe {
        spotifast_ios_now_playing_update(
            title.as_ptr(),
            artist.as_ptr(),
            album.as_ptr(),
            artwork.as_ptr(),
            duration,
            state.position.as_secs_f64(),
            playing,
        );
    }
}

/// The background self-test: a track that is not one, to see Now Playing
/// take it while the app is in the background.
pub fn show_test_track() {
    publish(&State {
        playback: Playback::Playing,
        track: Some(Track {
            id: "spotifast:selftest".into(),
            title: "Spotifast self-test".into(),
            artists: vec!["Spotifast".into()],
            album: "Simulator".into(),
            duration: Some(Duration::from_secs(180)),
            ..Track::default()
        }),
        position: Duration::from_secs(5),
        ..State::default()
    });
}

/// `value` as a C string, without any NUL it holds.
fn text(value: &str) -> CString {
    CString::new(value.replace('\0', "")).unwrap_or_default()
}

/// A remote command arrived (`spotifast_ios_command`, main thread): it waits
/// for the app's next pass, which it wakes.
pub fn command(code: i32, value: f64) {
    let mut shared = shared();
    let command = match code {
        PLAY => Command::Play,
        PAUSE => Command::Pause,
        PLAY_PAUSE => Command::PlayPause,
        NEXT => Command::Next,
        PREVIOUS => Command::Previous,
        SET_POSITION => {
            let Some(track_id) = shared.track_id.clone() else {
                return;
            };
            let Ok(at) = Duration::try_from_secs_f64(value) else {
                return;
            };
            Command::SetPosition {
                track_id,
                position: at,
            }
        }
        _ => {
            log::warn!("now playing: unknown remote command {code}");
            return;
        }
    };
    shared.commands.push(command);
    let wake = shared.wake.clone();
    drop(shared);
    if let Some(wake) = wake {
        wake();
    }
}
