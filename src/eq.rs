//! Ten-band equalizer and bass boost for local playback.
//!
//! Each Winamp band uses a second-order peaking filter. Bands and preamp range
//! from -12 to +12 dB. The bass boost is a low shelf after the bands, 0 to
//! +12 dB, apart from the 60 Hz band and from the presets.
//!
//! The UI writes settings into a cell the player reads once per packet
//! without a lock, solving the bands again only after a change. A change of
//! any gain, the switch included, arrives over about 23 ms so that it never
//! clicks; while the settings shape nothing, no filter runs at all.
//!
//! This stage does not clip boosted samples. `vis::Tapped` limits the signal
//! later, after accounting for output volume.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};
use std::sync::{Arc, Mutex, PoisonError};

use librespot_playback::{NUM_CHANNELS, SAMPLE_RATE};

/// The centre frequencies, Winamp's, in hertz.
pub const BANDS: [f32; 10] = [
    60.0, 170.0, 310.0, 600.0, 1000.0, 3000.0, 6000.0, 12000.0, 14000.0, 16000.0,
];
/// How far a band goes either way, in decibels.
pub const RANGE_DB: f32 = 12.0;
/// Bands closer to flat than this count as flat and are left out.
const FLAT: f32 = 0.05;
/// How far the solved gains may go past the sliders while making the
/// combined response meet them; a guard, not a target.
const SOLVED_LIMIT: f64 = 36.0;
/// The bass boost shelf's corner, in hertz: between the 60 Hz and 170 Hz
/// bands, so the boost is its own control and not a second 60 Hz slider.
pub const BASS_BOOST_HZ: f32 = 90.0;
/// How far the bass boost goes, in decibels; it only ever adds.
pub const BASS_BOOST_MAX_DB: f32 = 12.0;
/// Frames a change takes to arrive, about 23 ms: long enough not to click,
/// short enough to follow a slider.
const RAMP: usize = 1024;
/// Frames between new coefficients while a change arrives; the preamp moves
/// every frame. `RAMP` is a whole number of them.
const STEP: usize = 32;
/// Frames the filters keep running once a change has arrived at flat, so
/// that what they still hold of the sound before dies away (some thirty
/// time constants of the slowest band) before the samples pass untouched.
const TAIL: usize = 4096;
/// The interleaved channels, each with filters of its own.
const CHANNELS: usize = NUM_CHANNELS as usize;

/// Equalizer, preamp, balance, and mono settings.
///
/// Balance and mono apply even when the equalizer is off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqSettings {
    pub on: bool,
    /// Twelve decibels either way, like the bands.
    pub preamp_db: f32,
    pub bands_db: [f32; 10],
    /// -1 is all left, 1 all right.
    pub balance: f32,
    pub mono: bool,
    /// A low shelf at `BASS_BOOST_HZ` on top of the bands, 0 to
    /// `BASS_BOOST_MAX_DB`. Like the bands, it applies only while `on`.
    pub bass_boost_db: f32,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            on: false,
            preamp_db: 0.0,
            bands_db: [0.0; 10],
            balance: 0.0,
            mono: false,
            bass_boost_db: 0.0,
        }
    }
}

impl EqSettings {
    /// The same settings kept within what the equalizer can do.
    pub fn clamped(mut self) -> Self {
        self.preamp_db = self.preamp_db.clamp(-RANGE_DB, RANGE_DB);
        for band in &mut self.bands_db {
            *band = band.clamp(-RANGE_DB, RANGE_DB);
        }
        self.balance = self.balance.clamp(-1.0, 1.0);
        self.bass_boost_db = self.bass_boost_db.clamp(0.0, BASS_BOOST_MAX_DB);
        self
    }

    /// Per-channel balance gains. The quieter side is attenuated.
    pub fn channel_gains(&self) -> [f64; 2] {
        let balance = f64::from(self.balance);
        [(1.0 - balance).min(1.0), (1.0 + balance).min(1.0)]
    }

    /// Filter response used for playback and the curve display, the bass
    /// boost included.
    pub fn curve(&self) -> Curve {
        let boost = f64::from(self.bass_boost_db);
        Curve {
            preamp_db: self.preamp_db,
            bands: chain(&self.bands_db),
            shelf: (boost != 0.0).then(|| boost_shelf(boost)),
        }
    }

    /// The response at one frequency, in decibels, with the switch on.
    pub fn response_db(&self, hz: f32) -> f32 {
        self.curve().db_at(hz)
    }
}

/// Equalizer response by frequency.
pub struct Curve {
    preamp_db: f32,
    /// Each band's filter, `None` for a band left out.
    bands: [Option<Biquad>; 10],
    /// The bass boost's shelf, `None` while there is no boost.
    shelf: Option<Biquad>,
}

impl Curve {
    pub fn db_at(&self, hz: f32) -> f32 {
        let mut db = f64::from(self.preamp_db);
        for filter in self.bands.iter().flatten().chain(&self.shelf) {
            db += filter.gain_db_at(f64::from(hz));
        }
        db as f32
    }
}

/// Band widths in octaves, based on the midpoint between adjacent bands.
///
/// The bands are unevenly spaced. A shared width makes the high bands overlap
/// and overboost presets such as Full Treble. The outer bands extend three
/// octaves below 60 Hz and one octave above 16 kHz, matching Winamp.
fn band_widths() -> [f64; 10] {
    let octaves = BANDS.map(|hz| f64::from(hz).log2());
    let mut widths = [0.0; 10];
    for (index, width) in widths.iter_mut().enumerate() {
        let below = index
            .checked_sub(1)
            .map_or(octaves[index] - 3.0, |i| octaves[i]);
        let above = octaves
            .get(index + 1)
            .copied()
            .unwrap_or(octaves[index] + 1.0);
        *width = (octaves[index] - below) / 2.0 + (above - octaves[index]) / 2.0;
    }
    widths
}

