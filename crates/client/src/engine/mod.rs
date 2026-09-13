mod input;
mod output;
mod trouble;

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use bytes::Bytes;
use whisperia_proto::{audio::AudioKind, PeerId, FRAME_SAMPLES};

pub use self::trouble::AudioTrouble;
use self::{
    input::{Capture, InputCommand, InputParts},
    output::{OutputCommand, OutputParts},
};
use crate::{
    audio::{
        Alarm, AudioBackend, AudioError, DeviceNews, DeviceWatch, Failure, Flow, Stream,
        StreamShared, Wake,
    },
    codec::{CodecError, VoiceEncoder},
    dsp::{
        cue::{Cue, CueRequests},
        denoise::DenoiseMode,
        level::{AtomicF32, SILENCE_DB},
        resample::{FromWire, ResampleError, ToWire},
    },
    jitter::JitterStats,
    wake::{self, Waker, WakingSender},
};

const RETRY: Duration = Duration::from_secs(2);
const SPEAKERS_KEPT_FOR_SOUNDS: Duration = Duration::from_secs(4);
const RECEIVED_QUEUE: usize = 512;
const LOOPBACK_CAPACITY: usize = FRAME_SAMPLES * 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Microphone,
    Speakers,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("microphone: {0}")]
    Microphone(AudioError),
    #[error("speakers: {0}")]
    Speakers(AudioError),
    #[error(transparent)]
    Resample(#[from] ResampleError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("cannot start audio thread: {0}")]
    Thread(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Mode {
    Idle,
    Room,
    MicTest,
}

impl Mode {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Room,
            2 => Self::MicTest,
            _ => Self::Idle,
        }
    }
}

#[derive(Debug)]
pub struct Controls {
    pub muted: AtomicBool,
    pub deafened: AtomicBool,
    denoise: AtomicU8,
    pub mic_gain_db: AtomicF32,
    pub volume_db: AtomicF32,
    pub agc: AtomicBool,
    mode: AtomicU8,
    cues: CueRequests,
    voice_volumes: Mutex<Vec<(PeerId, f32)>>,
    share_volumes: Mutex<Vec<(PeerId, f32)>>,
    volumes_version: AtomicU32,
}

impl Controls {
    pub fn new(denoise: DenoiseMode, mic_gain_db: f32, volume_db: f32, agc: bool) -> Self {
        Self {
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            denoise: AtomicU8::new(denoise.as_u8()),
            mic_gain_db: AtomicF32::new(mic_gain_db),
            volume_db: AtomicF32::new(volume_db),
            agc: AtomicBool::new(agc),
            mode: AtomicU8::new(Mode::Idle as u8),
            cues: CueRequests::default(),
            voice_volumes: Mutex::new(Vec::new()),
            share_volumes: Mutex::new(Vec::new()),
            volumes_version: AtomicU32::new(0),
        }
    }

    pub fn denoise(&self) -> DenoiseMode {
        DenoiseMode::from_u8(self.denoise.load(Ordering::Relaxed))
    }

    pub fn set_denoise(&self, mode: DenoiseMode) {
        self.denoise.store(mode.as_u8(), Ordering::Relaxed);
    }

    pub fn set_volumes(&self, voice: &[(PeerId, f32)], share: &[(PeerId, f32)]) {
        let changed =
            Self::replace(&self.voice_volumes, voice) | Self::replace(&self.share_volumes, share);
        if changed {
            self.volumes_version.fetch_add(1, Ordering::Release);
        }
    }

    fn replace(table: &Mutex<Vec<(PeerId, f32)>>, wanted: &[(PeerId, f32)]) -> bool {
        let mut held = table.lock().unwrap_or_else(|p| p.into_inner());
        if held.as_slice() == wanted {
            return false;
        }
        held.clear();
        held.extend_from_slice(wanted);
        true
    }

    fn volumes_version(&self) -> u32 {
        self.volumes_version.load(Ordering::Acquire)
    }

