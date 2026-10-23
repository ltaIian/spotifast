//! Look-ahead soft-knee limiter for final audio output.
//!
//! Equalizer and preamp boosts can exceed full scale. Hard clipping would add
//! audible harmonics, so this limiter reduces gain before the output:
//!
//! - Both channels use the same gain to preserve the stereo image.
//! - Fast attack and slow release catch transients without pumping.
//! - A soft knee introduces gain reduction gradually.
//! - A -1 dBFS threshold leaves room for inter-sample peaks after resampling.
//! - Eight milliseconds of look-ahead lets gain settle before a peak arrives,
//!   and the gain answers to the loudest frame still held, so a crest longer
//!   than the look-ahead (bass below about 60 Hz) cannot outlast it.
//!
//! Samples below the knee pass unchanged. A final clamp only guards against
//! numerical overshoot.

/// Limiting threshold, below full scale to allow for inter-sample peaks.
///
/// With [`KNEE_DB`], unity gain begins at -2 dBFS. This matches librespot's
/// normalisation threshold, so flat normalized audio passes unchanged. Keep
/// both values aligned when changing either one.
const THRESHOLD_DB: f64 = -1.0;
/// Width of the soft knee below the threshold.
const KNEE_DB: f64 = 2.0;
/// Attack and release times in milliseconds.
const ATTACK_MS: f64 = 1.5;
const RELEASE_MS: f64 = 100.0;
/// Look-ahead duration, long enough for the attack to settle.
const LOOKAHEAD_MS: f64 = 8.0;

/// One-pole smoothing coefficient for a time constant in milliseconds:
/// the fraction of the distance left to travel each sample.
fn coefficient(ms: f64, sample_rate: f64) -> f64 {
    if ms <= 0.0 {
        return 1.0;
    }
    1.0 - (-1.0 / (ms / 1000.0 * sample_rate)).exp()
}

fn to_db(linear: f64) -> f64 {
    20.0 * linear.max(1e-12).log10()
}