/// Builds filters whose combined response matches each slider at its centre,
/// `None` for a band left out.
fn chain(bands_db: &[f32; 10]) -> [Option<Biquad>; 10] {
    let widths = band_widths();
    filters_for(&solved(bands_db, &widths), &widths)
}

/// The gain each band's filter is given so that together they meet every
/// slider at its centre, zero for a band left out. Adjacent filters overlap,
/// so their individual gains are solved together. It works in arrays only,
/// so the player's thread can run it without touching the heap.
fn solved(bands_db: &[f32; 10], widths: &[f64; 10]) -> [f64; 10] {
    let target: [f64; 10] = bands_db.map(f64::from);
    if target.iter().all(|gain| gain.abs() <= f64::from(FLAT)) {
        return [0.0; 10];
    }
    // How much each band moves every centre, per decibel it is given.
    let mut unit = [[0.0; 10]; 10];
    for (j, (hz, width)) in BANDS.iter().zip(widths).enumerate() {
        let filter = Biquad::peaking(f64::from(*hz), *width, 1.0);
        for (i, centre) in BANDS.iter().enumerate() {
            unit[i][j] = filter.gain_db_at(f64::from(*centre));
        }
    }
    let mut gains = target;
    for _ in 0..6 {
        let filters = filters_for(&gains, widths);
        let mut residual = [0.0; 10];
        for (i, centre) in BANDS.iter().enumerate() {
            let played: f64 = filters
                .iter()
                .flatten()
                .map(|filter| filter.gain_db_at(f64::from(*centre)))
                .sum();
            residual[i] = played - target[i];
        }
        if residual.iter().all(|r| r.abs() < 0.01) {
            break;
        }
        let correction = solve(unit, residual);
        for (gain, step) in gains.iter_mut().zip(correction) {
            *gain = (*gain - step).clamp(-SOLVED_LIMIT, SOLVED_LIMIT);
        }
    }
    for gain in &mut gains {
        if gain.abs() <= f64::from(FLAT) {
            *gain = 0.0;
        }
    }
    gains
}

/// The filter for each band worth running, `None` for one left out.
fn filters_for(gains: &[f64; 10], widths: &[f64; 10]) -> [Option<Biquad>; 10] {
    std::array::from_fn(|band| {
        let hz = f64::from(BANDS[band]);
        let gain = gains[band];
        if gain.abs() > f64::from(FLAT) {
            Some(Biquad::peaking(hz, widths[band], gain))
        } else {
            None
        }
    })
}

/// Gaussian elimination with partial pivoting, for the ten-by-ten
/// interaction of the bands.
fn solve(mut matrix: [[f64; 10]; 10], mut rhs: [f64; 10]) -> [f64; 10] {
    let n = 10;
    for column in 0..n {
        let pivot = (column..n)
            .max_by(|a, b| {
                matrix[*a][column]
                    .abs()
                    .total_cmp(&matrix[*b][column].abs())
            })
            .unwrap_or(column);
        matrix.swap(column, pivot);
        rhs.swap(column, pivot);
        let lead = matrix[column][column];
        if lead.abs() < 1e-12 {
            continue;
        }
        let pivot_row = matrix[column];
        let pivot_rhs = rhs[column];
        for row in column + 1..n {
            let factor = matrix[row][column] / lead;
            if factor == 0.0 {
                continue;
            }
            for (cell, above) in matrix[row][column..].iter_mut().zip(&pivot_row[column..]) {
                *cell -= factor * above;
            }
            rhs[row] -= factor * pivot_rhs;
        }
    }
    let mut solution = [0.0; 10];
    for row in (0..n).rev() {
        let mut sum = rhs[row];
        for k in row + 1..n {
            sum -= matrix[row][k] * solution[k];
        }
        let lead = matrix[row][row];
        solution[row] = if lead.abs() < 1e-12 { 0.0 } else { sum / lead };
    }
    solution
}

/// A named set of band gains, as Winamp shipped them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub name: &'static str,
    pub bands_db: [f32; 10],
}

