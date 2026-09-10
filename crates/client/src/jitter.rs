use std::time::Instant;

use whisperia_proto::{audio::MAX_PAYLOAD, FRAME_MS};

const CAPACITY: usize = 64;
pub const MIN_TARGET: usize = 2;
pub const MAX_TARGET: usize = 12;
const TRIM_WINDOW: u32 = 100;
const MAX_DRY_CONCEAL: u32 = 5;

const DISCONTINUITY_MS: f64 = 150.0;
const RESYNC_DISTANCE: u32 = 1000;
const PEAK_DECAY: f64 = 0.999;

const BASELINE_LEAK_MS: f64 = 0.002;

#[derive(Debug, PartialEq, Eq)]
pub enum Playout<'a> {
    Silence,
    Packet(&'a [u8]),
    Fec(&'a [u8]),
    Conceal,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JitterStats {
    pub received: u64,

    pub lost: u64,
    pub recovered_by_fec: u64,
    pub late: u64,
    pub trimmed: u64,
    pub underruns: u64,
}

struct Slot {
    seq: u32,
    filled: bool,
    data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Buffering,
    Playing,
}

pub struct JitterBuffer {
    slots: Vec<Slot>,
    state: State,
    next: u32,

    newest: u32,
    target: usize,
    dry_run: u32,

    fresh: bool,
    window_pops: u32,
    window_min_depth: usize,
    epoch: Instant,
    baseline_ms: Option<f64>,
    peak_delay_ms: f64,
    stats: JitterStats,
}

impl JitterBuffer {
    pub fn new(now: Instant) -> Self {
        Self {
            slots: (0..CAPACITY)
                .map(|_| Slot {
                    seq: 0,
                    filled: false,
                    data: Vec::with_capacity(MAX_PAYLOAD),
                })
                .collect(),
            state: State::Idle,
            next: 0,
            newest: 0,
            target: MIN_TARGET,
            dry_run: 0,
            fresh: true,
            window_pops: 0,
            window_min_depth: usize::MAX,
            epoch: now,
            baseline_ms: None,
            peak_delay_ms: 0.0,
            stats: JitterStats::default(),
        }
    }

    pub fn stats(&self) -> JitterStats {
        self.stats
    }

    pub fn target(&self) -> usize {
        self.target
    }

    pub fn depth(&self) -> usize {
        match self.state {
            State::Idle => 0,
            _ => (self.newest.wrapping_sub(self.next) as i32 + 1).max(0) as usize,
        }
    }

    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.slots.iter_mut().for_each(|s| s.filled = false);
        self.dry_run = 0;
        self.window_pops = 0;
        self.window_min_depth = usize::MAX;
    }

    pub fn insert(&mut self, seq: u32, payload: &[u8], now: Instant) {
        if payload.is_empty() || payload.len() > MAX_PAYLOAD {
            return;
        }
        self.stats.received += 1;
        let restarted = (seq.wrapping_sub(self.next) as i32).unsigned_abs() > RESYNC_DISTANCE;
        if self.state != State::Idle && restarted {
            self.reset();
            self.baseline_ms = None;
        }
        self.observe_arrival(seq, now);

        if self.state == State::Idle {
            self.state = State::Buffering;
            self.fresh = true;
            self.next = seq;
            self.newest = seq;
        }

        let mut ahead = seq.wrapping_sub(self.next) as i32;
        if self.state == State::Buffering && ahead != 0 {
            let nothing_buffered = self.depth() == 0;
            let room_behind = (self.newest.wrapping_sub(seq) as usize) < CAPACITY;
            if (ahead < 0 && self.fresh && room_behind) || (ahead > 0 && nothing_buffered) {
                self.next = seq;
                if nothing_buffered {
                    self.newest = seq;
                }
                ahead = 0;
            }
        }
        if ahead < 0 {
            self.stats.late += 1;
            return;
        }
        if ahead as usize >= CAPACITY {
            let new_next = seq.wrapping_sub(self.target as u32 - 1);
            self.stats.trimmed += u64::from(new_next.wrapping_sub(self.next)).min(CAPACITY as u64);
            self.discard_until(new_next);
        }

        let slot = &mut self.slots[seq as usize % CAPACITY];
        slot.seq = seq;
        slot.filled = true;
        slot.data.clear();
        slot.data.extend_from_slice(payload);
        if (seq.wrapping_sub(self.newest) as i32) > 0 {
            self.newest = seq;
        }
    }

    pub fn pop(&mut self) -> Playout<'_> {
        match self.state {
            State::Idle => return Playout::Silence,
            State::Buffering if self.depth() < self.target => return Playout::Silence,
            State::Buffering => {
                self.state = State::Playing;
                self.fresh = false;
                self.dry_run = 0;
                self.window_pops = 0;
                self.window_min_depth = usize::MAX;
            }
            State::Playing => {}
        }

        self.correct_drift();

        let seq = self.next;
        let depth = self.depth();
        if depth == 0 || (self.dry_run > 0 && depth < self.target) {
            if self.dry_run == 0 {
                self.stats.underruns += 1;
            }
            self.dry_run += 1;
            if self.dry_run > MAX_DRY_CONCEAL {
                self.state = State::Buffering;
                return Playout::Silence;
            }
            return Playout::Conceal;
        }
        self.dry_run = 0;
        self.next = seq.wrapping_add(1);

        if self.take(seq) {
            return Playout::Packet(&self.slots[seq as usize % CAPACITY].data);
        }
        self.stats.lost += 1;
        let following = seq.wrapping_add(1);
        if self.holds(following) {
            self.stats.recovered_by_fec += 1;
            return Playout::Fec(&self.slots[following as usize % CAPACITY].data);
        }
        Playout::Conceal
    }

    fn correct_drift(&mut self) {
        self.window_min_depth = self.window_min_depth.min(self.depth());
        self.window_pops += 1;
        if self.window_pops < TRIM_WINDOW {
            return;
        }
        let excess = self.window_min_depth.saturating_sub(self.target);
        if excess > 0 {
            self.stats.trimmed += excess as u64;
            self.discard_until(self.next.wrapping_add(excess as u32));
        }
        self.window_pops = 0;
        self.window_min_depth = usize::MAX;
    }

    fn discard_until(&mut self, new_next: u32) {
        let count = new_next.wrapping_sub(self.next).min(CAPACITY as u32);
        for i in 0..count {
            self.take(self.next.wrapping_add(i));
        }
        self.next = new_next;
        if (self.newest.wrapping_sub(new_next) as i32) < -1 {
            self.newest = new_next.wrapping_sub(1);
        }
    }

    fn holds(&self, seq: u32) -> bool {
        let slot = &self.slots[seq as usize % CAPACITY];
        slot.filled && slot.seq == seq
    }

    fn take(&mut self, seq: u32) -> bool {
        let held = self.holds(seq);
        if held {
            self.slots[seq as usize % CAPACITY].filled = false;
        }
        held
    }

    fn observe_arrival(&mut self, seq: u32, now: Instant) {
        let arrival_ms = now.saturating_duration_since(self.epoch).as_secs_f64() * 1000.0;

        let transit = arrival_ms - f64::from(seq) * f64::from(FRAME_MS);
        let baseline = match self.baseline_ms {
            Some(b) if transit - b < DISCONTINUITY_MS => (b + BASELINE_LEAK_MS).min(transit),

            _ => transit,
        };
        self.baseline_ms = Some(baseline);

        self.peak_delay_ms = (self.peak_delay_ms * PEAK_DECAY).max(transit - baseline);
        let frames = (self.peak_delay_ms / f64::from(FRAME_MS)).ceil() as usize + 1;
        self.target = frames.clamp(MIN_TARGET, MAX_TARGET);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    struct Rig {
        jb: JitterBuffer,
        t0: Instant,
    }

    impl Rig {
        fn new() -> Self {
            let t0 = Instant::now();
            Self {
                jb: JitterBuffer::new(t0),
                t0,
            }
        }
        fn put(&mut self, seq: u32, late_ms: u64) {
            let at = self.t0 + Duration::from_millis(u64::from(seq) * 10 + late_ms);
            self.jb.insert(seq, &[seq as u8, 1], at);
        }
        fn pop_kind(&mut self) -> &'static str {
            match self.jb.pop() {
                Playout::Silence => "silence",
                Playout::Packet(_) => "packet",
                Playout::Fec(_) => "fec",
                Playout::Conceal => "conceal",
            }
        }
    }

    #[test]
    fn buffers_until_target_then_plays_in_order() {
        let mut r = Rig::new();
        assert_eq!(r.jb.pop(), Playout::Silence);
        r.put(100, 0);
        assert_eq!(
            r.jb.pop(),
            Playout::Silence,
            "one frame is below the target of two"
        );
        r.put(101, 0);
        assert_eq!(r.jb.pop(), Playout::Packet(&[100, 1]));
        r.put(102, 0);
        assert_eq!(r.jb.pop(), Playout::Packet(&[101, 1]));
        assert_eq!(r.jb.pop(), Playout::Packet(&[102, 1]));
    }

    #[test]
    fn reordered_packets_play_in_order_and_late_ones_are_dropped() {
        let mut r = Rig::new();
        for seq in [1, 0, 3, 2] {
            r.put(seq, 0);
        }
        for seq in 0..4u8 {
            assert_eq!(r.jb.pop(), Playout::Packet(&[seq, 1]));
        }
        r.put(2, 0);
        assert_eq!(r.jb.stats().late, 1);
    }

    #[test]
    fn single_loss_uses_fec_and_double_loss_conceals() {
        let mut r = Rig::new();
        for seq in [0, 1, 3, 6, 7] {
            r.put(seq, 0);
        }
        let kinds: Vec<_> = (0..7).map(|_| r.pop_kind()).collect();
        assert_eq!(
            kinds,
            ["packet", "packet", "fec", "packet", "conceal", "fec", "packet"]
        );
        let s = r.jb.stats();
        assert_eq!((s.lost, s.recovered_by_fec), (3, 2));
    }

    #[test]
    fn dry_buffer_conceals_briefly_then_rebuffers() {
        let mut r = Rig::new();
        r.put(0, 0);
        r.put(1, 0);
        assert_eq!(r.pop_kind(), "packet");
        assert_eq!(r.pop_kind(), "packet");
        for _ in 0..MAX_DRY_CONCEAL {
            assert_eq!(r.pop_kind(), "conceal");
        }
        assert_eq!(r.pop_kind(), "silence");
        assert_eq!(r.jb.stats().underruns, 1);

        r.put(52, 0);
        assert_eq!(r.pop_kind(), "silence", "rebuffering to the target first");
        r.put(53, 0);
        assert_eq!(
            r.jb.pop(),
            Playout::Packet(&[52, 1]),
            "the muted gap is skipped, not concealed"
        );
        assert_eq!(r.jb.target(), MIN_TARGET, "a pause is not jitter");
    }

    #[test]
    fn a_stalled_talker_does_not_inflate_the_target() {
        let mut r = Rig::new();
        for seq in 0..50 {
            r.put(seq, 0);
            r.jb.pop();
        }

        for seq in 50..100 {
            r.put(seq, 400);
            r.jb.pop();
        }
        assert_eq!(r.jb.target(), MIN_TARGET);
    }

    #[test]
    fn a_frame_that_arrives_during_concealment_is_still_played() {
        let mut r = Rig::new();
        r.put(0, 0);
        r.put(1, 0);
        r.pop_kind();
        r.pop_kind();
        assert_eq!(r.pop_kind(), "conceal");
        r.put(2, 5);
        assert_eq!(
            r.pop_kind(),
            "conceal",
            "the safety margin is rebuilt before resuming"
        );
        r.put(3, 5);
        assert_eq!(r.jb.pop(), Playout::Packet(&[2, 1]));
        assert_eq!(r.jb.pop(), Playout::Packet(&[3, 1]));
        assert_eq!(r.jb.stats().late, 0);
    }

    #[test]
    fn sender_clock_running_fast_is_trimmed_not_accumulated() {
        let mut r = Rig::new();
        let mut seq = 0u32;

        for tick in 0..3000u32 {
            r.put(seq, 0);
            seq += 1;
            if tick % 100 == 0 {
                r.put(seq, 0);
                seq += 1;
            }
            r.jb.pop();
            assert!(
                r.jb.depth() <= MIN_TARGET + 3,
                "depth {} at tick {tick}",
                r.jb.depth()
            );
        }
        assert!(r.jb.stats().trimmed >= 25);
        assert_eq!(r.jb.stats().lost, 0);
    }

    #[test]
    fn sender_clock_running_slow_recovers_without_runaway() {
        let mut r = Rig::new();
        let mut seq = 0u32;
        let mut played = 0u32;
        for tick in 0..3000u32 {
            if tick % 100 != 99 {
                r.put(seq, 0);
                seq += 1;
            }
            if matches!(r.jb.pop(), Playout::Packet(_)) {
                played += 1;
            }
        }
        assert!(played >= seq - 4, "played {played} of {seq}");
        assert_eq!(r.jb.stats().late, 0);
    }

    #[test]
    fn target_grows_with_jitter_and_shrinks_back() {
        let mut r = Rig::new();
        for seq in 0..200 {
            r.put(seq, if seq % 10 == 0 { 45 } else { 0 });
            r.jb.pop();
        }
        assert!(r.jb.target() >= 5, "target {}", r.jb.target());
        for seq in 200..6000 {
            r.put(seq, 0);
            r.jb.pop();
        }
        assert_eq!(r.jb.target(), MIN_TARGET);
    }

    #[test]
    fn burst_beyond_capacity_jumps_to_the_live_edge() {
        let mut r = Rig::new();
        r.put(0, 0);
        r.put(1, 0);
        r.pop_kind();
        for seq in 2..200 {
            r.put(seq, 0);
        }
        assert!(r.jb.depth() <= CAPACITY);
        assert_eq!(r.pop_kind(), "packet");
        assert!(r.jb.depth() <= MAX_TARGET);
    }

    #[test]
    fn sequence_restart_resyncs() {
        let mut r = Rig::new();
        r.put(50_000, 0);
        r.put(50_001, 0);
        r.pop_kind();
        r.put(5, 0);
        r.put(6, 0);
        assert_eq!(r.jb.pop(), Playout::Packet(&[5, 1]));
        assert_eq!(r.jb.stats().received, 4, "each packet is counted once");
    }

    #[test]
    fn sequence_wraparound_is_seamless() {
        let mut r = Rig::new();
        let start = u32::MAX - 2;
        for i in 0..6u32 {
            let seq = start.wrapping_add(i);
            r.jb.insert(
                seq,
                &[i as u8, 1],
                r.t0 + Duration::from_millis(u64::from(i) * 10),
            );
        }
        for i in 0..6u8 {
            assert_eq!(r.jb.pop(), Playout::Packet(&[i, 1]));
        }
    }
}
