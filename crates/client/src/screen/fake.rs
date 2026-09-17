use std::hash::{Hash, Hasher};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use bytes::Bytes;
use whisperia_proto::PeerId;

use super::{
    policy, ListedSource, Preview, ScreenBackend, ScreenError, ShareEnd, ShareEnded, ShareHandle,
    ShareOut, SourceHeard, SourceKey, SourceKind, SourceNews, ViewerEvent, ViewerEvents,
    ViewerHandle,
};
use crate::{config::ShareConfig, net::OutgoingPicture};

pub const KEYFRAME: u8 = b'K';
pub const DELTA: u8 = b'P';
const PICTURE_BYTES: usize = 3000;
const KEYFRAME_BYTES: usize = 60_000;
const PICTURE_INTERVAL: Duration = Duration::from_millis(20);
const PICTURE_SIZE: (u32, u32) = (1920, 1080);
const LARGEST_STREAM: (u32, u32) = (4096, 2304);

pub type Shown = (bool, u64, usize);
pub const ICON_COLOUR: [u8; 3] = [250, 200, 0];

pub struct FakeScreen {
    pub listed: Mutex<Vec<(SourceKind, String, String)>>,
    pub painted: Mutex<Vec<(String, [u8; 3])>>,
    pub minimized: Mutex<Vec<String>>,
    pub cannot_view: AtomicBool,
    endings: Mutex<Vec<ShareEnded>>,
    news: Mutex<Vec<SourceHeard>>,
    viewers: Mutex<Vec<Arc<FakeViewer>>>,
    shares_started: AtomicU64,
    pictures_sent: Arc<AtomicU64>,
    shown: Mutex<Vec<String>>,
    settings: Arc<Mutex<Vec<ShareConfig>>>,
}

impl FakeScreen {
    pub fn new() -> Self {
        Self {
            listed: Mutex::new(
                [
                    (SourceKind::Screen, "Screen 1", "2560 x 1440"),
                    (SourceKind::Screen, "Screen 2", "1920 x 1080"),
                    (SourceKind::Window, "GitHub - File Explorer", "explorer"),
                    (
                        SourceKind::Window,
                        "Containers - Docker Desktop",
                        "Docker Desktop",
                    ),
                    (SourceKind::Window, "whisperia - Visual Studio Code", "Code"),
                ]
                .map(|(kind, label, detail)| (kind, label.to_owned(), detail.to_owned()))
                .into(),
            ),
            painted: Mutex::new(Vec::new()),
            minimized: Mutex::new(Vec::new()),
            cannot_view: AtomicBool::new(false),
            endings: Mutex::new(Vec::new()),
            news: Mutex::new(Vec::new()),
            viewers: Mutex::new(Vec::new()),
            shares_started: AtomicU64::new(0),
            pictures_sent: Arc::default(),
            shown: Mutex::new(Vec::new()),
            settings: Arc::default(),
        }
    }

    pub fn labels_shown(&self) -> Vec<String> {
        self.shown.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn settings_seen(&self) -> Vec<ShareConfig> {
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn viewers(&self) -> Vec<Arc<FakeViewer>> {
        self.viewers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn shares_started(&self) -> u64 {
        self.shares_started.load(Ordering::Relaxed)
    }

    pub fn pictures_sent(&self) -> u64 {
        self.pictures_sent.load(Ordering::Relaxed)
    }

    pub fn close_what_is_shared(&self) {
        let endings = std::mem::take(&mut *self.endings.lock().unwrap_or_else(|p| p.into_inner()));
        for ended in endings {
            ended(ShareEnd::SourceClosed);
        }
    }

    pub fn tell_what_is_shared(&self, news: SourceNews) {
        for heard in self
            .news
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter_mut()
        {
            heard(news);
        }
    }

    pub fn is_minimized(&self, label: &str) -> bool {
        self.minimized
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|minimized| minimized == label)
    }

    fn label_of(&self, key: SourceKey) -> String {
        self.listed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|(kind, label, _)| key_of(*kind, label) == key)
            .map_or_else(String::new, |(_, label, _)| label.clone())
    }
}

impl Default for FakeScreen {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FakeViewer {
    shown: Mutex<Vec<Shown>>,
    events: ViewerEvents,
}

impl FakeViewer {
    pub fn shown(&self) -> Vec<Shown> {
        self.shown.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl ViewerHandle for FakeViewer {
    fn push(&self, data: Bytes, keyframe: bool) {
        let number = data
            .get(1..9)
            .and_then(|b| b.try_into().ok())
            .map_or(0, u64::from_be_bytes);

        let mut shown = self.shown.lock().unwrap_or_else(|p| p.into_inner());
        if shown.is_empty() && !keyframe {
            drop(shown);
            let _ = self.events.send(ViewerEvent::NeedKeyframe);
            return;
        }
        shown.push((keyframe, number, data.len()));
    }
}

fn key_of(kind: SourceKind, label: &str) -> SourceKey {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    label.hash(&mut hasher);
    SourceKey {
        kind,
        handle: hasher.finish(),
        process: 0,
    }
}

fn solid(width: u32, height: u32, colour: [u8; 3]) -> Preview {
    let pixel = [colour[0], colour[1], colour[2], 255];
    Preview {
        width,
        height,
        rgba: pixel.repeat((width * height) as usize),
    }
}

fn stripes(index: usize) -> Preview {
    const COLOURS: [[u8; 3]; 5] = [
        [52, 96, 168],
        [148, 84, 160],
        [60, 134, 120],
        [176, 120, 60],
        [120, 60, 70],
    ];
    let (width, height) = (96u32, 54u32);
    let colour = COLOURS[index % COLOURS.len()];
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let shade = if (x + y) / 12 % 2 == 0 { 1.0 } else { 0.8 };
            rgba.extend(colour.map(|c| (f32::from(c) * shade) as u8));
            rgba.push(255);
        }
    }
    Preview {
        width,
        height,
        rgba,
    }
}

struct FakeShare {
    stop: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    settings: Arc<Mutex<Vec<ShareConfig>>>,
    wanted: Mutex<policy::Wanted>,
    sound: bool,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ShareHandle for FakeShare {
    fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }

    fn reconfigure(&self, settings: &ShareConfig) {
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(settings.clone());
        let changed = self
            .wanted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .reconfigure(settings);
        if changed.is_some() {
            self.keyframe.store(true, Ordering::Relaxed);
        }
    }

    fn follow(&self, source: (u32, u32)) {
        let changed = self
            .wanted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .follow(source);
        if changed.is_some() {
            self.keyframe.store(true, Ordering::Relaxed);
        }
    }

    fn stream(&self) -> policy::Stream {
        self.wanted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stream()
    }

    fn has_sound(&self) -> bool {
        self.sound
    }

    fn details(&self) -> String {
        "fake encoder".into()
    }
}

impl Drop for FakeShare {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl ScreenBackend for FakeScreen {
    fn list(&self) -> Result<Vec<ListedSource>, ScreenError> {
        let listed = self.listed.lock().unwrap_or_else(|p| p.into_inner());
        Ok(listed
            .iter()
            .map(|(kind, label, detail)| ListedSource {
                key: key_of(*kind, label),
                label: label.clone(),
                detail: detail.clone(),
                minimized: self.is_minimized(label),
            })
            .collect())
    }

    fn preview(&self, key: SourceKey) -> Option<Preview> {
        if self.is_minimized(&self.label_of(key)) {
            return None;
        }
        let painted = self.painted.lock().unwrap_or_else(|p| p.into_inner());
        let colour = painted
            .iter()
            .find(|(label, _)| key_of(key.kind, label) == key)
            .map(|(_, colour)| *colour);
        Some(match colour {
            Some(colour) => solid(96, 54, colour),
            None => stripes(key.handle as usize),
        })
    }

    fn icon(&self, key: SourceKey) -> Option<Preview> {
        (key.kind == SourceKind::Window).then(|| solid(16, 16, ICON_COLOUR))
    }

    fn share(
        &self,
        source: SourceKey,
        settings: &ShareConfig,
        out: ShareOut,
    ) -> Result<Box<dyn ShareHandle>, ScreenError> {
        self.shares_started.fetch_add(1, Ordering::Relaxed);
        let label = self.label_of(source);
        self.minimized
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|minimized| *minimized != label);
        self.shown
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(label);
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(settings.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(true));
        let ShareOut {
            mut pictures,
            sound,
            ended,
            news,
        } = out;
        self.endings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(ended);
        self.news
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(news);
        let mut sound = sound;
        let has_sound = sound.is_some();
        let interval = PICTURE_INTERVAL;
        let flags = (stop.clone(), keyframe.clone(), self.pictures_sent.clone());
        let thread = std::thread::Builder::new()
            .name("fake-share".into())
            .spawn(move || {
                let (stop, keyframe, sent) = flags;
                let mut number = 0u64;
                let mut phase = 0f32;
                let started = std::time::Instant::now();
                let mut delivered = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    let key = keyframe.swap(false, Ordering::Relaxed);
                    let size = if key { KEYFRAME_BYTES } else { PICTURE_BYTES };
                    let mut data = vec![0u8; size];
                    data[0] = if key { KEYFRAME } else { DELTA };
                    data[1..9].copy_from_slice(&number.to_be_bytes());
                    number += 1;
                    sent.fetch_add(1, Ordering::Relaxed);
                    pictures(OutgoingPicture {
                        data,
                        keyframe: key,
                        width: PICTURE_SIZE.0 as u16,
                        height: PICTURE_SIZE.1 as u16,
                    });
                    if let Some(sound) = &mut sound {
                        let due = (started.elapsed().as_secs_f64() * 48_000.0) as usize;
                        let chunk: Vec<f32> = (delivered..due)
                            .flat_map(|_| {
                                phase = (phase + 660.0 * std::f32::consts::TAU / 48_000.0)
                                    % std::f32::consts::TAU;
                                let sample = phase.sin() * 0.3;
                                [sample, sample]
                            })
                            .collect();
                        delivered = delivered.max(due);
                        sound(&chunk);
                    }
                    std::thread::sleep(interval);
                }
            })?;
        Ok(Box::new(FakeShare {
            stop,
            keyframe,
            settings: self.settings.clone(),
            wanted: Mutex::new(policy::Wanted::new(PICTURE_SIZE, settings, LARGEST_STREAM)),
            sound: has_sound,
            thread: Some(thread),
        }))
    }

    fn view(&self, _: PeerId, events: ViewerEvents) -> Result<Arc<dyn ViewerHandle>, ScreenError> {
        if self.cannot_view.load(Ordering::Relaxed) {
            return Err(ScreenError::NeedsDirect3D12);
        }
        let viewer = Arc::new(FakeViewer {
            shown: Mutex::new(Vec::new()),
            events,
        });
        self.viewers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(viewer.clone());
        Ok(viewer)
    }
}
