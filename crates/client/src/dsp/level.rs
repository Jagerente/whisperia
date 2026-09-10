use std::sync::atomic::{AtomicU32, Ordering};

pub const SILENCE_DB: f32 = -90.0;
const TALK_THRESHOLD_DB: f32 = -42.0;

const TALK_HANGOVER_FRAMES: u32 = 30;
const FALL_DB_PER_FRAME: f32 = 0.6;

pub fn rms_db(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return SILENCE_DB;
    }
    let mean_square = frame.iter().map(|x| x * x).sum::<f32>() / frame.len() as f32;
    (10.0 * mean_square.max(1e-12).log10()).max(SILENCE_DB)
}

#[derive(Debug, Clone, Copy)]
pub struct Meter {
    db: f32,
    hangover: u32,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            db: SILENCE_DB,
            hangover: 0,
        }
    }
}

impl Meter {
    pub fn feed(&mut self, frame: &[f32]) {
        self.feed_db(rms_db(frame));
    }

    pub fn feed_silence(&mut self) {
        self.feed_db(SILENCE_DB);
    }

    fn feed_db(&mut self, now: f32) {
        self.db = now.max(self.db - FALL_DB_PER_FRAME).max(SILENCE_DB);
        if now > TALK_THRESHOLD_DB {
            self.hangover = TALK_HANGOVER_FRAMES;
        } else {
            self.hangover = self.hangover.saturating_sub(1);
        }
    }

    pub fn db(&self) -> f32 {
        self.db
    }

    pub fn talking(&self) -> bool {
        self.hangover > 0
    }
}

#[derive(Debug)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_sine_is_about_minus_three_db() {
        let sine: Vec<f32> = (0..480)
            .map(|i| (i as f32 / 48.0 * std::f32::consts::TAU).sin())
            .collect();
        assert!((rms_db(&sine) + 3.01).abs() < 0.1);
        assert_eq!(rms_db(&[0.0; 480]), SILENCE_DB);
        assert_eq!(rms_db(&[]), SILENCE_DB);
    }

    #[test]
    fn meter_rises_instantly_falls_slowly_and_holds_talking() {
        let mut m = Meter::default();
        assert!(!m.talking());
        m.feed(&[0.5; 480]);
        assert!(m.db() > -7.0 && m.talking());

        m.feed_silence();
        assert!(m.db() > -8.0, "falls gradually");
        for _ in 0..TALK_HANGOVER_FRAMES - 2 {
            m.feed_silence();
        }
        assert!(m.talking(), "hangover bridges pauses between words");
        m.feed_silence();
        m.feed_silence();
        assert!(!m.talking());
    }
}