/// Winamp's presets, in its order.
/// How many of `PRESETS` are Winamp's own, in its order; what follows
/// are scenario presets of this app's, shown behind a separator.
pub const WINAMP_PRESET_COUNT: usize = 18;

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "Flat",
        bands_db: [0.0; 10],
    },
    Preset {
        name: "Classical",
        bands_db: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -7.2, -7.2, -7.2, -9.6],
    },
    Preset {
        name: "Club",
        bands_db: [0.0, 0.0, 8.0, 5.6, 5.6, 5.6, 3.2, 0.0, 0.0, 0.0],
    },
    Preset {
        name: "Dance",
        bands_db: [9.6, 7.2, 2.4, 0.0, 0.0, -5.6, -7.2, -7.2, 0.0, 0.0],
    },
    Preset {
        name: "Full Bass",
        bands_db: [-8.0, 9.6, 9.6, 5.6, 1.6, -4.0, -8.0, -10.4, -11.2, -11.2],
    },
    Preset {
        name: "Full Bass & Treble",
        bands_db: [7.2, 5.6, 0.0, -7.2, -4.8, 1.6, 8.0, 11.2, 12.0, 12.0],
    },
    Preset {
        name: "Full Treble",
        bands_db: [-9.6, -9.6, -9.6, -4.0, 2.4, 11.2, 12.0, 12.0, 12.0, 12.0],
    },
    Preset {
        name: "Laptop Speakers / Headphones",
        bands_db: [4.8, 11.2, 5.6, -3.2, -2.4, 1.6, 4.8, 9.6, 12.0, 12.0],
    },
    Preset {
        name: "Large Hall",
        bands_db: [10.4, 10.4, 5.6, 5.6, 0.0, -4.8, -4.8, -4.8, 0.0, 0.0],
    },
    Preset {
        name: "Live",
        bands_db: [-4.8, 0.0, 4.0, 5.6, 5.6, 5.6, 4.0, 2.4, 2.4, 2.4],
    },
    Preset {
        name: "Party",
        bands_db: [7.2, 7.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 7.2, 7.2],
    },
    Preset {
        name: "Pop",
        bands_db: [-1.6, 4.8, 7.2, 8.0, 5.6, 0.0, -2.4, -2.4, -1.6, -1.6],
    },
    Preset {
        name: "Reggae",
        bands_db: [0.0, 0.0, 0.0, -5.6, 0.0, 6.4, 6.4, 0.0, 0.0, 0.0],
    },
    Preset {
        name: "Rock",
        bands_db: [8.0, 4.8, -5.6, -8.0, -3.2, 4.0, 8.8, 11.2, 11.2, 11.2],
    },
    Preset {
        name: "Ska",
        bands_db: [-2.4, -4.8, -4.0, 0.0, 4.0, 5.6, 8.8, 9.6, 11.2, 9.6],
    },
    Preset {
        name: "Soft",
        bands_db: [4.8, 1.6, 0.0, -2.4, 0.0, 4.0, 8.0, 9.6, 11.2, 12.0],
    },
    Preset {
        name: "Soft Rock",
        bands_db: [4.0, 4.0, 2.4, 0.0, -4.0, -5.6, -3.2, 0.0, 2.4, 8.8],
    },
    Preset {
        name: "Techno",
        bands_db: [8.0, 5.6, 0.0, -5.6, -4.8, 0.0, 8.0, 9.6, 9.6, 8.8],
    },
    Preset {
        name: "Bass Booster",
        bands_db: [8.8, 7.2, 5.6, 3.2, 0.8, 0.0, 0.0, 0.0, 0.0, 0.0],
    },
    Preset {
        name: "Bass Reducer",
        bands_db: [-8.8, -7.2, -5.6, -3.2, -0.8, 0.0, 0.0, 0.0, 0.0, 0.0],
    },
    Preset {
        name: "Treble Booster",
        bands_db: [0.0, 0.0, 0.0, 0.0, 0.0, 0.8, 3.2, 5.6, 7.2, 8.8],
    },
    Preset {
        name: "Vocal Booster",
        bands_db: [-2.4, -4.8, -4.8, 1.6, 5.6, 5.6, 4.0, 1.6, 0.0, -2.4],
    },
    Preset {
        name: "Small Speakers",
        bands_db: [-8.0, -6.4, -4.0, -1.6, 1.6, 3.2, 4.8, 5.6, 5.6, 5.6],
    },
    Preset {
        name: "Spoken Word",
        bands_db: [-3.2, -0.8, 0.0, 0.8, 4.0, 5.6, 4.8, 2.4, 0.8, 0.0],
    },
    Preset {
        name: "Loudness",
        bands_db: [9.6, 6.4, 0.0, 0.0, -2.4, 0.0, -1.6, 0.0, 8.0, 1.6],
    },
    Preset {
        name: "Night Listening",
        bands_db: [-4.8, -3.2, -1.6, 0.8, 2.4, 3.2, 2.4, 0.8, -1.6, -3.2],
    },
];

/// Words in a settings snapshot: the switch, mono, the preamp, the balance
/// and the bass boost, then the ten bands, each float by its bits.
const WORDS: usize = 15;
/// Reads the player's thread tries, each having met a write under way,
/// before it keeps the settings it has until the next packet.
const READ_ATTEMPTS: usize = 64;

/// The settings as the window and the player's thread share them.
///
/// A sequence lock over atomic words. A write makes the sequence odd, stores
/// the words and makes it even again; a read that finds the same even
/// sequence before and after taking the words took one whole snapshot.
/// Writers take turns behind a mutex. Readers never lock or wait for one, so
/// the player's thread never stalls behind the window.
#[derive(Clone)]
pub struct SharedEq(Arc<SnapshotCell>);

struct SnapshotCell {
    sequence: AtomicU64,
    words: [AtomicU32; WORDS],
    /// Lets one writer in at a time; readers never touch it.
    writer: Mutex<()>,
}

pub fn shared() -> SharedEq {
    SharedEq(Arc::new(SnapshotCell {
        sequence: AtomicU64::new(0),
        words: encode(&EqSettings::default()).map(AtomicU32::new),
        writer: Mutex::new(()),
    }))
}

impl SharedEq {
    /// Replaces the whole settings snapshot. For the UI thread.
    pub fn set(&self, settings: EqSettings) {
        let cell = &*self.0;
        let _turn = cell.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let sequence = &cell.sequence;
        let start = sequence.load(Ordering::Relaxed);
        sequence.store(start.wrapping_add(1), Ordering::Relaxed);
        // The words below cannot be seen before the odd sequence.
        fence(Ordering::Release);
        for (word, value) in cell.words.iter().zip(encode(&settings)) {
            word.store(value, Ordering::Relaxed);
        }
        sequence.store(start.wrapping_add(2), Ordering::Release);
    }

    /// The latest snapshot, from any thread. It never takes a lock: a read
    /// that meets a write under way reads again.
    pub fn get(&self) -> EqSettings {
        loop {
            if let Some(settings) = self.try_get() {
                return settings;
            }
            std::thread::yield_now();
        }
    }

    /// The latest snapshot, or `None` when each of `READ_ATTEMPTS` reads met
    /// a write under way. What the player's thread reads with: a bounded
    /// number of tries, and no lock.
    fn try_get(&self) -> Option<EqSettings> {
        let cell = &*self.0;
        for _ in 0..READ_ATTEMPTS {
            let before = cell.sequence.load(Ordering::Acquire);
            if before.is_multiple_of(2) {
                let words = cell.words.each_ref().map(|w| w.load(Ordering::Relaxed));
                // The words above cannot be read after the sequence below.
                fence(Ordering::Acquire);
                if cell.sequence.load(Ordering::Relaxed) == before {
                    return Some(decode(words));
                }
            }
            std::hint::spin_loop();
        }
        None
    }
}

