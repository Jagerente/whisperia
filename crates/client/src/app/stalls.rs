use std::time::{Duration, Instant};

use crate::engine::Losses;

pub const WINDOW: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Stalls {
    since: Option<(Instant, Losses)>,
}

impl Stalls {
    pub fn take(&mut self, now: Instant, counted: Losses) -> Option<Losses> {
        let Some((start, before)) = self.since else {
            self.since = Some((now, counted));
            return None;
        };
        if now.duration_since(start) < WINDOW {
            return None;
        }
        self.since = Some((now, counted));
        let lost = Losses {
            microphone_samples: counted.microphone_samples - before.microphone_samples,
            speaker_underruns: counted.speaker_underruns - before.speaker_underruns,
        };
        (lost != Losses::default()).then_some(lost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lost(microphone_samples: u64, speaker_underruns: u64) -> Losses {
        Losses {
            microphone_samples,
            speaker_underruns,
        }
    }

    #[test]
    fn losses_are_told_once_a_window_and_only_when_there_were_any() {
        let t0 = Instant::now();
        let mut stalls = Stalls::default();
        assert_eq!(stalls.take(t0, lost(0, 0)), None);
        assert_eq!(stalls.take(t0 + WINDOW / 2, lost(480, 3)), None);
        assert_eq!(stalls.take(t0 + WINDOW, lost(480, 3)), Some(lost(480, 3)));
        assert_eq!(stalls.take(t0 + WINDOW * 2, lost(480, 3)), None);
        assert_eq!(stalls.take(t0 + WINDOW * 3, lost(480, 4)), Some(lost(0, 1)));
    }
}
