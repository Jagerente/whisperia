use super::{level::rms_db, voice::Heard};
use crate::config::MIC_GAIN_RANGE_DB;

const TARGET_DB: f32 = -24.0;
const NOISE_CEILING_DB: f32 = -50.0;

const SPEECH_RUN_FRAMES: u32 = 12;
const AVERAGE_FRAMES: u16 = 100;
const REACH_DB: f32 = 10.0;

const NOISE_PERIOD_FRAMES: u16 = 100;
const NOISE_FLOOR_BOTTOM_DB: f32 = -84.0;

const SETTLE_DB_PER_FRAME: f32 = 0.5;
const SETTLED_WITHIN_DB: f32 = 1.0;
const RISE_DB_PER_FRAME: f32 = 0.08;
const FALL_DB_PER_FRAME: f32 = 0.2;

#[derive(Debug, Clone, Copy)]
struct Average {
    db: f32,
    frames: u16,
}

impl Average {
    fn first(db: f32) -> Self {
        Self { db, frames: 1 }
    }

    fn with(self, now: f32) -> Self {
        let pull = match self.frames < AVERAGE_FRAMES {
            true => now - self.db,
            false => (now - self.db).clamp(-REACH_DB, REACH_DB),
        };
        let frames = (self.frames + 1).min(AVERAGE_FRAMES);
        Self {
            db: self.db + pull / f32::from(frames),
            frames,
        }
    }

    fn is_full(self) -> bool {
        self.frames == AVERAGE_FRAMES
    }
}

#[derive(Debug, Clone, Default)]
struct SpeechLevel {
    reliable: Option<Average>,
    pending: Option<Average>,
    run: u32,
}

impl SpeechLevel {
    fn hear(&mut self, now: f32, heard: Heard) {
        match heard {
            Heard::Speech => {
                self.run += 1;
                self.pending = Some(self.pending.map_or(Average::first(now), |a| a.with(now)));
                if self.talking() {
                    self.reliable = self.pending;
                }
            }
            Heard::Other => {
                self.run = 0;
                self.pending = self.reliable;
            }
        }
    }

    fn db(&self) -> Option<f32> {
        self.reliable.map(|average| average.db)
    }

    fn confident(&self) -> bool {
        self.reliable.is_some_and(Average::is_full)
    }

    fn talking(&self) -> bool {
        self.run >= SPEECH_RUN_FRAMES
    }
}

#[derive(Debug, Clone, Default)]
struct NoiseFloor {
    db: Option<f32>,
    period: Option<(f32, u16)>,
}

impl NoiseFloor {
    fn hear(&mut self, now: f32) {
        let now = now.max(NOISE_FLOOR_BOTTOM_DB);
        let floor = self.db.map_or(now, |db| db.min(now));
        let (quietest, frames) = self.period.map_or((now, 1), |(quietest, frames)| {
            (quietest.min(now), frames + 1)
        });
        if frames == NOISE_PERIOD_FRAMES {
            self.db = Some((floor + quietest) / 2.0);
            self.period = None;
        } else {
            self.db = Some(floor);
            self.period = Some((quietest, frames));
        }
    }

