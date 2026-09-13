pub mod cpal_backend;
#[cfg(windows)]
mod devices;
pub mod fake;

use std::{
    any::Any,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc, OnceLock,
    },
    thread::Thread,
};

pub const RING_MS: u32 = 200;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no {0} device available")]
    NoDevice(&'static str),
    #[error("{kind} device \"{name}\" not found")]
    NotFound { kind: &'static str, name: String },
    #[error("unsupported sample format {0}")]
    Format(String),
    #[error("audio device error: {0}")]
    Backend(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceList {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Wake(OnceLock<Thread>);

impl Wake {
    pub fn wake(&self) {
        if let Some(thread) = self.0.get() {
            thread.unpark();
        }
    }

    pub fn attach(&self, thread: Thread) {
        let _ = self.0.set(thread);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Invalidated,
    Broken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Capture,
    Playback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceNews {
    Changed,
    NewDefault(Flow),
}

pub type Alarm = Box<dyn Fn(Failure) + Send>;
pub type DevicesChanged = Arc<dyn Fn(DeviceNews) + Send + Sync>;
pub type DeviceWatch = Box<dyn Any + Send>;

#[derive(Debug, Default)]
pub struct StreamShared {
    lost: AtomicU64,
    period_frames: AtomicU32,
}

impl StreamShared {
    pub fn period(&self) -> u32 {
        self.period_frames.load(Ordering::Relaxed)
    }

    pub fn lost(&self) -> u64 {
        self.lost.load(Ordering::Relaxed)
    }

    pub(crate) fn count_lost(&self, how_many: u64) {
        self.lost.fetch_add(how_many, Ordering::Relaxed);
    }

    pub(crate) fn set_period(&self, frames: usize) {
        self.period_frames.store(frames as u32, Ordering::Relaxed);
    }
}

pub trait Stream {
    fn play(&self) -> Result<(), AudioError>;
}

pub struct CaptureStream {
    pub sample_rate: u32,
    pub samples: rtrb::Consumer<f32>,
    pub shared: Arc<StreamShared>,
    pub stream: Box<dyn Stream>,
}

pub struct PlaybackStream {
    pub sample_rate: u32,
    pub samples: rtrb::Producer<f32>,
    pub shared: Arc<StreamShared>,
    pub stream: Box<dyn Stream>,
}

pub trait AudioBackend: Send + Sync {
    fn devices(&self) -> DeviceList;

    fn open_capture(
        &self,
        device: Option<&str>,
        wake: Arc<Wake>,
        alarm: Alarm,
    ) -> Result<CaptureStream, AudioError>;

    fn open_playback(
        &self,
        device: Option<&str>,
        wake: Arc<Wake>,
        alarm: Alarm,
    ) -> Result<PlaybackStream, AudioError>;

    fn watch_devices(&self, changed: DevicesChanged) -> Result<DeviceWatch, AudioError>;
}

pub(crate) fn ring_capacity(sample_rate: u32) -> usize {
    (sample_rate as usize * RING_MS as usize) / 1000
}

#[derive(Default)]
pub(crate) struct Underruns {
    primed: bool,
}

impl Underruns {
    pub(crate) fn is_underrun(&mut self, wanted: usize, available: usize) -> bool {
        if available >= wanted {
            self.primed = true;
        }
        self.primed && available < wanted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_play_before_the_first_sound_is_no_underrun() {
        let mut underruns = Underruns::default();
        assert!(!underruns.is_underrun(480, 0));
        assert!(!underruns.is_underrun(480, 100));
        assert!(!underruns.is_underrun(480, 480));
        assert!(underruns.is_underrun(480, 100));
    }
}
