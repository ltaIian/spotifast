# Equaliser and bass boost: the shared interface

This is the contract between the `audio-dsp` agent (filters, processing, persistence) and the `mobile-ui` agent (sliders, presets, the equaliser screen). Both implement exactly what is written here. If either needs a change, it stops and asks the orchestrator; neither edits this file on its own.

Spotifast already has a ten-band equaliser (`src/eq.rs`), a preamp, balance and mono, Winamp's presets plus the app's own, user-saved presets, a Settings page section (`src/ui/settings.rs`, "Equalizer") and the Winamp skin's EQ window (`src/ui/winamp/equalizer.rs`). The task keeps all of that and adds one thing to the engine: **Bass boost**, a low-shelf filter independent of the 60 Hz band that stacks with whatever preset is active.

## 1. Persisted settings (`src/settings.rs`, owned by audio-dsp)

Existing fields stay exactly as they are: `eq_on: bool`, `eq_preamp_db: f32`, `eq_bands_db: [f32; 10]`, `eq_user_presets: Vec<UserEqPreset>`, `balance: f32`, `mono: bool`.

One new field on `Settings`:

```rust
/// Bass boost: a low-shelf below about 90 Hz, 0 to +12 dB, on top of the
/// bands. Independent of the 60 Hz slider and of the preset.
#[serde(default)]
pub eq_bass_boost_db: f32,   // default 0.0
```

- `#[serde(default)]` keeps older `settings.json` files loading (backward compatibility is an AGENTS.md rule).
- Bass boost is **not** part of `UserEqPreset` or of the built-in `Preset`s. Applying a preset leaves it unchanged. Saving a user preset does not record it.
- `Settings::default()` sets it to `0.0`.
- Note for the maintainer-facing policy: this is a new `Settings` field. CONTRIBUTING.md asks for maintainer approval before adding one; the user has explicitly asked for this slider, so it goes in, and the PLAN notes it as a policy exception for the fork.

## 2. Engine types (`src/eq.rs`, owned by audio-dsp)

```rust
pub const BANDS: [f32; 10] = [...];      // unchanged
pub const RANGE_DB: f32 = 12.0;          // unchanged, bands and preamp
pub const BASS_BOOST_HZ: f32 = 90.0;     // shelf midpoint, in the 80 to 100 Hz window
pub const BASS_BOOST_MAX_DB: f32 = 12.0; // slider range is 0.0..=BASS_BOOST_MAX_DB

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqSettings {
    pub on: bool,
    pub preamp_db: f32,
    pub bands_db: [f32; 10],
    pub balance: f32,
    pub mono: bool,
    /// New. 0.0 means the shelf is not in the chain.
    pub bass_boost_db: f32,
}
```

- `EqSettings::default()` has `bass_boost_db: 0.0`.
- `EqSettings::clamped()` clamps `bass_boost_db` to `0.0..=BASS_BOOST_MAX_DB`.
- `EqSettings::curve()` and `EqSettings::response_db(hz)` **include** the shelf, so the drawn response in Settings (`eq_curve`) and the Winamp graph show the boost.
- `bass_boost_db` applies only while `on` is true, like the bands and the preamp. Balance and mono keep applying regardless, as today.

### Shared cell between the UI thread and the player thread

Today `SharedEq` is `Arc<Mutex<EqSettings>>`. It becomes a lock-free cell with this API, so the thread that produces audio never takes a lock:

```rust
pub struct SharedEq(/* private: Arc of an atomic snapshot / seqlock, std only, no new crate */);

impl Clone for SharedEq { ... }            // cheap, shares the cell

pub fn shared() -> SharedEq;               // unchanged name, default settings

impl SharedEq {
    /// UI thread. Replaces the whole settings snapshot.
    pub fn set(&self, settings: EqSettings);
    /// Any thread. Returns the latest snapshot without blocking.
    pub fn get(&self) -> EqSettings;
}
```