impl std::fmt::Debug for SharedEq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedEq").field(&self.get()).finish()
    }
}

/// The settings as words: the switch, mono, the preamp, the balance, the
/// bass boost, then the bands.
fn encode(settings: &EqSettings) -> [u32; WORDS] {
    let mut words = [0; WORDS];
    words[0] = u32::from(settings.on);
    words[1] = u32::from(settings.mono);
    words[2] = settings.preamp_db.to_bits();
    words[3] = settings.balance.to_bits();
    words[4] = settings.bass_boost_db.to_bits();
    for (word, band) in words[5..].iter_mut().zip(settings.bands_db) {
        *word = band.to_bits();
    }
    words
}

fn decode(words: [u32; WORDS]) -> EqSettings {
    EqSettings {
        on: words[0] != 0,
        preamp_db: f32::from_bits(words[2]),
        bands_db: std::array::from_fn(|band| f32::from_bits(words[5 + band])),
        balance: f32::from_bits(words[3]),
        mono: words[1] != 0,
        bass_boost_db: f32::from_bits(words[4]),
    }
}

/// A second-order section in direct form I: a band, or the bass boost's shelf,
/// on one channel.
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// A peaking filter after the Audio EQ Cookbook, `width` octaves wide
    /// between its half-gain points and normalised by `a0`. The width is
    /// taken the cookbook's digital way, with its `w0 / sin(w0)` term: near
    /// the top of the band the plain analog Q would have come out three
    /// times too narrow, which is what rippled the treble.
    fn peaking(hz: f64, width: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = std::f64::consts::TAU * hz / f64::from(SAMPLE_RATE);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin * ((std::f64::consts::LN_2 / 2.0) * width * w0 / sin).sinh();
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha / a) / a0,
            ..Self::default()
        }
    }

    /// A low shelf after the Audio EQ Cookbook with a shelf slope of one
    /// (a Q of 0.707): `gain_db` well below `hz`, half of it at `hz`, and
    /// nothing far above. Normalised by `a0`.
    fn low_shelf(hz: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = std::f64::consts::TAU * hz / f64::from(SAMPLE_RATE);
        let (sin, cos) = w0.sin_cos();
        // The cookbook's alpha, sin(w0) / 2 * sqrt((A + 1/A) * (1/S - 1) + 2),
        // at S = 1.
        let alpha = sin / std::f64::consts::SQRT_2;
        let root = 2.0 * a.sqrt() * alpha;
        let a0 = (a + 1.0) + (a - 1.0) * cos + root;
        Self {
            b0: a * ((a + 1.0) - (a - 1.0) * cos + root) / a0,
            b1: 2.0 * a * ((a - 1.0) - (a + 1.0) * cos) / a0,
            b2: a * ((a + 1.0) - (a - 1.0) * cos - root) / a0,
            a1: -2.0 * ((a - 1.0) + (a + 1.0) * cos) / a0,
            a2: ((a + 1.0) + (a - 1.0) * cos - root) / a0,
            ..Self::default()
        }
    }

    /// The filter's gain at a frequency, in decibels, from its transfer
    /// function on the unit circle: what is played, including the bend
    /// the bilinear transform puts near the top of the band.
    fn gain_db_at(&self, hz: f64) -> f64 {
        let w = std::f64::consts::TAU * hz / f64::from(SAMPLE_RATE);
        let (sin1, cos1) = w.sin_cos();
        let (sin2, cos2) = (2.0 * w).sin_cos();
        let numerator = (self.b0 + self.b1 * cos1 + self.b2 * cos2).powi(2)
            + (self.b1 * sin1 + self.b2 * sin2).powi(2);
        let denominator = (1.0 + self.a1 * cos1 + self.a2 * cos2).powi(2)
            + (self.a1 * sin1 + self.a2 * sin2).powi(2);
        10.0 * (numerator / denominator).log10()
    }

    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    /// Takes `to`'s coefficients and keeps this filter's history, so the
    /// sound carries on through the change.
    fn retune(&mut self, to: &Self) {
        self.b0 = to.b0;
        self.b1 = to.b1;
        self.b2 = to.b2;
        self.a1 = to.a1;
        self.a2 = to.a2;
    }

    /// Forgets the signal so far.
    fn clear(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    /// Rounds a history too small to hear down to zero, so a long silence
    /// never leaves the filter working on subnormal numbers, which are slow
    /// on some processors.
    fn flush(&mut self) {
        for value in [&mut self.x1, &mut self.x2, &mut self.y1, &mut self.y2] {
            if value.abs() < 1e-30 {
                *value = 0.0;
            }
        }
    }
}

/// The bass boost's filter at `gain_db`.
fn boost_shelf(gain_db: f64) -> Biquad {
    Biquad::low_shelf(f64::from(BASS_BOOST_HZ), gain_db)
}

/// A gain in decibels as a factor.
fn amplitude(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Where the filters are, in decibels: the preamp, the gain each band's
/// filter is given (solved, zero for a band left out) and the bass boost.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shape {
    preamp_db: f64,
    bands_db: [f64; 10],
    boost_db: f64,
}

impl Shape {
    /// Nothing shaped: every filter plays the sound as it comes.
    const FLAT: Self = Self {
        preamp_db: 0.0,
        bands_db: [0.0; 10],
        boost_db: 0.0,
    };

    /// What `settings` ask of the filters: nothing while the switch is off.
    fn of(settings: &EqSettings, widths: &[f64; 10]) -> Self {
        if !settings.on {
            return Self::FLAT;
        }
        Self {
            preamp_db: f64::from(settings.preamp_db),
            bands_db: solved(&settings.bands_db, widths),
            boost_db: f64::from(settings.bass_boost_db),
        }
    }

