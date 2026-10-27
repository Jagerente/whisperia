use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::{ArrayView2, ArrayViewMut2};
use nnnoiseless::DenoiseState;
use serde::{Deserialize, Serialize};
use whisperia_proto::FRAME_SAMPLES;

use super::voice::Heard;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DenoiseMode {
    Off,
    Light,
    #[default]
    Strong,
}

impl DenoiseMode {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Off,
            1 => Self::Light,
            _ => Self::Strong,
        }
    }
}

const PCM_SCALE: f32 = 32768.0;

pub struct Denoiser {
    deep: Option<DfTract>,
    rnnoise: Box<DenoiseState<'static>>,
    scratch: [f32; FRAME_SAMPLES],
    cleaned: [f32; FRAME_SAMPLES],
}

impl Denoiser {
    pub fn new() -> Self {
        let deep = DfTract::new(DfParams::default(), &RuntimeParams::default_with_ch(1))
            .inspect_err(|e| tracing::error!("DeepFilterNet failed to load, using RNNoise: {e:#}"))
            .ok()
            .filter(|m| {
                let fits = m.hop_size == FRAME_SAMPLES && m.sr == 48_000;
                if !fits {
                    tracing::error!(m.hop_size, m.sr, "unexpected DeepFilterNet model geometry");
                }
                fits
            });
        Self {
            deep,
            rnnoise: DenoiseState::new(),
            scratch: [0.0; FRAME_SAMPLES],
            cleaned: [0.0; FRAME_SAMPLES],
        }
    }

    pub fn process(&mut self, mode: DenoiseMode, frame: &mut [f32; FRAME_SAMPLES]) -> Heard {
        if mode == DenoiseMode::Strong {
            self.deep_filter(frame);
        }
        for (scaled, sample) in self.scratch.iter_mut().zip(frame.iter()) {
            *scaled = sample * PCM_SCALE;
        }
        let probability = self.rnnoise.process_frame(&mut self.cleaned, &self.scratch);
        if mode != DenoiseMode::Off {
            for (sample, cleaned) in frame.iter_mut().zip(self.cleaned.iter()) {
                *sample = cleaned / PCM_SCALE;
            }
        }
        Heard::of_probability(probability)
    }

    fn deep_filter(&mut self, frame: &mut [f32; FRAME_SAMPLES]) {
        let Some(model) = self.deep.as_mut() else {
            return;
        };
        self.scratch.copy_from_slice(frame);
        let shape = (1, FRAME_SAMPLES);
        let (Ok(noisy), Ok(clean)) = (
            ArrayView2::from_shape(shape, &self.scratch[..]),
            ArrayViewMut2::from_shape(shape, &mut frame[..]),
        ) else {
            return;
        };
        if let Err(e) = model.process(noisy, clean) {
            tracing::error!("DeepFilterNet failed, switching to RNNoise: {e:#}");
            frame.copy_from_slice(&self.scratch);
            self.deep = None;
        }
    }
}

impl Default for Denoiser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn noise(state: &mut (u32, f32)) -> f32 {
        state.0 = state.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let white = (state.0 >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
        state.1 = 0.9 * state.1 + 0.1 * white;
        state.1 * 1.5
    }

    #[test]
    fn both_models_attenuate_stationary_noise() {
        for mode in [DenoiseMode::Light, DenoiseMode::Strong] {
            let mut d = Denoiser::new();
            assert!(d.deep.is_some(), "bundled DeepFilterNet model must load");
            let mut seed = (7, 0.0);
            let (mut energy_in, mut energy_out) = (0.0, 0.0);
            for i in 0..600 {
                let mut frame: [f32; FRAME_SAMPLES] = std::array::from_fn(|_| noise(&mut seed));
                let before = rms(&frame);
                d.process(mode, &mut frame);
                if i >= 500 {
                    energy_in += before;
                    energy_out += rms(&frame);
                }
            }
            assert!(
                energy_out < energy_in * 0.25,
                "{mode:?}: {energy_in} -> {energy_out}"
            );
        }
    }

    #[test]
    fn the_strongest_level_is_deepfilternet_followed_by_rnnoise() {
        let mut strong = Denoiser::new();
        let mut deep = Denoiser::new();
        let mut rnnoise = DenoiseState::new();
        let mut seed = (3, 0.0);
        for _ in 0..200 {
            let input: [f32; FRAME_SAMPLES] = std::array::from_fn(|_| noise(&mut seed));
            let mut out = input;
            strong.process(DenoiseMode::Strong, &mut out);

            let mut expected = input;
            deep.deep_filter(&mut expected);
            let scaled = expected.map(|s| s * PCM_SCALE);
            let mut cleaned = [0.0; FRAME_SAMPLES];
            rnnoise.process_frame(&mut cleaned, &scaled);
            assert_eq!(out, cleaned.map(|s| s / PCM_SCALE));
        }
    }

    #[test]
    fn the_strongest_level_without_deepfilternet_is_the_light_one() {
        let mut strong = Denoiser::new();
        strong.deep = None;
        let mut light = Denoiser::new();
        let mut seed = (5, 0.0);
        for _ in 0..50 {
            let input: [f32; FRAME_SAMPLES] = std::array::from_fn(|_| noise(&mut seed));
            let (mut a, mut b) = (input, input);
            strong.process(DenoiseMode::Strong, &mut a);
            light.process(DenoiseMode::Light, &mut b);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn off_is_bit_exact() {
        let mut d = Denoiser::new();
        let mut frame: [f32; FRAME_SAMPLES] = std::array::from_fn(|i| i as f32 / 1000.0);
        let copy = frame;
        d.process(DenoiseMode::Off, &mut frame);
        assert_eq!(frame, copy);
    }
}