    fn most_gain_db(&self) -> f32 {
        self.db
            .map_or(f32::INFINITY, |db| (NOISE_CEILING_DB - db).max(0.0))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Agc {
    speech: SpeechLevel,
    noise: NoiseFloor,
    gain_db: f32,
    settled: bool,
}

impl Agc {
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    pub fn listen(&mut self, frame: &[f32], heard: Heard) {
        let now = rms_db(frame);
        if heard == Heard::Other {
            self.noise.hear(now);
        }
        self.speech.hear(now, heard);
        let Some(speech_db) = self.speech.db() else {
            return;
        };
        let wanted = (TARGET_DB - speech_db)
            .min(self.noise.most_gain_db())
            .clamp(*MIC_GAIN_RANGE_DB.start(), *MIC_GAIN_RANGE_DB.end());
        let gap = wanted - self.gain_db;
        self.settled |= self.speech.confident() && gap.abs() <= SETTLED_WITHIN_DB;
        let (rise, fall) = match self.settled {
            true => (RISE_DB_PER_FRAME, FALL_DB_PER_FRAME),
            false => (SETTLE_DB_PER_FRAME, SETTLE_DB_PER_FRAME),
        };
        let rise = match self.speech.talking() {
            true => rise,
            false => 0.0,
        };
        self.gain_db += gap.clamp(-fall, rise);
    }
}

#[cfg(test)]
mod tests {
    use whisperia_proto::FRAME_SAMPLES;

    use super::*;
    use crate::dsp::db_to_gain;

    const SPREAD_DB: [f32; 8] = [0.0, 5.0, -4.0, 7.0, -6.0, 2.0, -3.0, -1.0];

    fn at(db: f32) -> [f32; FRAME_SAMPLES] {
        [db_to_gain(db); FRAME_SAMPLES]
    }

    fn talk(agc: &mut Agc, db: f32, frames: usize) -> f32 {
        let mut out = 0.0;
        for n in 0..frames {
            let level = db + SPREAD_DB[n % SPREAD_DB.len()];
            agc.listen(&at(level), Heard::Speech);
            out += level + agc.gain_db();
        }
        out / frames as f32
    }

    fn settled(db: f32) -> Agc {
        let mut agc = Agc::default();
        talk(&mut agc, db, 2000);
        agc
    }

    #[test]
    fn quiet_and_loud_talkers_end_up_at_the_target() {
        for db in [-50.0, -30.0, -12.0] {
            let out = talk(&mut settled(db), db, 500);
            assert!((out - TARGET_DB).abs() < 0.5, "{db} dB came out at {out}");
        }
    }

    #[test]
    fn a_quiet_microphone_is_lifted_within_its_first_second_of_speech() {
        let mut agc = Agc::default();
        talk(&mut agc, -48.0, 100);
        assert!(
            (agc.gain_db() - 24.0).abs() < 1.0,
            "{} after a second",
            agc.gain_db()
        );
    }

    #[test]
    fn once_settled_the_gain_moves_gently() {
        let mut agc = settled(-30.0);
        let before = agc.gain_db();
        talk(&mut agc, -40.0, 50);
        assert!(
            agc.gain_db() - before <= 4.0 + 1e-3,
            "{before} -> {}",
            agc.gain_db()
        );
    }

    #[test]
    fn a_quieter_microphone_is_followed_within_seconds() {
        let mut agc = settled(-15.0);
        talk(&mut agc, -40.0, 600);
        let wanted = settled(-40.0).gain_db();
        assert!(
            (agc.gain_db() - wanted).abs() < 1.0,
            "{} instead of {wanted}",
            agc.gain_db()
        );
    }

    #[test]
    fn a_loud_moment_barely_moves_the_gain() {
        let mut agc = settled(-35.0);
        let before = agc.gain_db();
        for _ in 0..20 {
            agc.listen(&at(-5.0), Heard::Speech);
        }
        assert!(
            before - agc.gain_db() < 3.0,
            "{before} -> {}",
            agc.gain_db()
        );
    }

    #[test]
    fn bursts_shorter_than_a_word_are_not_taken_for_speech() {
        let mut agc = settled(-35.0);
        let gain = agc.gain_db();
        for _ in 0..100 {
            agc.listen(&at(-90.0), Heard::Other);
            for _ in 0..SPEECH_RUN_FRAMES - 1 {
                agc.listen(&at(-5.0), Heard::Speech);
            }
        }
        assert!(
            (agc.gain_db() - gain).abs() < 0.1,
            "{gain} -> {}",
            agc.gain_db()
        );
    }

    #[test]
    fn pauses_do_not_raise_the_gain() {
        let mut agc = settled(-35.0);
        let gain = agc.gain_db();
        for _ in 0..1000 {
            agc.listen(&at(-90.0), Heard::Other);
            agc.listen(&at(-75.0), Heard::Other);
        }
        assert!(
            (agc.gain_db() - gain).abs() < 0.1,
            "{gain} -> {}",
            agc.gain_db()
        );
    }

    #[test]
    fn background_noise_is_never_lifted_over_minus_fifty_db() {
        for noise in [-60.0, -48.0] {
            let mut agc = Agc::default();
            for _ in 0..20 {
                for _ in 0..30 {
                    agc.listen(&at(noise), Heard::Other);
                }
                talk(&mut agc, -45.0, 100);
            }
            let lifted = noise + agc.gain_db();
            assert!(
                lifted <= NOISE_CEILING_DB.max(noise) + 0.1,
                "{noise} dB of noise lifted to {lifted}"
            );
        }
    }

    #[test]
    fn a_noisier_room_brings_the_gain_down() {
        let mut agc = Agc::default();
        for _ in 0..10 {
            agc.listen(&at(-75.0), Heard::Other);
            talk(&mut agc, -45.0, 100);
        }
        assert!(agc.gain_db() > 20.0);
        for _ in 0..20 {
            for _ in 0..30 {
                agc.listen(&at(-55.0), Heard::Other);
            }
            talk(&mut agc, -45.0, 100);
        }
        let lifted = -55.0 + agc.gain_db();
        assert!(lifted <= NOISE_CEILING_DB + 1.0, "noise lifted to {lifted}");
    }
}