    /// `progress` of the way from this shape to `to`, each gain moving evenly
    /// in decibels; all the way at one.
    fn towards(&self, to: &Self, progress: f64) -> Self {
        if progress >= 1.0 {
            return *to;
        }
        let mix = |from: f64, end: f64| from + (end - from) * progress;
        Self {
            preamp_db: mix(self.preamp_db, to.preamp_db),
            bands_db: std::array::from_fn(|band| mix(self.bands_db[band], to.bands_db[band])),
            boost_db: mix(self.boost_db, to.boost_db),
        }
    }
}

/// The filters on the player's thread.
///
/// Nothing `process` does takes a lock, waits or touches the heap: the
/// settings come from `SharedEq` without a lock, the filters live in arrays,
/// and the bands are solved again only when the settings change.
pub struct Processor {
    shared: SharedEq,
    /// The settings last read; kept while a read keeps meeting a write.
    applied: EqSettings,
    widths: [f64; 10],
    /// The shape the filters hold, and the change under way: where it set
    /// out from and is going, the frames left until it arrives, and those
    /// left until the filters take their next coefficients.
    now: Shape,
    from: Shape,
    to: Shape,
    ramp: usize,
    block: usize,
    /// Frames left before filters that arrived at flat stop running.
    tail: usize,
    /// Whether the filters run at all; while not, samples pass untouched.
    shaping: bool,
    /// The preamp's gain, and how far it moves each frame.
    gain: f64,
    gain_step: f64,
    /// Each channel's band filters, and its bass boost shelf.
    bands: [[Biquad; 10]; CHANNELS],
    shelves: [Biquad; CHANNELS],
}

impl Processor {
    pub fn new(shared: SharedEq) -> Self {
        let applied = shared.get().clamped();
        let widths = band_widths();
        let shape = Shape::of(&applied, &widths);
        let mut processor = Self {
            shared,
            applied,
            widths,
            now: shape,
            from: shape,
            to: shape,
            ramp: 0,
            block: 0,
            tail: 0,
            shaping: shape != Shape::FLAT,
            gain: amplitude(shape.preamp_db),
            gain_step: 0.0,
            bands: [[Biquad::default(); 10]; CHANNELS],
            shelves: [Biquad::default(); CHANNELS],
        };
        processor.tune(&shape);
        processor
    }

    /// Gives every filter the coefficients for `shape`, keeping each one's
    /// history so the sound carries on.
    fn tune(&mut self, shape: &Shape) {
        for (band, (hz, width)) in BANDS.iter().zip(self.widths).enumerate() {
            let filter = Biquad::peaking(f64::from(*hz), width, shape.bands_db[band]);
            for channel in &mut self.bands {
                channel[band].retune(&filter);
            }
        }
        let shelf = boost_shelf(shape.boost_db);
        for channel in &mut self.shelves {
            channel.retune(&shelf);
        }
    }

    /// Discards the previous signal without changing the filter settings.
    /// A change under way arrives at once, with no sound left to carry it.
    pub fn reset(&mut self) {
        if self.ramp > 0 || self.tail > 0 {
            let to = self.to;
            self.tune(&to);
            self.now = to;
            self.from = to;
            self.ramp = 0;
            self.block = 0;
            self.tail = 0;
            self.gain = amplitude(to.preamp_db);
            self.gain_step = 0.0;
            self.shaping = to != Shape::FLAT;
        }
        self.clear();
    }

    /// Forgets every filter's history.
    fn clear(&mut self) {
        for filter in self.bands.iter_mut().flatten().chain(&mut self.shelves) {
            filter.clear();
        }
    }

    /// Reads the settings and, when they ask something new of the filters,
    /// sets the change off. A read that keeps meeting a write leaves things
    /// as they were until the next packet.
    fn follow(&mut self) {
        let Some(settings) = self.shared.try_get() else {
            return;
        };
        let wanted = settings.clamped();
        if wanted == self.applied {
            return;
        }
        self.applied = wanted;
        let to = Shape::of(&wanted, &self.widths);
        if to == self.to {
            return;
        }
        if !self.shaping {
            // Flat filters with no history play the sound exactly as it
            // comes, so they start from there and the change moves them.
            self.clear();
            self.tune(&Shape::FLAT);
            self.now = Shape::FLAT;
            self.gain = 1.0;
            self.shaping = true;
        }
        self.from = self.now;
        self.to = to;
        self.ramp = RAMP;
        self.block = 0;
        self.tail = 0;
    }

    /// Carries the change under way one frame along. At the start of each
    /// block of `STEP` frames the filters take the coefficients for where
    /// the change is at the block's end, and the preamp sets off for its
    /// gain there, a frame's share at a time.
    fn advance(&mut self) {
        if self.block == 0 {
            let block = self.ramp.min(STEP);
            let progress = (RAMP - self.ramp + block) as f64 / RAMP as f64;
            let at = self.from.towards(&self.to, progress);
            self.tune(&at);
            self.now = at;
            self.gain_step = (amplitude(at.preamp_db) - self.gain) / block as f64;
            self.block = block;
        }
        self.gain += self.gain_step;
        self.block -= 1;
        self.ramp -= 1;
        if self.ramp == 0 {
            self.gain = amplitude(self.to.preamp_db);
            if self.to == Shape::FLAT {
                self.tail = TAIL;
            }
        }
    }