    fn volume_db(&self, id: PeerId, kind: AudioKind) -> f32 {
        let table = match kind {
            AudioKind::Voice => &self.voice_volumes,
            AudioKind::Share => &self.share_volumes,
        };
        table
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|(peer, _)| *peer == id)
            .map_or(0.0, |(_, db)| *db)
    }

    fn mode(&self) -> Mode {
        Mode::from_u8(self.mode.load(Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PeerTelemetry {
    pub id: PeerId,
    pub level_db: f32,
    pub talking: bool,
    pub jitter: JitterStats,
    pub target_frames: usize,
    pub depth_frames: usize,
}

#[derive(Debug)]
pub struct Telemetry {
    pub mic_db: AtomicF32,
    pub mic_talking: AtomicBool,
    pub agc_gain_db: AtomicF32,
    pub frames_sent: AtomicU64,
    peers: Mutex<Vec<PeerTelemetry>>,
    shares: Mutex<Vec<PeerTelemetry>>,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            mic_db: AtomicF32::new(SILENCE_DB),
            mic_talking: AtomicBool::new(false),
            agc_gain_db: AtomicF32::new(0.0),
            frames_sent: AtomicU64::new(0),
            peers: Mutex::new(Vec::new()),
            shares: Mutex::new(Vec::new()),
        }
    }
}

impl Telemetry {
    pub fn peers(&self) -> Vec<PeerTelemetry> {
        self.peers.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn shares(&self) -> Vec<PeerTelemetry> {
        self.shares
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn quiet(&self) {
        self.mic_db.set(SILENCE_DB);
        self.mic_talking.store(false, Ordering::Relaxed);
        self.peers.lock().unwrap_or_else(|p| p.into_inner()).clear();
        self.shares
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Losses {
    pub microphone_samples: u64,
    pub speaker_underruns: u64,
}

pub type Received = (Bytes, Instant);
pub type Outgoing = tokio::sync::mpsc::Sender<Bytes>;

struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn(
        name: &str,
        wake: &Wake,
        run: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || run(stopped))?;
        wake.attach(thread.thread().clone());
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

struct Microphone {
    orders: Option<Sender<InputCommand>>,
    wake: Arc<Wake>,
    thread: Option<JoinHandle<()>>,
}

impl Microphone {
    fn start(parts: impl FnOnce(Receiver<InputCommand>) -> InputParts) -> std::io::Result<Self> {
        let (orders, received) = mpsc::channel();
        let parts = parts(received);
        let thread = std::thread::Builder::new()
            .name("ww-microphone".into())
            .spawn(move || input::run(parts))?;
        let wake = Arc::new(Wake::default());
        wake.attach(thread.thread().clone());
        Ok(Self {
            orders: Some(orders),
            wake,
            thread: Some(thread),
        })
    }

    fn tell(&self, order: InputCommand) {
        if let Some(orders) = &self.orders {
            let _ = orders.send(order);
        }
        self.wake.wake();
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        self.orders = None;
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

struct Input {
    id: u64,
    stream: Box<dyn Stream>,
    shared: Arc<StreamShared>,
}

struct Output {
    id: u64,
    stream: Box<dyn Stream>,
    shared: Arc<StreamShared>,
    commands: Sender<OutputCommand>,
    worker: Worker,
}

enum AudioEvent {
    Failed {
        side: Side,
        stream: u64,
        failure: Failure,
    },
    Devices(DeviceNews),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Wait {
    #[default]
    Nothing,
    Until(Instant),
    ForDevices,
}

#[derive(Debug, Default)]
struct Health {
    trouble: Option<AudioTrouble>,
    wait: Wait,
}

impl Health {
    fn may_open(&self, now: Instant) -> bool {
        match self.wait {
            Wait::Nothing => true,
            Wait::Until(at) => now >= at,
            Wait::ForDevices => false,
        }
    }

    fn next_try(&self) -> Option<Instant> {
        match self.wait {
            Wait::Until(at) => Some(at),
            Wait::Nothing | Wait::ForDevices => None,
        }
    }

    fn failed(&mut self, now: Instant, trouble: AudioTrouble, devices_heard: bool) {
        let a_device_brings_it_back = matches!(
            trouble,
            AudioTrouble::Missing(_) | AudioTrouble::Unplugged(..)
        );
        self.wait = match devices_heard && a_device_brings_it_back {
            true => Wait::ForDevices,
            false => Wait::Until(now + RETRY),
        };
        self.trouble = Some(trouble);
    }

    fn works(&mut self) -> bool {
        self.wait = Wait::Nothing;
        self.trouble.take().is_some()
    }

    fn devices_changed(&mut self) {
        if self.trouble.is_some() {
            self.wait = Wait::Nothing;
        }
    }
}

pub struct Engine {
    backend: Arc<dyn AudioBackend>,
    input_device: Option<String>,
    output_device: Option<String>,
    controls: Arc<Controls>,
    telemetry: Arc<Telemetry>,
    outgoing: Arc<Mutex<Option<Outgoing>>>,
    received_sink: SyncSender<Received>,
    received: Arc<Mutex<Receiver<Received>>>,
    microphone: Microphone,
    in_room: bool,
    testing: bool,
    looped: bool,
    sounds_until: Option<Instant>,
    input: Option<Input>,
    output: Option<Output>,
    microphone_health: Health,
    speakers_health: Health,
    streams_opened: u64,
    devices_news: u64,
    events: Receiver<AudioEvent>,
    heard: WakingSender<AudioEvent>,
    devices: Option<DeviceWatch>,
    retired: Losses,
}

impl Engine {
    pub fn new(
        backend: Arc<dyn AudioBackend>,
        controls: Arc<Controls>,
        waker: &Waker,
    ) -> Result<Self, EngineError> {
        let (heard, events) = wake::channel(waker);
        let told = heard.clone();
        let devices = backend
            .watch_devices(Arc::new(move |news| {
                let _ = told.send(AudioEvent::Devices(news));
            }))
            .map_err(|e| tracing::warn!("sound devices are looked for again on a timer: {e}"))
            .ok();
        let (received_sink, received) = mpsc::sync_channel(RECEIVED_QUEUE);
        let telemetry = Arc::new(Telemetry::default());
        let outgoing: Arc<Mutex<Option<Outgoing>>> = Arc::default();
        let encoder = VoiceEncoder::new()?;
        let microphone = Microphone::start(|orders| InputParts {
            encoder,
            controls: controls.clone(),
            telemetry: telemetry.clone(),
            outgoing: outgoing.clone(),
            orders,
        })?;
        Ok(Self {
            backend,
            input_device: None,
            output_device: None,
            controls,
            telemetry,
            outgoing,
            received_sink,
            received: Arc::new(Mutex::new(received)),
            microphone,
            in_room: false,
            testing: false,
            looped: false,
            sounds_until: None,
            input: None,
            output: None,
            microphone_health: Health::default(),
            speakers_health: Health::default(),
            streams_opened: 0,
            devices_news: 0,
            events,
            heard,
            devices,
            retired: Losses::default(),
        })
    }

    pub fn controls(&self) -> &Arc<Controls> {
        &self.controls
    }

    pub fn telemetry(&self) -> &Arc<Telemetry> {
        &self.telemetry
    }

    pub fn troubles(&self) -> impl Iterator<Item = &AudioTrouble> {
        [&self.microphone_health, &self.speakers_health]
            .into_iter()
            .filter_map(|health| health.trouble.as_ref())
    }

    fn health(&mut self, side: Side) -> &mut Health {
        match side {
            Side::Microphone => &mut self.microphone_health,
            Side::Speakers => &mut self.speakers_health,
        }
    }

    pub fn is_testing(&self) -> bool {
        self.testing
    }

    pub fn received_sink(&self) -> impl Fn(Bytes, Instant) + Send + Sync + 'static {
        let sink = self.received_sink.clone();
        move |datagram, at| {
            let _ = sink.try_send((datagram, at));
        }
    }

    pub fn losses(&self) -> Losses {
        let (mut losses, retired) = (Losses::default(), self.retired);
        if let Some(input) = &self.input {
            losses.microphone_samples = input.shared.lost();
        }
        if let Some(output) = &self.output {
            losses.speaker_underruns = output.shared.lost();
        }
        Losses {
            microphone_samples: retired.microphone_samples + losses.microphone_samples,
            speaker_underruns: retired.speaker_underruns + losses.speaker_underruns,
        }
    }

    pub fn use_devices(&mut self, input: Option<String>, output: Option<String>) {
        if (&self.input_device, &self.output_device) == (&input, &output) {
            return;
        }
        (self.input_device, self.output_device) = (input, output);
        self.close_input();
        self.close_output();
        self.microphone_health = Health::default();
        self.speakers_health = Health::default();
        self.settle(Instant::now());
    }

    pub fn enter_room(&mut self, outgoing: Outgoing) {
        *self.outgoing.lock().unwrap_or_else(|p| p.into_inner()) = Some(outgoing);
        self.in_room = true;
        self.settle(Instant::now());
    }

    pub fn leave_room(&mut self) {
        *self.outgoing.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.in_room = false;
        self.settle(Instant::now());
    }

    pub fn test_microphone(&mut self, on: bool) {
        if self.testing == on {
            return;
        }
        self.testing = on;
        if !on {
            self.telemetry.quiet();
        }
        self.settle(Instant::now());
    }

    pub fn play(&mut self, cue: Cue) {
        self.controls.cues.request(cue);
        let now = Instant::now();
        self.sounds_until = Some(now + SPEAKERS_KEPT_FOR_SOUNDS);
        self.settle(now);
    }

    pub fn forget(&self, peer: PeerId) {
        self.tell_output(OutputCommand::Forget(peer));
    }

    pub fn forget_share(&self, peer: PeerId) {
        self.tell_output(OutputCommand::ForgetShare(peer));
    }

    pub fn next_tick(&self) -> Option<Instant> {
        [
            self.microphone_health.next_try(),
            self.speakers_health.next_try(),
            self.sounds_until,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub fn devices_news(&self) -> u64 {
        self.devices_news
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let before: Vec<AudioTrouble> = self.troubles().cloned().collect();
        let news = self.devices_news;
        self.settle(now);
        !self.troubles().eq(before.iter()) || self.devices_news != news
    }

    fn tell_output(&self, command: OutputCommand) {
        if let Some(output) = &self.output {
            let _ = output.commands.send(command);
        }
    }

    fn mode(&self) -> Mode {
        match (self.testing, self.in_room) {
            (true, _) => Mode::MicTest,
            (false, true) => Mode::Room,
            (false, false) => Mode::Idle,
        }
    }

    fn settle(&mut self, now: Instant) {
        let mode = self.mode();
        self.controls.mode.store(mode as u8, Ordering::Relaxed);
        if self.sounds_until.is_some_and(|until| now >= until) {
            self.sounds_until = None;
        }

        while let Ok(event) = self.events.try_recv() {
            self.hear(now, event);
        }

        let wants_input = mode != Mode::Idle;
        let wants_output = wants_input || self.sounds_until.is_some();
        if !wants_input {
            self.close_input();
            self.microphone_health = Health::default();
        }
        if !wants_output {
            self.close_output();
            self.speakers_health = Health::default();
        }

        let mut opened = false;
        if wants_output && self.output.is_none() && self.speakers_health.may_open(now) {
            match self.open_output() {
                Ok(output) => {
                    self.output = Some(output);
                    opened = true;
                    self.works_again(Side::Speakers);
                }
                Err(e) => self.failed_to_open(now, Side::Speakers, &e),
            }
        }
        if wants_input && self.input.is_none() && self.microphone_health.may_open(now) {
            match self.open_input() {
                Ok(input) => {
                    self.input = Some(input);
                    opened = true;
                    self.works_again(Side::Microphone);
                }
                Err(e) => self.failed_to_open(now, Side::Microphone, &e),
            }
        }
        let looping = mode == Mode::MicTest;
        if opened || looping != self.looped {
            self.loop_back(looping);
        }
    }

    fn hear(&mut self, now: Instant, event: AudioEvent) {
        let (side, stream, failure) = match event {
            AudioEvent::Devices(news) => return self.hear_of_devices(news),
            AudioEvent::Failed {
                side,
                stream,
                failure,
            } => (side, stream, failure),
        };
        let current = match side {
            Side::Microphone => self.input.as_ref().map(|input| input.id),
            Side::Speakers => self.output.as_ref().map(|output| output.id),
        };
        if current != Some(stream) {
            return;
        }
        match side {
            Side::Microphone => self.close_input(),
            Side::Speakers => self.close_output(),
        }
        match failure {
            Failure::Invalidated => tracing::info!("the {side:?} changed, opening them again"),
            Failure::Broken => {
                tracing::warn!("the {side:?} stopped working");
                self.health(side)
                    .failed(now, AudioTrouble::Broken(side), false);
            }
        }
    }

    fn hear_of_devices(&mut self, news: DeviceNews) {
        self.devices_news += 1;
        self.microphone_health.devices_changed();
        self.speakers_health.devices_changed();
        let DeviceNews::NewDefault(flow) = news else {
            return;
        };
        match flow {
            Flow::Capture if self.input_device.is_none() && self.input.is_some() => {
                tracing::info!("a new default microphone, following it");
                self.close_input();
            }
            Flow::Playback if self.output_device.is_none() && self.output.is_some() => {
                tracing::info!("new default speakers, following them");
                self.close_output();
            }
            Flow::Capture | Flow::Playback => {}
        }
    }

    fn works_again(&mut self, side: Side) {
        if self.health(side).works() {
            tracing::info!("the {side:?} work again");
        }
    }

    fn failed_to_open(&mut self, now: Instant, side: Side, error: &EngineError) {
        let trouble = AudioTrouble::from(error);
        let devices_heard = self.devices.is_some();
        let health = self.health(side);
        if health.trouble.as_ref() != Some(&trouble) {
            tracing::warn!("the {side:?} do not start: {error}");
        }
        health.failed(now, trouble, devices_heard);
    }

    fn alarm(&self, side: Side, stream: u64) -> Alarm {
        let heard = self.heard.clone();
        Box::new(move |failure| {
            let _ = heard.send(AudioEvent::Failed {
                side,
                stream,
                failure,
            });
        })
    }

    fn next_stream(&mut self) -> u64 {
        self.streams_opened += 1;
        self.streams_opened
    }

    fn loop_back(&mut self, on: bool) {
        let (Some(_), Some(output)) = (&self.input, &self.output) else {
            self.looped = false;
            return;
        };
        let (to_ears, from_mic) = match on {
            true => {
                let (producer, consumer) = rtrb::RingBuffer::new(LOOPBACK_CAPACITY);
                (Some(producer), Some(consumer))
            }
            false => (None, None),
        };
        let _ = output.commands.send(OutputCommand::Loopback(from_mic));
        self.microphone.tell(InputCommand::Loopback(to_ears));
        self.looped = on;
    }

    fn open_output(&mut self) -> Result<Output, EngineError> {
        let wake = Arc::new(Wake::default());
        let id = self.next_stream();
        let playback = self
            .backend
            .open_playback(
                self.output_device.as_deref(),
                wake.clone(),
                self.alarm(Side::Speakers, id),
            )
            .map_err(EngineError::Speakers)?;
        let (commands, orders) = mpsc::channel();
        let parts = OutputParts {
            playing: playback.samples,
            shared: playback.shared.clone(),
            sample_rate: playback.sample_rate,
            resampler: FromWire::new(playback.sample_rate)?,
            controls: self.controls.clone(),
            telemetry: self.telemetry.clone(),
            received: self.received.clone(),
            orders,
            alarm: self.alarm(Side::Speakers, id),
        };
        let worker = Worker::spawn("ww-speakers", &wake, move |stop| output::run(parts, &stop))?;
        playback.stream.play().map_err(EngineError::Speakers)?;
        tracing::info!(rate = playback.sample_rate, "speakers open");
        Ok(Output {
            id,
            stream: playback.stream,
            shared: playback.shared,
            commands,
            worker,
        })
    }

    fn open_input(&mut self) -> Result<Input, EngineError> {
        let id = self.next_stream();
        let capture = self
            .backend
            .open_capture(
                self.input_device.as_deref(),
                self.microphone.wake.clone(),
                self.alarm(Side::Microphone, id),
            )
            .map_err(EngineError::Microphone)?;
        self.microphone.tell(InputCommand::Capture(Some(Capture {
            samples: capture.samples,
            resampler: ToWire::new(capture.sample_rate)?,
            alarm: self.alarm(Side::Microphone, id),
        })));
        if let Err(e) = capture.stream.play() {
            self.microphone.tell(InputCommand::Capture(None));
            return Err(EngineError::Microphone(e));
        }
        tracing::info!(rate = capture.sample_rate, "microphone open");
        Ok(Input {
            id,
            stream: capture.stream,
            shared: capture.shared,
        })
    }

    fn close_input(&mut self) {
        if let Some(Input { stream, shared, .. }) = self.input.take() {
            drop(stream);
            self.microphone.tell(InputCommand::Capture(None));
            self.retired.microphone_samples += shared.lost();
        }
    }

    fn close_output(&mut self) {
        if let Some(Output {
            stream,
            shared,
            worker,
            ..
        }) = self.output.take()
        {
            drop(stream);
            drop(worker);
            self.retired.speaker_underruns += shared.lost();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.close_input();
        self.close_output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::FakeBackend;

    #[test]
    fn a_new_default_moves_only_the_side_left_to_windows() {
        let backend = FakeBackend::quiet(48_000, None);
        let controls = Arc::new(Controls::new(DenoiseMode::Off, 0.0, 0.0, false));
        let mut engine =
            Engine::new(Arc::new(backend.clone()), controls, &Waker::default()).unwrap();
        engine.use_devices(Some("Fake microphone".into()), None);
        let (outgoing, _sent) = tokio::sync::mpsc::channel(8);
        engine.enter_room(outgoing);
        let opened = |count: &std::sync::atomic::AtomicUsize| count.load(Ordering::Relaxed);
        assert_eq!(
            (
                opened(&backend.microphones_opened),
                opened(&backend.speakers_opened)
            ),
            (1, 1)
        );

        backend.move_default_devices();
        engine.tick(Instant::now());
        assert_eq!(opened(&backend.microphones_opened), 1, "chosen by name");
        assert_eq!(opened(&backend.speakers_opened), 2, "left to Windows");
        assert_eq!(engine.troubles().count(), 0);
    }

    #[test]
    fn a_missing_device_is_looked_for_when_windows_tells_of_a_change() {
        let now = Instant::now();
        let mut health = Health::default();
        health.failed(now, AudioTrouble::Missing(Side::Microphone), true);
        assert!(!health.may_open(now + RETRY * 10));
        assert_eq!(health.next_try(), None);
        health.devices_changed();
        assert!(health.may_open(now));
    }

    #[test]
    fn a_broken_device_is_tried_again_after_a_while() {
        let now = Instant::now();
        let mut health = Health::default();
        health.failed(now, AudioTrouble::Broken(Side::Speakers), true);
        assert!(!health.may_open(now));
        assert!(health.may_open(now + RETRY));
    }

    #[test]
    fn without_news_of_devices_a_missing_one_is_tried_again_after_a_while() {
        let now = Instant::now();
        let mut health = Health::default();
        health.failed(
            now,
            AudioTrouble::Unplugged(Side::Speakers, "Yeti".into()),
            false,
        );
        assert_eq!(health.next_try(), Some(now + RETRY));
    }

    #[test]
    fn news_of_devices_leaves_a_working_side_alone() {
        let mut health = Health::default();
        health.devices_changed();
        assert_eq!(health.wait, Wait::Nothing);
        health.failed(Instant::now(), AudioTrouble::Broken(Side::Speakers), true);
        assert!(health.works());
        assert!(!health.works(), "it was said once");
    }
}
