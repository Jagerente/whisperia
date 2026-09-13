use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use super::{
    ring_capacity, Alarm, AudioBackend, AudioError, CaptureStream, DeviceList, DeviceNews,
    DeviceWatch, DevicesChanged, Failure, Flow, PlaybackStream, Stream, StreamShared, Underruns,
    Wake,
};

const TICK: Duration = Duration::from_millis(10);
const DEVICE_BUFFER_TICKS: u32 = 10;

#[derive(Clone)]
pub struct FakeBackend {
    pub sample_rate: u32,
    pub tone: Option<(f32, f32)>,
    pub tone_on: Arc<AtomicBool>,
    pub clock_skew_ppm: f64,

    pub played: Arc<Mutex<Vec<f32>>>,
    pub record: bool,

    pub microphones_opened: Arc<AtomicUsize>,
    pub speakers_opened: Arc<AtomicUsize>,
    pub microphone_broken: Arc<AtomicBool>,
    pub speakers_broken: Arc<AtomicBool>,
    pub microphone_missing: Arc<AtomicBool>,
    pub listed: Arc<Mutex<DeviceList>>,
    pub speakers_playing: Arc<AtomicUsize>,
    pub playback_started: Arc<Mutex<Option<Instant>>>,
    unplugs: Arc<AtomicU64>,
    watchers: Arc<Mutex<Vec<DevicesChanged>>>,
}

impl FakeBackend {
    pub fn quiet(sample_rate: u32, tone: Option<(f32, f32)>) -> Self {
        Self {
            record: false,
            ..Self::new(sample_rate, tone)
        }
    }

    pub fn new(sample_rate: u32, tone: Option<(f32, f32)>) -> Self {
        Self {
            sample_rate,
            tone,
            tone_on: Arc::new(AtomicBool::new(true)),
            clock_skew_ppm: 0.0,
            played: Arc::default(),
            record: true,
            microphones_opened: Arc::default(),
            speakers_opened: Arc::default(),
            microphone_broken: Arc::default(),
            speakers_broken: Arc::default(),
            microphone_missing: Arc::default(),
            listed: Arc::new(Mutex::new(DeviceList {
                inputs: vec!["Fake microphone".into()],
                outputs: vec!["Fake speaker".into()],
            })),
            speakers_playing: Arc::default(),
            playback_started: Arc::default(),
            unplugs: Arc::default(),
            watchers: Arc::default(),
        }
    }

    fn tick(&self) -> Duration {
        TICK.div_f64(1.0 + self.clock_skew_ppm / 1e6)
    }

    pub fn move_default_devices(&self) {
        self.tell(DeviceNews::NewDefault(Flow::Capture));
        self.tell(DeviceNews::NewDefault(Flow::Playback));
    }

    pub fn unplug_devices(&self) {
        self.unplugs.fetch_add(1, Ordering::Relaxed);
        self.devices_changed();
    }

    pub fn devices_changed(&self) {
        self.tell(DeviceNews::Changed);
    }

    fn tell(&self, news: DeviceNews) {
        let watchers = self
            .watchers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        for changed in watchers {
            changed(news);
        }
    }

    fn trouble_watch(&self, broken: &Arc<AtomicBool>, alarm: Alarm) -> impl FnMut() + Send {
        let (broken, unplugs) = (broken.clone(), self.unplugs.clone());
        let opened_at = unplugs.load(Ordering::Relaxed);
        let mut raised = false;
        move || {
            if raised {
                return;
            }
            let failure = if unplugs.load(Ordering::Relaxed) != opened_at {
                Some(Failure::Invalidated)
            } else if broken.load(Ordering::Relaxed) {
                Some(Failure::Broken)
            } else {
                None
            };
            if let Some(failure) = failure {
                raised = true;
                alarm(failure);
            }
        }
    }
}

type Tick = Box<dyn FnMut() + Send>;

struct Clock {
    name: &'static str,
    period: Duration,
    body: Mutex<Option<Tick>>,
    stop: Arc<AtomicBool>,
    playing: Option<Arc<AtomicUsize>>,
}

impl Clock {
    fn new(name: &'static str, period: Duration, body: Tick) -> Self {
        Self {
            name,
            period,
            body: Mutex::new(Some(body)),
            stop: Arc::new(AtomicBool::new(false)),
            playing: None,
        }
    }
}

impl Stream for Clock {
    fn play(&self) -> Result<(), AudioError> {
        let Some(mut body) = self.body.lock().unwrap_or_else(|p| p.into_inner()).take() else {
            return Ok(());
        };
        let (stop, period, playing) = (self.stop.clone(), self.period, self.playing.clone());
        if let Some(playing) = &playing {
            playing.fetch_add(1, Ordering::Relaxed);
        }
        std::thread::Builder::new()
            .name(self.name.to_owned())
            .spawn(move || {
                let mut deadline = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    body();
                    let now = Instant::now();
                    deadline = next_deadline(deadline, period, now);
                    if let Some(left) = deadline.checked_duration_since(now) {
                        std::thread::sleep(left);
                    }
                }
                if let Some(playing) = playing {
                    playing.fetch_sub(1, Ordering::Relaxed);
                }
            })
            .map(drop)
            .map_err(|e| AudioError::Backend(format!("cannot start fake audio thread: {e}")))
    }
}

