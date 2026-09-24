use std::time::{Duration, Instant};

use whisperia::audio::fake::FakeBackend;

pub const SILENCE: f32 = 0.001;

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

pub fn is_silent(x: &[f32]) -> bool {
    rms(x) < SILENCE
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0f32, |most, s| most.max(s.abs()))
}

pub fn goertzel(x: &[f32], rate: u32, hz: f32) -> f32 {
    let w = std::f64::consts::TAU * f64::from(hz) / f64::from(rate);
    let coefficient = 2.0 * w.cos();
    let (mut before, mut last) = (0f64, 0f64);
    for &sample in x {
        let now = f64::from(sample) + coefficient * last - before;
        before = last;
        last = now;
    }
    let power = last * last + before * before - coefficient * last * before;
    (2.0 * power.max(0.0).sqrt() / x.len().max(1) as f64) as f32
}

pub fn tone_level(x: &[f32], rate: u32, hz: f32) -> f32 {
    let block = (rate / 20) as usize;
    let mut levels: Vec<f32> = x
        .chunks_exact(block)
        .map(|chunk| goertzel(chunk, rate, hz))
        .collect();
    if levels.is_empty() {
        return goertzel(x, rate, hz);
    }
    levels.sort_by(f32::total_cmp);
    levels[levels.len() / 2]
}

pub fn is_tone(x: &[f32], rate: u32, hz: f32) -> bool {
    let at = tone_level(x, rate, hz);
    let beside = tone_level(x, rate, hz * 0.8).max(tone_level(x, rate, hz * 1.25));
    at > 0.01 && at > 5.0 * beside
}

pub fn played(speaker: &FakeBackend) -> usize {
    speaker
        .played
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .len()
}

pub fn last(speaker: &FakeBackend, samples: usize) -> Vec<f32> {
    let played = speaker.played.lock().unwrap_or_else(|p| p.into_inner());
    played[played.len().saturating_sub(samples)..].to_vec()
}

pub fn since(speaker: &FakeBackend, from: usize) -> Vec<f32> {
    let played = speaker.played.lock().unwrap_or_else(|p| p.into_inner());
    played.get(from..).unwrap_or_default().to_vec()
}

pub fn peak_after(speaker: &FakeBackend, from: usize, timeout: Duration) -> f32 {
    const HEARD: f32 = 0.02;
    const OVER: usize = 4_800;
    let deadline = Instant::now() + timeout;
    loop {
        let played = since(speaker, from);
        let loudest = peak(&played);
        let over = played.len() > OVER && peak(&played[played.len() - OVER..]) < HEARD;
        if (loudest > HEARD && over) || Instant::now() > deadline {
            return loudest;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, amplitude: f32, rate: u32, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|n| amplitude * (n as f32 / rate as f32 * hz * std::f32::consts::TAU).sin())
            .collect()
    }

    #[test]
    fn goertzel_measures_the_amplitude_at_its_frequency() {
        let tone = sine(440.0, 0.4, 48_000, 48_000);
        assert!((goertzel(&tone, 48_000, 440.0) - 0.4).abs() < 0.01);
        assert!(goertzel(&tone, 48_000, 1500.0) < 0.01);
    }

    #[test]
    fn the_level_of_a_tone_survives_a_gap() {
        let mut tone = sine(440.0, 0.4, 48_000, 48_000);
        tone[10_000..14_800].fill(0.0);
        tone.rotate_left(333);
        assert!((tone_level(&tone, 48_000, 440.0) - 0.4).abs() < 0.02);
    }

    #[test]
    fn a_tone_is_told_from_its_neighbours_and_from_silence() {
        let tone = sine(660.0, 0.3, 48_000, 24_000);
        assert!(is_tone(&tone, 48_000, 660.0));
        assert!(!is_tone(&tone, 48_000, 440.0));
        assert!(!is_tone(&[0.0; 4800], 48_000, 660.0));
    }

    #[test]
    fn silence_is_silent_and_a_tone_is_not() {
        assert!(is_silent(&[0.0; 480]));
        assert!(!is_silent(&sine(440.0, 0.1, 48_000, 480)));
    }
}
