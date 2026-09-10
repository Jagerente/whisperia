use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::{ArrayView2, ArrayViewMut2};
use nnnoiseless::DenoiseState;
use serde::{Deserialize, Serialize};
use whisperia_proto::FRAME_SAMPLES;

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

pub struct Denoiser {
    deep: Option<DfTract>,
    light: Box<DenoiseState<'static>>,
    scratch: Vec<f32>,
    last_mode: DenoiseMode,
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
            light: DenoiseState::new(),
            scratch: vec![0.0; FRAME_SAMPLES],
            last_mode: DenoiseMode::Off,
        }
    }

    pub fn deep_available(&self) -> bool {
        self.deep.is_some()
    }

    pub fn process(&mut self, mode: DenoiseMode, frame: &mut [f32]) {
        debug_assert_eq!(frame.len(), FRAME_SAMPLES);
        let mode = match mode {
            DenoiseMode::Strong if self.deep.is_none() => DenoiseMode::Light,
            m => m,
        };
        if mode != self.last_mode {
            if mode == DenoiseMode::Light {
                self.light = DenoiseState::new();
            }
            self.last_mode = mode;
        }

        match mode {
            DenoiseMode::Off => {}
            DenoiseMode::Light => {
                for (s, x) in self.scratch.iter_mut().zip(frame.iter()) {
                    *s = x * 32768.0;
                }
                self.light.process_frame(frame, &self.scratch);
                frame.iter_mut().for_each(|x| *x /= 32768.0);
            }
            DenoiseMode::Strong => {
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
        let mut d = Denoiser::new();
        assert!(d.deep_available(), "bundled DeepFilterNet model must load");
        for mode in [DenoiseMode::Light, DenoiseMode::Strong] {
            let mut seed = (7, 0.0);
            let (mut energy_in, mut energy_out) = (0.0, 0.0);
            for i in 0..600 {
                let mut frame: Vec<f32> = (0..FRAME_SAMPLES).map(|_| noise(&mut seed)).collect();
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
    fn off_is_bit_exact() {
        let mut d = Denoiser::new();
        let mut frame: Vec<f32> = (0..FRAME_SAMPLES).map(|i| i as f32 / 1000.0).collect();
        let copy = frame.clone();
        d.process(DenoiseMode::Off, &mut frame);
        assert_eq!(frame, copy);
    }
}
