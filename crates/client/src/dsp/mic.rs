use whisperia_proto::FRAME_SAMPLES;

use super::{
    agc::Agc,
    db_to_gain,
    denoise::{DenoiseMode, Denoiser},
    level::Meter,
    limiter::Limiter,
    Gain,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MicSettings {
    pub denoise: DenoiseMode,
    pub agc: bool,
    pub gain_db: f32,
}

pub struct MicChain {
    denoiser: Denoiser,
    agc: Agc,
    gain: Gain,
    limiter: Limiter,
    meter: Meter,
}

impl MicChain {
    pub fn new() -> Self {
        Self {
            denoiser: Denoiser::new(),
            agc: Agc::default(),
            gain: Gain::default(),
            limiter: Limiter::default(),
            meter: Meter::default(),
        }
    }

    pub fn process(
        &mut self,
        frame: &mut [f32; FRAME_SAMPLES],
        settings: MicSettings,
        silent: bool,
    ) {
        if !silent {
            let heard = self.denoiser.process(settings.denoise, frame);
            self.agc.listen(frame, heard);
        }
        let gain_db = match settings.agc {
            true => self.agc.gain_db(),
            false => settings.gain_db,
        };
        self.gain.glide(frame, db_to_gain(gain_db));
        self.limiter.process(frame);
        self.meter.feed(frame);
    }

    pub fn agc_gain_db(&self) -> f32 {
        self.agc.gain_db()
    }

    pub fn meter(&self) -> &Meter {
        &self.meter
    }
}

impl Default for MicChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRONG: MicSettings = MicSettings {
        denoise: DenoiseMode::Strong,
        agc: true,
        gain_db: 0.0,
    };
    const SECOND: usize = 100;

    struct Noise(u32);

    impl Noise {
        fn white(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        }
    }

    fn typing(seconds: usize) -> Vec<f32> {
        let mut noise = Noise(11);
        let mut out: Vec<f32> = (0..seconds * SECOND * FRAME_SAMPLES)
            .map(|_| noise.white() * 0.0003)
            .collect();
        let mut at = 2_000;
        while at + 2_000 < out.len() {
            for i in 0..2_000 {
                let t = i as f32 / 48_000.0;
                let tail = (t * 3_100.0 * std::f32::consts::TAU).sin() * (-t / 0.006).exp();
                let burst = noise.white() * (-t / 0.0008).exp();
                out[at + i] += 0.2 * (burst + 0.5 * tail);
            }
            at += 7_000 + (noise.white().abs() * 9_000.0) as usize;
        }
        out
    }

    fn running_water(seconds: usize) -> Vec<f32> {
        let mut noise = Noise(5);
        let mut low = 0.0;
        (0..seconds * SECOND * FRAME_SAMPLES)
            .map(|n| {
                low = 0.7 * low + 0.3 * noise.white();
                let swell = 1.0 + 0.3 * (n as f32 / 48_000.0 * 4.0).sin();
                0.03 * low * swell
            })
            .collect()
    }

    fn bursts_after_silence(seconds: usize) -> Vec<f32> {
        let mut noise = Noise(17);
        let mut low = 0.0;
        (0..seconds * SECOND * FRAME_SAMPLES)
            .map(|n| {
                low = 0.7 * low + 0.3 * noise.white();
                match n % 19_200 < 7_200 {
                    true => 0.03 * low,
                    false => 0.0,
                }
            })
            .collect()
    }

    fn attenuation_db(chain: &mut MicChain, sound: &[f32], judged_from: usize) -> f32 {
        let (mut before, mut after) = (0.0, 0.0);
        for (n, input) in sound.chunks_exact(FRAME_SAMPLES).enumerate() {
            let mut frame: [f32; FRAME_SAMPLES] = std::array::from_fn(|i| input[i]);
            chain.process(&mut frame, STRONG, false);
            if n >= judged_from {
                before += input.iter().map(|s| s * s).sum::<f32>();
                after += frame.iter().map(|s| s * s).sum::<f32>();
            }
        }
        10.0 * (before / after.max(1e-12)).log10()
    }

    #[test]
    fn the_strongest_level_takes_running_water_away_once_it_has_heard_it() {
        let water = running_water(6);
        let removed = attenuation_db(&mut MicChain::new(), &water, 4 * SECOND);
        assert!(removed > 20.0, "only {removed} dB of water removed");
    }

    #[test]
    fn the_strongest_level_takes_noise_bursts_after_silence_away() {
        let bursts = bursts_after_silence(6);
        let removed = attenuation_db(&mut MicChain::new(), &bursts, 0);
        assert!(removed > 57.0, "only {removed} dB of the bursts removed");
    }

    #[test]
    fn the_strongest_level_takes_typing_away_once_it_has_heard_it() {
        let keys = typing(8);
        let removed = attenuation_db(&mut MicChain::new(), &keys, 5 * SECOND);
        assert!(removed > 20.0, "only {removed} dB of typing removed");
    }
}
