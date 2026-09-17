#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod policy;
#[cfg(windows)]
mod sound;
#[cfg(windows)]
mod windows;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use whisperia_proto::{
    audio::{AudioKind, AudioPacket, HEADER_LEN, MAX_PAYLOAD},
    PeerId, FRAME_SAMPLES,
};

use crate::{
    codec::{CodecError, ShareSoundEncoder},
    config::ShareConfig,
    net::OutgoingPicture,
    wake::WakingSender,
};

#[derive(Debug, thiserror::Error)]
pub enum ScreenError {
    #[error("showing the screen is not available on this system")]
    Unsupported,
    #[error("this PC has no Direct3D 12")]
    NeedsDirect3D12,
    #[error("cannot start a thread: {0}")]
    Thread(#[from] std::io::Error),
    #[error("the window cannot show the picture: {0}")]
    NotShown(#[from] slint::wgpu_30::TextureImportError),
    #[cfg(windows)]
    #[error("{0}")]
    Media(#[from] whisperia_winmedia::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ShareEnd {
    #[error("what was shown is gone")]
    SourceClosed,
    #[error("{0}")]
    Failed(#[from] ScreenError),
}

pub struct PickedSource {
    pub label: String,
    pub key: SourceKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Screen,
    Window,
}

pub struct Preview {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceKey {
    pub kind: SourceKind,
    pub handle: u64,
    pub process: u32,
}

pub struct ListedSource {
    pub key: SourceKey,
    pub label: String,
    pub detail: String,
    pub minimized: bool,
}

impl ListedSource {
    pub(crate) fn into_picked(self) -> PickedSource {
        PickedSource {
            label: self.label,
            key: self.key,
        }
    }
}

pub type PictureOut = Box<dyn FnMut(OutgoingPicture) + Send>;
pub type SoundOut = Box<dyn FnMut(&[f32]) + Send>;
pub type ShareEnded = Box<dyn FnOnce(ShareEnd) + Send>;
pub type SourceHeard = Box<dyn FnMut(SourceNews) + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceNews {
    Resized((u32, u32)),
    Minimized,
    Restored,
}

pub struct ShareOut {
    pub pictures: PictureOut,
    pub sound: Option<SoundOut>,
    pub ended: ShareEnded,
    pub news: SourceHeard,
}

#[derive(Debug)]
pub enum ViewerEvent {
    NeedKeyframe,
    Failed(ScreenError),
}

pub type ViewerEvents = WakingSender<ViewerEvent>;

pub trait ShareHandle: Send {
    fn request_keyframe(&self);
    fn reconfigure(&self, settings: &ShareConfig);
    fn follow(&self, source: (u32, u32));
    fn stream(&self) -> policy::Stream;
    fn has_sound(&self) -> bool;
    fn details(&self) -> String;
}

pub trait ViewerHandle: Send + Sync {
    fn push(&self, data: Bytes, keyframe: bool);
}

pub trait ScreenBackend: Send + Sync {
    fn list(&self) -> Result<Vec<ListedSource>, ScreenError>;

    fn preview(&self, key: SourceKey) -> Option<Preview>;

    fn icon(&self, key: SourceKey) -> Option<Preview>;

    fn share(
        &self,
        source: SourceKey,
        settings: &ShareConfig,
        out: ShareOut,
    ) -> Result<Box<dyn ShareHandle>, ScreenError>;

    fn view(
        &self,
        sharer: PeerId,
        events: ViewerEvents,
    ) -> Result<Arc<dyn ViewerHandle>, ScreenError>;
}

pub fn default_backend() -> Arc<dyn ScreenBackend> {
    #[cfg(windows)]
    {
        Arc::new(windows::WinScreen)
    }
    #[cfg(not(windows))]
    {
        Arc::new(Unsupported)
    }
}

pub struct Unsupported;

impl ScreenBackend for Unsupported {
    fn list(&self) -> Result<Vec<ListedSource>, ScreenError> {
        Err(ScreenError::Unsupported)
    }

    fn preview(&self, _: SourceKey) -> Option<Preview> {
        None
    }

    fn icon(&self, _: SourceKey) -> Option<Preview> {
        None
    }

    fn share(
        &self,
        _: SourceKey,
        _: &ShareConfig,
        _: ShareOut,
    ) -> Result<Box<dyn ShareHandle>, ScreenError> {
        Err(ScreenError::Unsupported)
    }

    fn view(&self, _: PeerId, _: ViewerEvents) -> Result<Arc<dyn ViewerHandle>, ScreenError> {
        Err(ScreenError::Unsupported)
    }
}

#[derive(Debug, Default)]
pub struct PictureGate {
    last: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Show,
    Skip,
}

impl PictureGate {
    pub fn admit(&mut self, seq: u32, keyframe: bool) -> Verdict {
        let follows = self.last.is_some_and(|last| seq == last.wrapping_add(1));
        if keyframe || follows {
            self.last = Some(seq);
            Verdict::Show
        } else {
            self.last = None;
            Verdict::Skip
        }
    }
}

#[derive(Debug)]
pub struct KeyframeAsks {
    every: Duration,
    last: Option<Instant>,
    wanted: bool,
}

impl KeyframeAsks {
    pub fn new(every: Duration) -> Self {
        Self {
            every,
            last: None,
            wanted: false,
        }
    }

    pub fn want(&mut self) {
        self.wanted = true;
    }

    pub fn due(&mut self, now: Instant) -> bool {
        let spaced = self
            .last
            .is_none_or(|last| now.duration_since(last) >= self.every);
        if !(self.wanted && spaced) {
            return false;
        }
        self.wanted = false;
        self.last = Some(now);
        true
    }
}

pub struct ShareSound {
    encoder: ShareSoundEncoder,
    pending: Vec<f32>,
    seq: u32,
    outgoing: tokio::sync::mpsc::Sender<Bytes>,
}

impl ShareSound {
    const FRAME: usize = FRAME_SAMPLES * 2;

    pub fn new(outgoing: tokio::sync::mpsc::Sender<Bytes>) -> Result<Self, CodecError> {
        Ok(Self {
            encoder: ShareSoundEncoder::new()?,
            pending: Vec::with_capacity(Self::FRAME * 8),
            seq: 0,
            outgoing,
        })
    }

    pub fn push(&mut self, samples: &[f32]) {
        self.pending.extend_from_slice(samples);
        let mut packet = [0u8; MAX_PAYLOAD];
        let mut used = 0;
        while self.pending.len() - used >= Self::FRAME {
            let frame = &self.pending[used..used + Self::FRAME];
            used += Self::FRAME;
            let seq = self.seq;
            self.seq = self.seq.wrapping_add(1);
            let len = match self.encoder.encode(frame, &mut packet) {
                Ok(len) => len,
                Err(e) => {
                    tracing::debug!("share sound: {e}");
                    continue;
                }
            };
            let mut datagram = Vec::with_capacity(HEADER_LEN + len);
            AudioPacket {
                kind: AudioKind::Share,
                sender: 0,
                seq,
                payload: &packet[..len],
            }
            .write_to(&mut datagram);

            let _ = self.outgoing.try_send(datagram.into());
        }
        self.pending.drain(..used);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_shown_between_a_gap_and_the_next_keyframe() {
        let mut gate = PictureGate::default();

        assert_eq!(gate.admit(7, false), Verdict::Skip);
        assert_eq!(gate.admit(8, false), Verdict::Skip);
        assert_eq!(gate.admit(9, true), Verdict::Show);
        assert_eq!(gate.admit(10, false), Verdict::Show);

        assert_eq!(gate.admit(12, false), Verdict::Skip);
        assert_eq!(gate.admit(13, false), Verdict::Skip);
        assert_eq!(gate.admit(14, true), Verdict::Show);
        assert_eq!(gate.admit(15, false), Verdict::Show);

        let mut gate = PictureGate::default();
        assert_eq!(gate.admit(u32::MAX, true), Verdict::Show);
        assert_eq!(gate.admit(0, false), Verdict::Show);
    }

    #[test]
    fn a_keyframe_is_asked_for_once_per_want_and_spaced_out() {
        let mut asks = KeyframeAsks::new(Duration::from_millis(500));
        let start = Instant::now();
        assert!(!asks.due(start), "nobody wanted one");
        asks.want();
        asks.want();
        assert!(asks.due(start));
        assert!(!asks.due(start), "two wants are one ask");
        asks.want();
        assert!(!asks.due(start + Duration::from_millis(100)));
        assert!(
            asks.due(start + Duration::from_millis(600)),
            "a want is kept until it may be asked"
        );
        assert!(!asks.due(start + Duration::from_millis(1200)));
    }

    #[test]
    fn sound_becomes_one_datagram_per_ten_milliseconds() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let mut sound = ShareSound::new(tx).unwrap();
        let tone: Vec<f32> = (0..FRAME_SAMPLES * 2 * 3 + 100)
            .map(|i| (i as f32 / 30.0).sin() * 0.3)
            .collect();

        for chunk in tone.chunks(777) {
            sound.push(chunk);
        }
        let mut seqs = Vec::new();
        while let Ok(datagram) = rx.try_recv() {
            let packet = AudioPacket::parse(&datagram).unwrap();
            assert_eq!(packet.kind, AudioKind::Share);
            seqs.push(packet.seq);
        }
        assert_eq!(seqs, [0, 1, 2], "three whole frames, the rest waits");
        sound.push(&vec![0.0; FRAME_SAMPLES * 2 - 100]);
        assert_eq!(AudioPacket::parse(&rx.try_recv().unwrap()).unwrap().seq, 3);
    }
}