    /// One frame through the preamp, the bands and the shelf.
    fn filter_frame(&mut self, frame: &mut [f64; CHANNELS]) {
        if self.ramp > 0 {
            self.advance();
        } else if self.tail > 0 {
            self.tail -= 1;
            self.shaping = self.tail > 0;
        }
        let gain = self.gain;
        let filters = self.bands.iter_mut().zip(&mut self.shelves);
        for (sample, (bands, shelf)) in frame.iter_mut().zip(filters) {
            let mut y = *sample * gain;
            for band in bands {
                y = band.run(y);
            }
            *sample = shelf.run(y);
        }
    }

    /// Runs interleaved stereo samples through the equalizer, in place.
    pub fn process(&mut self, samples: &mut [f64]) {
        self.follow();
        let gains = self.applied.channel_gains();
        let placing = self.applied.mono || gains != [1.0, 1.0];
        if !self.shaping && !placing {
            return;
        }
        let (frames, _) = samples.as_chunks_mut::<CHANNELS>();
        for frame in frames {
            if self.shaping {
                self.filter_frame(frame);
            }
            if self.applied.mono && frame.len() == 2 {
                let middle = (frame[0] + frame[1]) / 2.0;
                frame[0] = middle;
                frame[1] = middle;
            }
            for (sample, gain) in frame.iter_mut().zip(gains) {
                // No ceiling here. These are floats with room to spare, and
                // the one ceiling in the chain sits at the end, past the
                // volume: a boost that would clip at full volume is fine
                // three notches down, and holding it back here would take
                // that away for good. See `vis::Tapped`.
                *sample *= gain;
            }
        }
        if self.shaping {
            for filter in self.bands.iter_mut().flatten().chain(&mut self.shelves) {
                filter.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, frames: usize) -> Vec<f64> {
        (0..frames)
            .flat_map(|i| {
                let t = i as f64 / f64::from(SAMPLE_RATE);
                let sample = 0.25 * (std::f64::consts::TAU * f64::from(hz) * t).sin();
                [sample, sample]
            })
            .collect()
    }

    fn rms(samples: &[f64]) -> f64 {
        (samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64).sqrt()
    }

    #[test]
    fn a_boosted_band_makes_its_tone_louder_and_leaves_others_alone() {
        let mut settings = EqSettings {
            on: true,
            ..EqSettings::default()
        };
        settings.bands_db[4] = 12.0; // 1 kHz
        let shared = shared();
        shared.set(settings);
        let mut processor = Processor::new(shared);
        let mut at_band = tone(1000.0, 8192);
        let before = rms(&at_band[4096..]);
        processor.process(&mut at_band);
        let after = rms(&at_band[4096..]);
        let gain_db = 20.0 * (after / before).log10();
        assert!((gain_db - 12.0).abs() < 1.0, "1 kHz gained {gain_db:.1} dB");

        let mut far = tone(60.0, 8192);
        let before = rms(&far[4096..]);
        processor.process(&mut far);
        let after = rms(&far[4096..]);
        let gain_db = 20.0 * (after / before).log10();
        assert!(gain_db.abs() < 1.0, "60 Hz moved {gain_db:.1} dB");
    }

    #[test]
    fn off_or_flat_changes_nothing_and_the_preamp_scales() {
        let shared = shared();
        let mut processor = Processor::new(shared.clone());
        let original = tone(440.0, 1024);
        let mut samples = original.clone();
        processor.process(&mut samples);
        assert_eq!(samples, original);

        let mut settings = EqSettings {
            on: true,
            ..EqSettings::default()
        };
        shared.set(settings);
        let mut samples = original.clone();
        processor.process(&mut samples);
        assert_eq!(samples, original);

        // A change arrives over `RAMP` frames, here one packet, so the
        // packet after it plays the new gain.
        settings.preamp_db = 6.0;
        shared.set(settings);
        processor.process(&mut original.clone());
        let mut samples = original.clone();
        processor.process(&mut samples);
        let ratio = rms(&samples) / rms(&original);
        assert!((20.0 * ratio.log10() - 6.0).abs() < 0.1);

        settings.preamp_db = -6.0;
        shared.set(settings);
        processor.process(&mut original.clone());
        let mut samples = original.clone();
        processor.process(&mut samples);
        let ratio = rms(&samples) / rms(&original);
        assert!((20.0 * ratio.log10() + 6.0).abs() < 0.1);
    }

    #[test]
    fn the_drawn_response_peaks_at_the_band_and_adds_the_preamp() {
        let mut settings = EqSettings::default();
        settings.bands_db[7] = 6.0; // 12 kHz
        settings.preamp_db = -3.0;
        assert!((settings.response_db(12_000.0) - 3.0).abs() < 0.1);
        assert!((settings.response_db(100.0) + 3.0).abs() < 0.2);
        let clamped = EqSettings {
            preamp_db: 20.0,
            bands_db: [20.0; 10],
            ..settings
        }
        .clamped();
        assert_eq!(clamped.preamp_db, RANGE_DB);
        assert!(clamped.bands_db.iter().all(|band| *band == RANGE_DB));
    }

    #[test]
    fn balance_turns_one_side_down_and_mono_makes_the_sides_the_same() {
        let shared = shared();
        let mut processor = Processor::new(shared.clone());
        let mut samples = vec![0.5, -0.25, 0.5, -0.25];
        let mut settings = EqSettings {
            balance: 0.5,
            ..EqSettings::default()
        };
        shared.set(settings);
        processor.process(&mut samples);
        assert_eq!(samples, vec![0.25, -0.25, 0.25, -0.25]);

        settings.balance = 0.0;
        settings.mono = true;
        shared.set(settings);
        let mut samples = vec![0.5, -0.25];
        processor.process(&mut samples);
        assert_eq!(samples, vec![0.125, 0.125]);
        assert_eq!(
            EqSettings {
                balance: 3.0,
                ..EqSettings::default()
            }
            .clamped()
            .balance,
            1.0
        );
    }

    /// Every preset, played, meets its sliders at every band's centre,
    /// which the top three bands, a fifth of an octave apart, did not.
    #[test]
    fn what_is_played_meets_the_sliders_at_every_band() {
        for preset in PRESETS {
            let settings = EqSettings {
                on: true,
                bands_db: preset.bands_db,
                ..EqSettings::default()
            };
            let curve = settings.curve();
            for (hz, wanted) in BANDS.iter().zip(preset.bands_db) {
                let got = curve.db_at(*hz);
                assert!(
                    (got - wanted).abs() < 0.3,
                    "{} at {hz} Hz plays {got:.1} dB for {wanted:.1}",
                    preset.name
                );
            }
        }
    }

    /// One slider moves its own band and leaves the neighbours' centres
    /// alone, even at the top where they sit close together.
    #[test]
    fn a_slider_leaves_its_neighbours_centres_alone() {
        let mut settings = EqSettings::default();
        settings.bands_db[8] = 12.0; // 14 kHz
        let curve = settings.curve();
        assert!((curve.db_at(14_000.0) - 12.0).abs() < 0.3);
        assert!(
            curve.db_at(12_000.0).abs() < 0.3,
            "12 kHz moved {:.1}",
            curve.db_at(12_000.0)
        );
        assert!(
            curve.db_at(16_000.0).abs() < 0.3,
            "16 kHz moved {:.1}",
            curve.db_at(16_000.0)
        );
        assert!(curve.db_at(1_000.0).abs() < 0.1);
        // Between two boosted neighbours the response does not sag away.
        settings.bands_db[7] = 12.0; // 12 kHz too
        let curve = settings.curve();
        assert!(
            curve.db_at(13_000.0) > 9.0,
            "13 kHz sags to {:.1}",
            curve.db_at(13_000.0)
        );
    }

    /// The outer sliders reach past their centres: the sub-bass follows
    /// the 60 Hz slider and the air follows the 16 kHz one, as they did.
    #[test]
    fn the_outer_sliders_reach_the_ends() {
        let mut settings = EqSettings::default();
        settings.bands_db[0] = 12.0;
        let curve = settings.curve();
        assert!(
            curve.db_at(30.0) > 6.0,
            "30 Hz gets {:.1}",
            curve.db_at(30.0)
        );
        assert!(curve.db_at(170.0).abs() < 0.3);
        let mut settings = EqSettings::default();
        settings.bands_db[9] = 12.0;
        let curve = settings.curve();
        // A peaking filter is back at 0 dB by the sample rate's top, so
        // half the slider is what 19 kHz can hold.
        assert!(
            curve.db_at(19_000.0) > 5.0,
            "19 kHz gets {:.1}",
            curve.db_at(19_000.0)
        );
        assert!(curve.db_at(14_000.0).abs() < 0.3);
    }

    #[test]
    fn every_preset_stays_within_the_range() {
        assert_eq!(PRESETS[0].name, "Flat");
        for preset in PRESETS {
            assert!(
                preset.bands_db.iter().all(|band| band.abs() <= RANGE_DB),
                "{} leaves the range",
                preset.name
            );
        }
    }

    /// With the switch on and every slider, the preamp and the bass boost at
    /// zero, the sound passes bit for bit; and it does again once a boost
    /// has been taken away and the change has died away.
    #[test]
    fn flat_settings_with_the_switch_on_leave_the_sound_untouched() {
        let flat = EqSettings {
            on: true,
            ..EqSettings::default()
        };
        let shared = shared();
        shared.set(flat);
        let mut processor = Processor::new(shared.clone());
        let original = tone(440.0, 1024);
        let mut samples = original.clone();
        processor.process(&mut samples);
        assert_eq!(samples, original);

        let mut shaped = flat;
        shaped.bands_db[0] = 12.0;
        shaped.bass_boost_db = 12.0;
        shared.set(shaped);
        processor.process(&mut tone(440.0, 4096));
        shared.set(flat);
        processor.process(&mut tone(440.0, RAMP + TAIL));
        let mut samples = original.clone();
        processor.process(&mut samples);
        assert_eq!(samples, original, "once the change is over");
    }

    /// The bass boost alone lifts the deep bass by its full amount and the
    /// shelf's corner by half, leaves the middle alone, and the drawn
    /// response shows what is played.
    #[test]
    fn the_bass_boost_lifts_the_bass_and_leaves_the_middle() {
        let boosted = EqSettings {
            on: true,
            bass_boost_db: 12.0,
            ..EqSettings::default()
        };
        let shared = shared();
        shared.set(boosted);
        let mut processor = Processor::new(shared);
        // A quarter of a second, a whole number of half periods of each tone,
        // after the shelf has settled.
        let mut played = |hz: f32| {
            let mut samples = tone(hz, 22_050);
            let before = rms(&samples[22_050..]);
            processor.process(&mut samples);
            20.0 * (rms(&samples[22_050..]) / before).log10()
        };
        let deep = played(40.0);
        assert!((deep - 12.0).abs() < 1.0, "40 Hz gained {deep:.1} dB");
        let corner = played(90.0);
        assert!((corner - 6.0).abs() < 1.5, "90 Hz gained {corner:.1} dB");
        let middle = played(1000.0);
        assert!(middle.abs() < 0.5, "1 kHz moved {middle:.1} dB");
        for (hz, gained) in [(40.0, deep), (90.0, corner), (1000.0, middle)] {
            let drawn = f64::from(boosted.response_db(hz));
            assert!((drawn - gained).abs() < 0.1, "{hz} Hz: {drawn:.1} dB");
        }
    }

    /// The bass boost is no second 60 Hz slider: with both, the bass rises
    /// by the two together, played and drawn.
    #[test]
    fn the_bass_boost_stacks_on_the_sixty_hertz_band() {
        let mut band = EqSettings {
            on: true,
            ..EqSettings::default()
        };
        band.bands_db[0] = 12.0;
        let boost = EqSettings {
            on: true,
            bass_boost_db: 12.0,
            ..EqSettings::default()
        };
        let mut both = band;
        both.bass_boost_db = 12.0;
        let played = |settings: EqSettings| {
            let shared = shared();
            shared.set(settings);
            let mut processor = Processor::new(shared);
            let mut samples = tone(60.0, 22_050);
            let before = rms(&samples[22_050..]);
            processor.process(&mut samples);
            20.0 * (rms(&samples[22_050..]) / before).log10()
        };
        let (band_alone, boost_alone) = (played(band), played(boost));
        let together = played(both);
        assert!(
            together > band_alone + 6.0 && together > boost_alone + 6.0,
            "60 Hz gains {together:.1} dB with both, {band_alone:.1} and {boost_alone:.1} apart"
        );
        let drawn = both.response_db(60.0);
        let apart = band.response_db(60.0) + boost.response_db(60.0);
        assert!((drawn - apart).abs() < 0.01, "{drawn} against {apart}");
        assert!((f64::from(drawn) - together).abs() < 0.1);
    }

    #[test]
    fn the_bass_boost_is_kept_within_its_range() {
        for (asked, kept) in [(-3.0, 0.0), (20.0, BASS_BOOST_MAX_DB), (6.5, 6.5)] {
            let settings = EqSettings {
                bass_boost_db: asked,
                ..EqSettings::default()
            };
            assert_eq!(settings.clamped().bass_boost_db, kept, "{asked} dB");
        }
    }

    /// The largest step between neighbouring samples of the left channel.
    fn largest_step(samples: &[f64]) -> f64 {
        let left: Vec<f64> = samples.iter().step_by(2).copied().collect();
        let mut largest = 0.0f64;
        for pair in left.windows(2) {
            largest = largest.max((pair[1] - pair[0]).abs());
        }
        largest
    }

    /// A tone played in packets of 1024 frames with the settings changed
    /// between the eighth and the ninth: the largest step between samples
    /// before the change, from it until well after it has arrived, and at
    /// the end.
    fn steps_across(from: EqSettings, to: EqSettings, hz: f32) -> [f64; 3] {
        let shared = shared();
        shared.set(from);
        let mut processor = Processor::new(shared.clone());
        let mut samples = tone(hz, 24 * 1024);
        for (packet, frames) in samples.chunks_mut(2 * 1024).enumerate() {
            if packet == 8 {
                shared.set(to);
            }
            processor.process(frames);
        }
        let change = 2 * 8 * 1024;
        let end = samples.len();
        [
            largest_step(&samples[change - 2 * 4096..change]),
            largest_step(&samples[change - 2..change + 2 * (RAMP + TAIL + 1024)]),
            largest_step(&samples[end - 2 * 4096..]),
        ]
    }

    /// Moving the bass boost, the preamp or a band, or turning the switch
    /// either way, mid-stream never steps from one sample to the next by
    /// more than the tone does on its own before or after: no click.
    #[test]
    fn a_change_mid_stream_does_not_click() {
        let on = EqSettings {
            on: true,
            ..EqSettings::default()
        };
        let mut boosted = on;
        boosted.bass_boost_db = 12.0;
        let mut louder = on;
        louder.preamp_db = 12.0;
        let mut quieter = on;
        quieter.preamp_db = -12.0;
        let mut band = on;
        band.bands_db[4] = 12.0; // 1 kHz
        let mut loud = boosted;
        loud.preamp_db = 6.0;
        loud.bands_db[0] = 12.0;
        let mut off = loud;
        off.on = false;
        let changes = [
            ("the bass boost up", on, boosted, 40.0),
            ("the bass boost down", boosted, on, 40.0),
            ("the preamp up", on, louder, 1000.0),
            ("the preamp down", on, quieter, 1000.0),
            ("a band", on, band, 1000.0),
            ("the switch off", loud, off, 1000.0),
            ("the switch off", loud, off, 40.0),
            ("the switch on", off, loud, 1000.0),
            ("the switch on", off, loud, 40.0),
        ];
        for (change, from, to, hz) in changes {
            let [before, across, after] = steps_across(from, to, hz);
            assert!(
                across <= 1.25 * before.max(after),
                "{change} at {hz} Hz steps {across:.4}, the tone {before:.4} and {after:.4}"
            );
        }
    }

    /// One thread writes two whole snapshots in turn while another reads:
    /// every read is one of the two, never part of each.
    #[test]
    fn a_read_never_sees_half_a_write() {
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        let first = EqSettings {
            on: true,
            preamp_db: -6.0,
            bands_db: [3.0; 10],
            balance: -0.5,
            mono: false,
            bass_boost_db: 12.0,
        };
        let second = EqSettings {
            on: false,
            preamp_db: 6.0,
            bands_db: [-3.0; 10],
            balance: 0.5,
            mono: true,
            bass_boost_db: 0.5,
        };
        let shared = shared();
        shared.set(first);
        let stop = Arc::new(AtomicBool::new(false));
        let writes = Arc::new(AtomicUsize::new(0));
        let writer = {
            let shared = shared.clone();
            let stop = Arc::clone(&stop);
            let writes = Arc::clone(&writes);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    shared.set(second);
                    shared.set(first);
                    writes.fetch_add(2, Ordering::Relaxed);
                    // A breath between writes, so reads also land between
                    // them and not only across them.
                    for _ in 0..64 {
                        std::hint::spin_loop();
                    }
                }
            })
        };
        let mut reads = 0;
        while reads < 20_000 || writes.load(Ordering::Relaxed) < 20_000 {
            let seen = shared.get();
            assert!(seen == first || seen == second, "a torn read: {seen:?}");
            if let Some(seen) = shared.try_get() {
                assert!(seen == first || seen == second, "a torn read: {seen:?}");
            }
            reads += 1;
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
    }
}
