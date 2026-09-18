use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub const EVERY: Duration = Duration::from_millis(500);
pub const KEEP: usize = 60;

const GOOD_MS: f32 = 60.0;
const WEAK_MS: f32 = 200.0;
const GOOD_LOSS: f32 = 0.01;
const WEAK_LOSS: f32 = 0.05;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Look {
    pub rtt_ms: f32,
    pub sent: u64,
    pub lost_sending: u64,
    pub heard: u64,
    pub missed: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quality {
    #[default]
    Good,
    Weak,
    Poor,
}

impl Quality {
    pub fn word(self) -> &'static str {
        match self {
            Self::Good => "Good",
            Self::Weak => "Weak",
            Self::Poor => "Poor",
        }
    }

    pub fn of_delay(ms: f32) -> Self {
        if ms <= GOOD_MS {
            Self::Good
        } else if ms <= WEAK_MS {
            Self::Weak
        } else {
            Self::Poor
        }
    }

    pub fn of_loss(part: f32) -> Self {
        if part <= GOOD_LOSS {
            Self::Good
        } else if part <= WEAK_LOSS {
            Self::Weak
        } else {
            Self::Poor
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Loss {
    pub sending: f32,
    pub hearing: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    Out,
    In,
}

impl Loss {
    pub fn worst(self) -> f32 {
        self.sending.max(self.hearing)
    }

    pub fn way(self) -> Option<Way> {
        if self.worst() <= 0.0 {
            None
        } else if self.sending >= self.hearing {
            Some(Way::Out)
        } else {
            Some(Way::In)
        }
    }

    fn between(first: &Look, last: &Look) -> Self {
        let part = |whole: u64, gone: u64| {
            if whole == 0 {
                0.0
            } else {
                gone as f32 / whole as f32
            }
        };
        let missed = last.missed.saturating_sub(first.missed);
        Self {
            sending: part(
                last.sent.saturating_sub(first.sent),
                last.lost_sending.saturating_sub(first.lost_sending),
            ),
            hearing: part(last.heard.saturating_sub(first.heard) + missed, missed),
        }
    }
}

#[derive(Default)]
pub struct Link {
    looks: VecDeque<Look>,
    due: Option<Instant>,
}

impl Link {
    pub fn take(&mut self, now: Instant, look: Look) {
        if self.due.is_some_and(|due| now < due) {
            return;
        }
        self.due = Some(now + EVERY);
        while self.looks.len() >= KEEP {
            self.looks.pop_front();
        }
        self.looks.push_back(look);
    }

    pub fn latest(&self) -> Option<Look> {
        self.looks.back().copied()
    }

    pub fn delays(&self) -> impl ExactSizeIterator<Item = f32> + '_ {
        self.looks.iter().map(|look| look.rtt_ms)
    }

    pub fn top_ms(&self) -> f32 {
        let worst = self.delays().fold(0.0f32, f32::max) * 1.25;
        [0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0]
            .into_iter()
            .find(|step| worst <= *step)
            .unwrap_or_else(|| (worst / 500.0).ceil() * 500.0)
    }

    pub fn loss(&self) -> Loss {
        match (self.looks.front(), self.looks.back()) {
            (Some(first), Some(last)) => Loss::between(first, last),
            _ => Loss::default(),
        }
    }

    pub fn losses(&self) -> impl Iterator<Item = Loss> + '_ {
        self.looks.iter().scan(None, |before, look| {
            let loss = before.map_or(Loss::default(), |before| Loss::between(before, look));
            *before = Some(look);
            Some(loss)
        })
    }

    pub fn quality(&self) -> Quality {
        let delay = self
            .latest()
            .map_or(Quality::Good, |look| Quality::of_delay(look.rtt_ms));
        delay.max(Quality::of_loss(self.loss().worst()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn looks_are_taken_apart_and_only_the_last_half_minute_is_kept() {
        let t0 = Instant::now();
        let mut link = Link::default();
        link.take(
            t0,
            Look {
                rtt_ms: 1.0,
                ..Look::default()
            },
        );
        link.take(
            at(t0, 100),
            Look {
                rtt_ms: 9.0,
                ..Look::default()
            },
        );
        assert_eq!(link.delays().len(), 1);
        assert_eq!(link.latest().unwrap().rtt_ms, 1.0);

        for step in 1..200u64 {
            link.take(
                at(t0, step * EVERY.as_millis() as u64),
                Look {
                    rtt_ms: step as f32,
                    ..Look::default()
                },
            );
        }
        assert_eq!(link.delays().len(), KEEP);
        assert_eq!(link.latest().unwrap().rtt_ms, 199.0);
        assert_eq!(link.delays().next().unwrap(), 140.0);
    }

    #[test]
    fn loss_is_what_went_missing_within_the_window() {
        let t0 = Instant::now();
        let mut link = Link::default();
        assert_eq!(link.loss(), Loss::default());
        link.take(
            t0,
            Look {
                sent: 1000,
                lost_sending: 100,
                heard: 500,
                missed: 50,
                ..Look::default()
            },
        );
        link.take(
            at(t0, 500),
            Look {
                sent: 1100,
                lost_sending: 110,
                heard: 596,
                missed: 54,
                ..Look::default()
            },
        );
        let loss = link.loss();
        assert!((loss.sending - 0.1).abs() < 1e-6, "{loss:?}");
        assert!((loss.hearing - 0.04).abs() < 1e-6, "{loss:?}");
        assert!((loss.worst() - 0.1).abs() < 1e-6);

        link.take(
            at(t0, 1000),
            Look {
                sent: 1200,
                lost_sending: 110,
                ..Look::default()
            },
        );
        assert_eq!(link.loss().hearing, 0.0);
    }

    #[test]
    fn loss_names_the_way_that_lost_more() {
        assert_eq!(Loss::default().way(), None);
        let out = Loss {
            sending: 0.02,
            hearing: 0.01,
        };
        assert_eq!(out.way(), Some(Way::Out));
        let into = Loss {
            sending: 0.0,
            hearing: 0.003,
        };
        assert_eq!(into.way(), Some(Way::In));
    }

    #[test]
    fn every_look_but_the_first_knows_what_was_lost_since_the_one_before() {
        let t0 = Instant::now();
        let mut link = Link::default();
        assert_eq!(link.losses().count(), 0);
        for (step, lost) in [0u64, 0, 5, 5].into_iter().enumerate() {
            link.take(
                at(t0, step as u64 * EVERY.as_millis() as u64),
                Look {
                    sent: step as u64 * 100,
                    lost_sending: lost,
                    ..Look::default()
                },
            );
        }
        let sending: Vec<f32> = link.losses().map(|loss| loss.sending).collect();
        assert_eq!(sending, [0.0, 0.0, 0.05, 0.0]);
    }

    #[test]
    fn a_grade_is_the_worse_of_the_delay_and_what_got_lost() {
        assert_eq!(Quality::of_delay(1.0), Quality::Good);
        assert_eq!(Quality::of_delay(120.0), Quality::Weak);
        assert_eq!(Quality::of_delay(400.0), Quality::Poor);
        assert_eq!(Quality::of_loss(0.0), Quality::Good);
        assert_eq!(Quality::of_loss(0.03), Quality::Weak);
        assert_eq!(Quality::of_loss(0.2), Quality::Poor);
        assert_eq!(Quality::Good.max(Quality::Poor), Quality::Poor);

        let t0 = Instant::now();
        let mut link = Link::default();
        link.take(
            t0,
            Look {
                rtt_ms: 2.0,
                sent: 100,
                ..Look::default()
            },
        );
        assert_eq!(link.quality(), Quality::Good);
        link.take(
            at(t0, 500),
            Look {
                rtt_ms: 2.0,
                sent: 200,
                lost_sending: 10,
                ..Look::default()
            },
        );
        assert_eq!(link.quality(), Quality::Poor);
    }

    #[test]
    fn a_graph_of_round_trips_gets_a_round_number_over_the_worst_of_them() {
        let t0 = Instant::now();
        let mut link = Link::default();
        assert_eq!(link.top_ms(), 0.5);
        link.take(
            t0,
            Look {
                rtt_ms: 1.4,
                ..Look::default()
            },
        );
        assert_eq!(link.top_ms(), 2.0);
        link.take(
            at(t0, 500),
            Look {
                rtt_ms: 17.0,
                ..Look::default()
            },
        );
        assert_eq!(link.top_ms(), 50.0);
        link.take(
            at(t0, 1000),
            Look {
                rtt_ms: 900.0,
                ..Look::default()
            },
        );
        assert_eq!(link.top_ms(), 1500.0);
    }
}