fn from_db(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Gain reduction for a signal `over_db` above the threshold.
/// Uses a quadratic curve through the knee.
fn reduction_db(over_db: f64) -> f64 {
    if over_db <= -KNEE_DB / 2.0 {
        0.0
    } else if over_db >= KNEE_DB / 2.0 {
        over_db
    } else {
        let above = over_db + KNEE_DB / 2.0;
        above * above / (2.0 * KNEE_DB)
    }
}

/// Limiter state shared across audio blocks.
pub struct Limiter {
    /// Current linear gain. One means no reduction.
    gain: f64,
    attack: f64,
    release: f64,
    /// Delayed frames, oldest first. Initial silence preserves block lengths.
    held: std::collections::VecDeque<[f64; 2]>,
    /// The frames still to go out, the incoming one included, that are
    /// louder than every frame after them: (frame number, louder channel),
    /// oldest first, so the first is the loudest of the look-ahead (a
    /// sliding-window maximum). Room for the whole window is made in `new`.
    loudest: std::collections::VecDeque<(u64, f64)>,
    /// Frames that have arrived, which numbers them.
    arrived: u64,
    /// Frames the look-ahead holds.
    lookahead: u64,
}

impl Limiter {
    pub fn new(sample_rate: f64) -> Self {
        let lookahead = ((LOOKAHEAD_MS / 1000.0 * sample_rate).round() as usize).max(1);
        // Room for the incoming frame as well, so that no block allocates.
        let mut held = std::collections::VecDeque::with_capacity(lookahead + 1);
        held.resize(lookahead, [0.0; 2]);
        Self {
            gain: 1.0,
            attack: coefficient(ATTACK_MS, sample_rate),
            release: coefficient(RELEASE_MS, sample_rate),
            held,
            loudest: std::collections::VecDeque::with_capacity(lookahead + 1),
            arrived: 0,
            lookahead: lookahead as u64,
        }
    }

    /// Notes the incoming frame's level and returns the loudest level of
    /// the frames still to go out, the incoming one included.
    fn loudest_to_come(&mut self, level: f64) -> f64 {
        let start = self.arrived.saturating_sub(self.lookahead);
        let window = &mut self.loudest;
        // Frames that have gone out no longer count.
        while window.front().is_some_and(|&(frame, _)| frame < start) {
            window.pop_front();
        }
        // Nor do quieter frames ahead of this one: they go out first.
        while window.back().is_some_and(|&(_, held)| held <= level) {
            window.pop_back();
        }
        window.push_back((self.arrived, level));
        self.arrived += 1;
        window.front().map_or(level, |&(_, loudest)| loudest)
    }

    /// Limits interleaved stereo `frames` to `full_scale`.
    ///
    /// `full_scale` may exceed one when output volume is applied later. For
    /// example, at quarter volume a sample level of four reaches full scale.
    pub fn process(&mut self, frames: &mut [f64], full_scale: f64) {
        if !(full_scale.is_finite() && full_scale > 0.0) {
            return;
        }
        let threshold_db = to_db(full_scale) + THRESHOLD_DB;
        let ceiling = full_scale;
        for frame in frames.chunks_mut(2) {
            // Calculate gain from the loudest frame still to go out, the
            // incoming one included, and apply it to the delayed frame. Use
            // the louder channel for both channels. A gain from the incoming
            // frame alone would recover while a long crest was still held.
            let peak = frame
                .iter()
                .fold(0.0f64, |loudest, sample| loudest.max(sample.abs()));
            let loudest = self.loudest_to_come(peak);
            let target = from_db(-reduction_db(to_db(loudest) - threshold_db));
            // Reduce gain quickly and restore it slowly.
            let coefficient = if target < self.gain {
                self.attack
            } else {
                self.release
            };
            self.gain += (target - self.gain) * coefficient;

            self.held
                .push_back([frame[0], frame.get(1).copied().unwrap_or(0.0)]);
            let due = self.held.pop_front().unwrap_or([0.0; 2]);
            for (sample, held) in frame.iter_mut().zip(due) {
                // Guard against numerical overshoot after gain reduction.
                *sample = (held * self.gain).clamp(-ceiling, ceiling);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;

    const RATE: f64 = 44_100.0;
    /// Number of frames held for look-ahead.
    const DELAY: usize = 353;

    fn tone(level: f64, frames: usize) -> Vec<f64> {
        (0..frames * 2).map(|_| level).collect()
    }

    fn peak(samples: &[f64]) -> f64 {
        samples
            .iter()
            .fold(0.0f64, |loudest, sample| loudest.max(sample.abs()))
    }

    /// Samples below the knee pass unchanged.
    #[test]
    fn quiet_sound_passes_through_untouched() {
        let mut limiter = Limiter::new(RATE);
        let mut samples = tone(0.5, DELAY * 2);
        limiter.process(&mut samples, 1.0);
        assert!(
            samples[DELAY * 2..]
                .iter()
                .all(|sample| (sample - 0.5).abs() < 1e-12),
            "half scale is well under the knee and must be left alone"
        );
    }

    /// Output never exceeds full scale, including with a +12 dB preamp.
    #[test]
    fn a_loud_boost_is_held_to_full_scale() {
        let mut limiter = Limiter::new(RATE);
        let mut samples = tone(4.0, 44_100);
        limiter.process(&mut samples, 1.0);
        assert!(peak(&samples) <= 1.0, "nothing may leave above full scale");
    }

    /// Look-ahead reduces gain before the first loud sample reaches output.
    #[test]
    fn a_sudden_transient_is_caught_before_it_arrives() {
        let mut limiter = Limiter::new(RATE);
        let mut samples = tone(0.0, DELAY);
        samples.extend(tone(4.0, DELAY * 2));
        limiter.process(&mut samples, 1.0);
        // Exponential smoothing approaches but does not reach its target. The
        // first sample may slightly exceed the threshold but not the ceiling.
        let loudest = peak(&samples);
        assert!(
            loudest < 0.95,
            "the gain was down in time and the clamp never fired: {loudest}"
        );
        assert!(
            loudest > from_db(THRESHOLD_DB),
            "and it really did have to limit: {loudest}"
        );
    }

    /// Sustained output settles at the threshold, leaving resampling headroom.
    #[test]
    fn it_settles_a_decibel_under_the_ceiling() {
        let mut limiter = Limiter::new(RATE);
        let mut samples = tone(4.0, 44_100);
        limiter.process(&mut samples, 1.0);
        let settled = peak(&samples[80_000..]);
        let wanted = from_db(THRESHOLD_DB);
        assert!(
            (settled - wanted).abs() < 0.01,
            "settled at {settled}, wanted about {wanted}"
        );
    }

    /// The ceiling accounts for volume applied after the limiter.
    #[test]
    fn a_volume_still_to_come_raises_the_ceiling() {
        let mut limiter = Limiter::new(RATE);
        let mut samples = tone(1.0, DELAY * 2);
        limiter.process(&mut samples, 4.0);
        assert!(
            samples[DELAY * 2..]
                .iter()
                .all(|sample| (sample - 1.0).abs() < 1e-12),
            "a quarter volume leaves room for four times the signal"
        );
    }

    /// Both channels use the same gain, preserving the stereo image.
    #[test]
    fn the_stereo_image_does_not_move() {
        let mut limiter = Limiter::new(RATE);
        // Only the left channel peaks.
        let mut samples: Vec<f64> = (0..DELAY * 4).flat_map(|_| [4.0, 0.25]).collect();
        limiter.process(&mut samples, 1.0);
        for frame in samples[DELAY * 2..].chunks(2) {
            let ratio = frame[0] / frame[1];
            assert!(
                (ratio - 16.0).abs() < 1e-9,
                "the two channels kept their proportion: {ratio}"
            );
        }
    }

    /// Gain recovers slowly after a loud passage.
    #[test]
    fn it_lets_go_slowly() {
        let mut limiter = Limiter::new(RATE);
        limiter.process(&mut tone(4.0, 4410), 1.0);
        let held = limiter.gain;
        assert!(held < 0.3, "a four-times signal is well turned down");
        // After 100 ms of quiet, gain is recovering but has not reached unity.
        limiter.process(&mut tone(0.1, 441), 1.0);
        assert!(limiter.gain > held, "it recovers");
        assert!(limiter.gain < 1.0, "but not all at once");
    }

    fn sine(hz: f64, frames: usize) -> Vec<f64> {
        (0..frames)
            .flat_map(|i| {
                let sample = (TAU * hz * i as f64 / RATE).sin();
                [sample, sample]
            })
            .collect()
    }

    /// The equalizer with the whole bass boost and then the limiter, in
    /// packets, as `vis::Tapped` runs them.
    fn boosted_and_limited(mut samples: Vec<f64>) -> Vec<f64> {
        let eq = crate::eq::shared();
        eq.set(crate::eq::EqSettings {
            on: true,
            bass_boost_db: crate::eq::BASS_BOOST_MAX_DB,
            ..crate::eq::EqSettings::default()
        });
        let mut equalizer = crate::eq::Processor::new(eq);
        let mut limiter = Limiter::new(RATE);
        for packet in samples.chunks_mut(2048) {
            equalizer.process(packet);
            limiter.process(packet, 1.0);
        }
        samples
    }

    /// Full-scale bass under the whole bass boost, its crests longer than
    /// the look-ahead, is held by the gain: nothing reaches the clamp at
    /// the ceiling, and it settles a decibel under it.
    #[test]
    fn boosted_bass_never_reaches_the_clamp() {
        for hz in [40.0, 50.0, 60.0] {
            let samples = boosted_and_limited(sine(hz, 44_100));
            assert!(
                samples.iter().all(|sample| sample.abs() < 1.0),
                "{hz} Hz reached the clamp"
            );
            let settled = peak(&samples[44_100..]);
            assert!(settled < 0.95, "{hz} Hz settled at {settled}");
        }
    }

    /// A boosted kick drum every half second over a quieter bed: each
    /// kick is held by the gain, never flattened by the clamp.
    #[test]
    fn a_boosted_kick_over_a_bed_never_reaches_the_clamp() {
        let music: Vec<f64> = (0..2 * 44_100)
            .flat_map(|i| {
                let since = (i % 22_050) as f64 / RATE;
                let kick = 0.5 * (-since / 0.15).exp() * (TAU * 55.0 * since).sin();
                let bed = 0.25 * (TAU * 220.0 * i as f64 / RATE).sin();
                [kick + bed, kick + bed]
            })
            .collect();
        let samples = boosted_and_limited(music);
        let loudest = peak(&samples);
        assert!(loudest < 0.95, "a kick left at {loudest}");
    }

    /// Output block length always matches input block length.
    #[test]
    fn a_block_leaves_as_long_as_it_arrived() {
        let mut limiter = Limiter::new(RATE);
        for frames in [1, 7, 512, 4410] {
            let mut samples = tone(0.3, frames);
            let before = samples.len();
            limiter.process(&mut samples, 1.0);
            assert_eq!(samples.len(), before, "a block of {frames} frames");
        }
    }
}
