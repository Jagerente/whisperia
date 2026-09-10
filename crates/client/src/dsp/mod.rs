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

#[cfg(test)]
mod tests {
    use super::*;

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
}
