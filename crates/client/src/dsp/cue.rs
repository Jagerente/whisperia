use std::sync::atomic::{AtomicU32, Ordering};

use whisperia_proto::SAMPLE_RATE;

const AMPLITUDE: f32 = 0.11;
const ATTACK_SAMPLES: f32 = 0.004 * SAMPLE_RATE as f32;
const RELEASE_SAMPLES: f32 = 0.010 * SAMPLE_RATE as f32;
const MAX_VOICES: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    Join,
    Leave,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
    Message,
    Connected,
    Lost,
    ShareStart,
    ShareStop,
}

const ALL: [Cue; 11] = [
    Cue::Join,
    Cue::Leave,
    Cue::Mute,
    Cue::Unmute,
    Cue::Deafen,
    Cue::Undeafen,
    Cue::Message,
    Cue::Connected,
    Cue::Lost,
    Cue::ShareStart,
    Cue::ShareStop,
];

impl Cue {
    fn notes(self) -> &'static [(f32, u32)] {
        match self {
            Cue::Join => &[(659.3, 70), (880.0, 110)],
            Cue::Leave => &[(880.0, 70), (587.3, 110)],
            Cue::Mute => &[(523.3, 60), (392.0, 90)],
            Cue::Unmute => &[(392.0, 60), (523.3, 90)],
            Cue::Deafen => &[(392.0, 60), (311.1, 60), (261.6, 100)],
            Cue::Undeafen => &[(261.6, 60), (311.1, 60), (392.0, 100)],
            Cue::Message => &[(1046.5, 45), (1318.5, 80)],
            Cue::Connected => &[(523.3, 70), (659.3, 70), (784.0, 130)],
            Cue::Lost => &[(392.0, 120), (0.0, 40), (293.7, 200)],
            Cue::ShareStart => &[(587.3, 60), (740.0, 60), (880.0, 60), (1174.7, 120)],
            Cue::ShareStop => &[(1174.7, 60), (880.0, 60), (587.3, 120)],
        }
    }

    fn bit(self) -> u32 {
        1 << self as u32
    }
}

#[derive(Debug, Default)]
pub struct CueRequests(AtomicU32);

impl CueRequests {
    pub fn request(&self, cue: Cue) {
        self.0.fetch_or(cue.bit(), Ordering::Relaxed);
    }

    pub fn take(&self) -> impl Iterator<Item = Cue> {
        let bits = self.0.swap(0, Ordering::Relaxed);
        ALL.into_iter().filter(move |c| bits & c.bit() != 0)
    }
}

struct Voice {
    notes: &'static [(f32, u32)],
    note: usize,
    position: u32,
    phase: f32,
}

pub struct CuePlayer {
    voices: Vec<Voice>,
}

impl Default for CuePlayer {
    fn default() -> Self {
        Self {
            voices: Vec::with_capacity(MAX_VOICES),
        }
    }
}

impl CuePlayer {
    pub fn start(&mut self, cue: Cue) {
        if self.voices.len() < MAX_VOICES {
            self.voices.push(Voice {
                notes: cue.notes(),
                note: 0,
                position: 0,
                phase: 0.0,
            });
        }
    }

    pub fn is_idle(&self) -> bool {
        self.voices.is_empty()
    }

    pub fn mix_into(&mut self, out: &mut [f32]) {
        for voice in &mut self.voices {
            for sample in out.iter_mut() {
                let Some(&(hz, ms)) = voice.notes.get(voice.note) else {
                    break;
                };
                let length = ms * SAMPLE_RATE / 1000;
                let at = voice.position as f32;
                let left = (length - voice.position) as f32;
                let envelope = (at / ATTACK_SAMPLES).min(left / RELEASE_SAMPLES).min(1.0)
                    * (-2.5 * at / length as f32).exp();
                *sample += AMPLITUDE * envelope * (voice.phase * std::f32::consts::TAU).sin();

                voice.phase = (voice.phase + hz / SAMPLE_RATE as f32).fract();
                voice.position += 1;
                if voice.position >= length {
                    voice.note += 1;
                    voice.position = 0;
                }
            }
        }
        self.voices.retain(|v| v.note < v.notes.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_delivered_once() {
        let requests = CueRequests::default();
        requests.request(Cue::Mute);
        requests.request(Cue::Lost);
        requests.request(Cue::Mute);
        assert_eq!(requests.take().collect::<Vec<_>>(), [Cue::Mute, Cue::Lost]);
        assert_eq!(requests.take().count(), 0);
    }

    #[test]
    fn every_cue_is_audible_short_and_never_loud() {
        for cue in ALL {
            let mut player = CuePlayer::default();
            player.start(cue);
            let mut frames = 0;
            let mut peak = 0f32;
            while !player.is_idle() {
                let mut frame = [0f32; 480];
                player.mix_into(&mut frame);
                peak = frame.iter().fold(peak, |m, s| m.max(s.abs()));
                frames += 1;
            }
            assert!(peak > 0.03 && peak <= AMPLITUDE, "{cue:?}: peak {peak}");
            assert!((5..=50).contains(&frames), "{cue:?}: {frames} frames");
        }
    }

    #[test]
    fn too_many_cues_at_once_are_dropped_not_queued() {
        let mut player = CuePlayer::default();
        (0..20).for_each(|_| player.start(Cue::Join));
        assert_eq!(player.voices.len(), MAX_VOICES);
        assert_eq!(player.voices.capacity(), MAX_VOICES);
    }
}