fn next_deadline(deadline: Instant, tick: Duration, now: Instant) -> Instant {
    let due = deadline + tick;
    if now.saturating_duration_since(due) > tick * DEVICE_BUFFER_TICKS {
        now
    } else {
        due
    }
}

impl Drop for Clock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl AudioBackend for FakeBackend {
    fn devices(&self) -> DeviceList {
        self.listed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn open_capture(
        &self,
        _device: Option<&str>,
        wake: Arc<Wake>,
        alarm: Alarm,
    ) -> Result<CaptureStream, AudioError> {
        if self.microphone_missing.load(Ordering::Relaxed) {
            return Err(AudioError::NoDevice("input"));
        }
        if self.microphone_broken.load(Ordering::Relaxed) {
            return Err(AudioError::Backend("the fake microphone is broken".into()));
        }
        self.microphones_opened.fetch_add(1, Ordering::Relaxed);
        let rate = self.sample_rate;
        let per_tick = (rate / 100) as usize;
        let (mut producer, consumer) = rtrb::RingBuffer::new(ring_capacity(rate));
        let shared = Arc::new(StreamShared::default());

        let (tone, tone_on, clocked) = (self.tone, self.tone_on.clone(), shared.clone());
        let mut trouble = self.trouble_watch(&self.microphone_broken, alarm);
        let mut n = 0u64;
        let clock = Clock::new(
            "fake-capture",
            self.tick(),
            Box::new(move || {
                trouble();
                clocked.set_period(per_tick);
                let tone = tone.filter(|_| tone_on.load(Ordering::Relaxed));
                let mut lost = 0;
                for _ in 0..per_tick {
                    let sample = tone.map_or(0.0, |(hz, amp)| {
                        amp * (n as f32 / rate as f32 * hz * std::f32::consts::TAU).sin()
                    });
                    n = (n + 1) % u64::from(rate);
                    if producer.push(sample).is_err() {
                        lost += 1;
                    }
                }
                if lost > 0 {
                    clocked.count_lost(lost);
                }
                wake.wake();
            }),
        );

        Ok(CaptureStream {
            sample_rate: rate,
            samples: consumer,
            shared,
            stream: Box::new(clock),
        })
    }

    fn open_playback(
        &self,
        _device: Option<&str>,
        wake: Arc<Wake>,
        alarm: Alarm,
    ) -> Result<PlaybackStream, AudioError> {
        if self.speakers_broken.load(Ordering::Relaxed) {
            return Err(AudioError::Backend("the fake speakers are broken".into()));
        }
        self.speakers_opened.fetch_add(1, Ordering::Relaxed);
        let rate = self.sample_rate;
        let per_tick = (rate / 100) as usize;
        let (producer, mut consumer) = rtrb::RingBuffer::new(ring_capacity(rate));
        let shared = Arc::new(StreamShared::default());

        let (played, record, clocked) = (self.played.clone(), self.record, shared.clone());
        let started = self.playback_started.clone();
        let mut trouble = self.trouble_watch(&self.speakers_broken, alarm);
        let mut underruns = Underruns::default();
        let mut clock = Clock::new(
            "fake-playback",
            self.tick(),
            Box::new(move || {
                trouble();
                started
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get_or_insert_with(Instant::now);
                clocked.set_period(per_tick);
                let available = consumer.slots().min(per_tick);
                let mut sink = record.then(|| played.lock().unwrap_or_else(|p| p.into_inner()));
                for _ in 0..per_tick {
                    let sample = consumer.pop().unwrap_or(0.0);
                    if let Some(sink) = sink.as_mut() {
                        sink.push(sample);
                    }
                }
                if underruns.is_underrun(per_tick, available) {
                    clocked.count_lost(1);
                }
                wake.wake();
            }),
        );
        clock.playing = Some(self.speakers_playing.clone());

        Ok(PlaybackStream {
            sample_rate: rate,
            samples: producer,
            shared,
            stream: Box::new(clock),
        })
    }

    fn watch_devices(&self, changed: DevicesChanged) -> Result<DeviceWatch, AudioError> {
        self.watchers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(changed);
        Ok(Box::new(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_on_time_keeps_its_beat() {
        let start = Instant::now();
        let now = start + TICK / 2;
        assert_eq!(next_deadline(start, TICK, now), start + TICK);
    }

    #[test]
    fn a_clock_late_by_less_than_the_device_buffer_catches_up_with_its_beat() {
        let start = Instant::now();
        let now = start + TICK * DEVICE_BUFFER_TICKS;
        assert_eq!(next_deadline(start, TICK, now), start + TICK);
    }

    #[test]
    fn a_clock_late_by_more_than_the_device_buffer_starts_a_new_beat_instead_of_bursting() {
        let start = Instant::now();
        let now = start + TICK * (DEVICE_BUFFER_TICKS + 2);
        assert_eq!(next_deadline(start, TICK, now), now);
    }
}
