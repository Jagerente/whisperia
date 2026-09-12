use whisperia_proto::FRAME_SAMPLES;

use super::db_to_gain;

pub const CEILING: f32 = 0.9;
const BLOCK_SAMPLES: usize = 48;
const BLOCKS: usize = FRAME_SAMPLES / BLOCK_SAMPLES;
const RELEASE_DB_PER_BLOCK: f32 = 0.05;

#[derive(Debug, Clone)]
pub struct Limiter {
    gain: f32,
}

impl Default for Limiter {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

impl Limiter {
    pub fn process(&mut self, frame: &mut [f32; FRAME_SAMPLES]) {
        let mut allowed = [1.0f32; BLOCKS];
        for (allowed, block) in allowed.iter_mut().zip(frame.chunks(BLOCK_SAMPLES)) {
            let peak = block.iter().fold(0f32, |m, s| m.max(s.abs()));
            if peak > CEILING {
                *allowed = CEILING / peak;
            }
        }

        let release = db_to_gain(RELEASE_DB_PER_BLOCK);
        let mut start = self.gain.min(allowed[0]);
        for (k, block) in frame.chunks_mut(BLOCK_SAMPLES).enumerate() {
            let next = allowed.get(k + 1).copied().unwrap_or(1.0);
            let end = (start * release).min(1.0).min(allowed[k]).min(next);
            for (i, s) in block.iter_mut().enumerate() {
                *s *= start + (end - start) * (i as f32 / BLOCK_SAMPLES as f32);
            }
            start = end;
        }
        self.gain = start;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(frame: &[f32]) -> f32 {
        frame.iter().fold(0f32, |m, s| m.max(s.abs()))
    }

    fn sine(amplitude: f32, from: usize) -> [f32; FRAME_SAMPLES] {
        std::array::from_fn(|i| {
            amplitude * ((from + i) as f32 / 48.0 * std::f32::consts::TAU * 1.37).sin()
        })
    }

    #[test]
    fn audio_under_the_ceiling_passes_untouched() {
        let mut limiter = Limiter::default();
        let mut frame = sine(0.8, 0);
        let copy = frame;
        limiter.process(&mut frame);
        assert_eq!(frame, copy);
    }

    #[test]
    fn nothing_leaves_above_the_ceiling() {
        let mut limiter = Limiter::default();
        let amplitudes = [0.1, 3.0, 0.2, 8.0, 1.0, 0.95, 40.0, 0.5];
        for (n, amplitude) in amplitudes.into_iter().enumerate() {
            let mut frame = sine(amplitude, n * FRAME_SAMPLES);
            frame[n * 37 % FRAME_SAMPLES] = amplitude * 1.5;
            limiter.process(&mut frame);
            assert!(
                peak(&frame) <= CEILING + 1e-5,
                "{amplitude}: {}",
                peak(&frame)
            );
        }
    }

    #[test]
    fn the_gain_comes_down_smoothly_before_a_peak_in_the_same_frame() {
        let mut limiter = Limiter::default();
        let input = {
            let mut frame = [0.5; FRAME_SAMPLES];
            frame[300] = 2.0;
            frame
        };
        let mut frame = input;
        limiter.process(&mut frame);
        assert_eq!(frame[..200], input[..200]);
        assert!((frame[300] - CEILING).abs() < 1e-5);
        let gains: Vec<f32> = frame.iter().zip(&input).map(|(o, i)| o / i).collect();
        let steepest = gains
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0f32, f32::max);
        assert!(steepest < 0.02, "the gain jumps by {steepest}");
    }

    #[test]
    fn a_click_lets_go_within_half_a_second() {
        let mut limiter = Limiter::default();
        let mut click = [0.0; FRAME_SAMPLES];
        click[10] = 9.0;
        limiter.process(&mut click);
        let mut after = sine(0.5, 0);
        limiter.process(&mut after);
        assert!(peak(&after) < 0.2, "the click holds the gain down");
        for _ in 0..50 {
            limiter.process(&mut [0.0; FRAME_SAMPLES]);
        }
        let mut later = sine(0.5, 0);
        limiter.process(&mut later);
        assert_eq!(later, sine(0.5, 0));
    }
}