- `App::push_eq` (src/app.rs) becomes `self.winamp.eq.set(eq_settings(&self.settings))`. The one test that read `app.winamp.eq.lock().unwrap().bands_db` (src/app.rs, "the sound stays" test near line 21850) reads `.get().bands_db`. audio-dsp makes these two edits in `src/app.rs`; mobile-ui does not touch `push_eq`.
- `crate::app::eq_settings(&Settings) -> EqSettings` (src/app.rs ~line 12851) gains `bass_boost_db: settings.eq_bass_boost_db`. audio-dsp owns this edit.

### Processor (player thread)

```rust
pub struct Processor { ... }
impl Processor {
    pub fn new(shared: SharedEq) -> Self;
    pub fn reset(&mut self);                       // clears filter state, keeps settings
    pub fn process(&mut self, samples: &mut [f64]); // interleaved stereo, 44.1 kHz, in place
}
```

The public surface is unchanged, so `vis::Tapped` (src/vis.rs) keeps calling it as today. Inside:

- **Where it runs.** `Tapped::write` is called on librespot's player thread, once per decoded packet (about 1024 to 2048 frames), before the samples enter rodio's queue. The device callback (cpal, via fastframe-audio, `MixerRender::render` in src/sink.rs) only drains that queue and must gain nothing new. So "the audio thread" for this contract means the player thread, and the rules below apply there. See PLAN.md section 4 for the full stage order (librespot's own normalisation limiter sits upstream of the EQ when normalisation is on).
- **No locks, no allocations in `process`.** Read the snapshot with `SharedEq::get`. Filter chains live in fixed-size arrays (`[Biquad; 10]` bands plus one shelf, per channel), not `Vec`. Coefficients are recomputed only when the snapshot differs from the applied one. The one-off `rebuild` may compute, but may not allocate on the heap.
- **Smoothing.** A change in any gain (preamp, a band, the shelf) must not click. Ramp or crossfade over about 10 to 30 ms (roughly 512 to 1024 frames at 44.1 kHz). The mechanism is audio-dsp's choice (per-stage gain ramps, or a crossfade between the old and new chain), but it must be allocation-free and must be covered by a test that checks for no sample-to-sample jump larger than the signal itself at the moment of change.
- **Bypass.** When `on` is false, or when every band is within 0.05 dB of flat, the preamp is 0 and `bass_boost_db` is 0, no filter runs (the current early return). Balance and mono still apply as today.
- **Bass boost filter.** RBJ Audio EQ Cookbook low-shelf, `f0 = BASS_BOOST_HZ`, shelf slope `S = 1.0` (Q = 0.707), gain `bass_boost_db`, computed in f64 at `SAMPLE_RATE`. It is a separate biquad after the band chain and before balance/mono, per channel, stored as fixed state (for example `shelf: [Biquad; 2]`) that `Processor::reset()` clears like the bands. It does not feed the band solver in `chain()`; the 60 Hz slider and the shelf are independent by construction. The `vis.rs` processing-reset test gets a `bass_boost_db = 12.0` case so a shelf left out of `reset()` is caught.
- **Clipping.** The existing final limiter (`src/limiter.rs`, run in `Tapped` after volume, with the ceiling computed from the output volume) stays the safety net, so a +12 dB shelf cannot clip the output. No automatic pre-gain is added; the preamp slider remains the user's manual headroom control. audio-dsp confirms with a test that a full-scale 60 Hz tone with +12 dB boost leaves `Tapped`'s limiter within the ceiling.

### Tests audio-dsp adds to `src/eq.rs`

In the style of the existing tests (tones, RMS, `(x - y).abs() < tolerance`):

1. Flat settings with the switch on and `bass_boost_db = 0.0` leave samples bit-identical (unity).
2. `bass_boost_db = 12.0` alone raises a 40 Hz tone by 12 dB ± 1 dB, raises a 90 Hz tone by about 6 dB (± 1.5 dB, the shelf midpoint), and moves a 1 kHz tone by less than 0.5 dB.
3. The shelf stacks: `bands_db[0] = 12.0` plus `bass_boost_db = 12.0` gives more low-end gain than either alone, and `response_db(60.0)` reflects both.
4. `clamped()` clamps `bass_boost_db` to `0.0..=BASS_BOOST_MAX_DB`.
5. Changing the boost mid-stream produces no click (the smoothing test above).
6. `SharedEq::set` on one thread and `get` on another never observe a torn snapshot (spawn a writer thread toggling between two distinct full snapshots; the reader only ever sees one of the two).

## 3. Actions (`src/model.rs`, `src/app.rs`, owned by mobile-ui)

One new variant next to the existing EQ actions in the `Action` enum (`src/model.rs`, near `SetEqBand(usize, f32)` at about line 1327):

```rust
/// The bass boost slider, in decibels, 0 to BASS_BOOST_MAX_DB.
SetEqBassBoost(f32),
```

Handled in `App` next to `Action::SetEqPreamp` (src/app.rs, about line 11654):

```rust
Action::SetEqBassBoost(gain_db) => {
    self.settings.eq_bass_boost_db = gain_db.clamp(0.0, crate::eq::BASS_BOOST_MAX_DB);
    self.push_eq();
}
```

Existing actions keep their meaning: `ToggleEq`, `SetEqBand(band, db)`, `SetEqPreamp(db)`, `ApplyEqPreset(index)`, `ApplyUserEqPreset(name)`, `SaveEqPreset(name)`, `DeleteEqPreset(name)`, `SetBalance`, `ToggleMono`.

## 4. The slider in the interface (owned by mobile-ui)

- **Settings page** (`src/ui/settings.rs`, the "Equalizer" section): one horizontal slider labelled "Bass boost" directly **below** the row of band sliders, range `0.0..=BASS_BOOST_MAX_DB`, step 0.5 dB, showing the value as "+N dB" ("0 dB" at rest), disabled-looking when `eq_on` is false (same `on` treatment as `eq_slider`). Emits `Action::SetEqBassBoost`. Add the words "Bass boost" to the section's search keywords (`RowText`) so Settings search finds it. The existing `eq_curve` draws `settings.curve()`, so it shows the shelf for free; its display clamps at ±12 dB, which is fine. Under the compact breakpoint the band row (470 pt unwrapped today) wraps.
- **Narrow / iOS equaliser screen** (new, mobile-ui, `Page::Equalizer`): the same controls as the Settings section, laid out for a phone: the on/off switch, the preset chips, the curve, **all ten band sliders in a wrapped grid (two rows of five, each column at least 44 pt wide)**, and the Bass boost slider below them. It reads `app.settings.eq_*` and emits the same actions, reusing `eq_slider`, `eq_curve`, `eq_preset_controls`, `widgets::switch` and `widgets::chips` rather than redrawing them. It must be reachable from the phone navigation without the Winamp skin.
- **Winamp skin EQ window**: unchanged. Its sprite layout has no room, and its graph is a spline through the ten slider positions (not the real response), so it stays bands-only on purpose; bass boost still applies to the sound while that window is open.
- Presets: the existing list (Winamp's 18 followed by the app's own, then user presets) stays. The user's requested Spotify names already present: Flat, Classical, Dance, Pop, Rock, Bass Booster, Vocal Booster. Not added in this task: Acoustic, Electronic, Hip-Hop, Jazz, R&B (the user said to keep the existing style when an equaliser already exists).

## 5. Behavioural rules both sides rely on

- All values persist between launches through `Settings` (the dirty flag set by `push_eq`).
- Bass boost is independent of presets and of the 60 Hz band: applying a preset never changes it; moving it never changes a band.
- The drawn response curve includes the shelf when `bass_boost_db > 0`.
- Nothing here changes desktop behaviour when `eq_bass_boost_db` is 0: the chain is bit-identical to today's output in that case (test 1).
