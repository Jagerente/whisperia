pub mod cue;
pub mod denoise;
pub mod level;
pub mod resample;

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[derive(Debug, Clone)]
pub struct Gain {
    now: f32,
}

impl Default for Gain {
    fn default() -> Self {
        Self { now: 1.0 }
    }
}

impl Gain {
    pub fn glide(&mut self, frame: &mut [f32], to: f32) {
        let from = self.now;
        let n = frame.len() as f32;
        for (i, s) in frame.iter_mut().enumerate() {
            *s *= from + (to - from) * ((i + 1) as f32 / n);
        }
        self.now = to;
    }
}

pub fn volume_to_gain(db: f32) -> f32 {
    if is_silent(db) {
        0.0
    } else {
        db_to_gain(db)
    }
}

pub fn is_silent(volume_db: f32) -> bool {
    volume_db <= *crate::config::GAIN_RANGE_DB.start()
}

pub const VOLUME_PERCENT_MAX: f32 = 200.0;

pub fn percent_to_volume(percent: f32) -> f32 {
    if percent.is_nan() || percent <= 0.0 {
        return *crate::config::GAIN_RANGE_DB.start();
    }
    crate::config::clamp_db(20.0 * (percent / 100.0).log10())
}

pub fn volume_to_percent(db: f32) -> f32 {
    if is_silent(db) {
        0.0
    } else {
        (db_to_gain(db) * 100.0).round().min(VOLUME_PERCENT_MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_turned_all_the_way_down_is_silence() {
        let bottom = *crate::config::GAIN_RANGE_DB.start();
        assert_eq!(volume_to_gain(bottom), 0.0);
        assert_eq!(volume_to_gain(bottom - 5.0), 0.0);
        assert!(volume_to_gain(bottom + 1.0) > 0.03);
        assert!((volume_to_gain(0.0) - 1.0).abs() < 1e-6);
        assert!((volume_to_gain(6.0) - 2.0).abs() < 0.01);

        assert!(db_to_gain(bottom) > 0.0);
    }

    #[test]
    fn a_volume_in_percent_says_what_it_does_to_the_sound() {
        assert_eq!(percent_to_volume(100.0), 0.0);
        assert_eq!(volume_to_percent(0.0), 100.0);
        assert!((volume_to_gain(percent_to_volume(200.0)) - 2.0).abs() < 0.01);
        assert!((volume_to_gain(percent_to_volume(50.0)) - 0.5).abs() < 0.01);
        assert_eq!(volume_to_percent(percent_to_volume(0.0)), 0.0);

        for percent in [4.0, 25.0, 70.0, 100.0, 133.0, 200.0] {
            assert_eq!(volume_to_percent(percent_to_volume(percent)), percent);
        }

        assert!(is_silent(percent_to_volume(2.0)));
        assert_eq!(volume_to_percent(percent_to_volume(2.0)), 0.0);

        assert_eq!(volume_to_percent(20.0), VOLUME_PERCENT_MAX);
    }

    #[test]
    fn a_gain_glides_across_one_frame_and_then_holds() {
        let mut gain = Gain::default();
        let mut frame = [1.0; 4];
        gain.glide(&mut frame, 3.0);
        assert_eq!(frame, [1.5, 2.0, 2.5, 3.0]);
        let mut frame = [1.0; 4];
        gain.glide(&mut frame, 3.0);
        assert_eq!(frame, [3.0; 4]);
    }

    #[test]
    fn a_gain_of_one_leaves_the_sound_as_it_is() {
        let mut frame = [0.1, -0.7, 0.33];
        Gain::default().glide(&mut frame, 1.0);
        assert_eq!(frame, [0.1, -0.7, 0.33]);
    }

    #[test]
    fn a_percentage_that_is_not_a_number_is_silence_not_the_loudest() {
        assert!(is_silent(percent_to_volume(f32::NAN)));
    }
}
