use nnnoiseless::DenoiseState;
use whisperia_proto::FRAME_SAMPLES;

const SPEECH_FROM: f32 = 0.95;
const PCM_SCALE: f32 = 32768.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    Speech,
    Other,
}

impl Heard {
    fn of_probability(probability: f32) -> Self {
        if probability >= SPEECH_FROM {
            Self::Speech
        } else {
            Self::Other
        }
    }
}

pub struct VoiceDetector {
    model: Box<DenoiseState<'static>>,
    input: Vec<f32>,
    output: Vec<f32>,
}

impl VoiceDetector {
    pub fn new() -> Self {
        Self {
            model: DenoiseState::new(),
            input: vec![0.0; FRAME_SAMPLES],
            output: vec![0.0; FRAME_SAMPLES],
        }
    }

    pub fn hear(&mut self, frame: &[f32]) -> Heard {
        debug_assert_eq!(frame.len(), FRAME_SAMPLES);
        for (scaled, sample) in self.input.iter_mut().zip(frame) {
            *scaled = sample * PCM_SCALE;
        }
        Heard::of_probability(self.model.process_frame(&mut self.output, &self.input))
    }
}

impl Default for VoiceDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share_of_speech(mut sample: impl FnMut(usize) -> f32) -> f32 {
        let mut detector = VoiceDetector::new();
        let mut speech = 0;
        for f in 0..300 {
            let frame: Vec<f32> = (0..FRAME_SAMPLES)
                .map(|i| sample(f * FRAME_SAMPLES + i))
                .collect();
            if detector.hear(&frame) == Heard::Speech && f >= 100 {
                speech += 1;
            }
        }
        speech as f32 / 200.0
    }

    fn hiss() -> impl FnMut(usize) -> f32 {
        let mut state = 7u32;
        move |_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((state >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.1
        }
    }

    #[test]
    fn a_voiced_sound_is_speech() {
        let voice = |n: usize| {
            let t = n as f32 / 48_000.0;
            (1..12)
                .map(|k| (t * 140.0 * k as f32 * std::f32::consts::TAU).sin() / k as f32)
                .sum::<f32>()
                * 0.05
        };
        assert!(share_of_speech(voice) > 0.9);
    }

    #[test]
    fn hiss_and_silence_are_not_speech() {
        assert_eq!(share_of_speech(hiss()), 0.0);
        assert_eq!(share_of_speech(|_| 0.0), 0.0);
    }
}
