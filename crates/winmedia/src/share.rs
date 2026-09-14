use std::{
    sync::{
        mpsc::{Receiver, RecvTimeoutError, Sender},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crate::{
    capture::{Capture, CaptureEvent, CapturedFrame},
    convert::{fit, rect, Converter, Direction},
    d3d::Device,
    encode::{Encoder, EncoderEvent, Notify},
    mf::{self, ComThread},
    sources::Source,
    EncodedFrame, Error,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareSettings {
    size: (u32, u32),
    fps: u32,
    bitrate: u32,
}

impl ShareSettings {
    pub const LARGEST: (u32, u32) = (4096, 2304);

    pub fn new(size: (u32, u32), fps: u32, bitrate: u32) -> Result<Self, Error> {
        let even = size.0 % 2 == 0 && size.1 % 2 == 0;
        let fits =
            (1..=Self::LARGEST.0).contains(&size.0) && (1..=Self::LARGEST.1).contains(&size.1);
        match even && fits && fps > 0 && bitrate > 0 {
            true => Ok(Self { size, fps, bitrate }),
            false => Err(Error::Unencodable {
                width: size.0,
                height: size.1,
                fps,
                bitrate,
            }),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn fps(&self) -> u32 {
        self.fps
    }

    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    fn interval(&self) -> Duration {
        Duration::from_secs(1) / self.fps
    }
}

#[derive(Debug)]
pub enum EndReason {
    SourceClosed,
    Failed(Error),
}

enum Message {
    Stop,
    Keyframe,
    Reconfigure(ShareSettings),
    Capture(CaptureEvent),
    Encoder {
        generation: u64,
        event: EncoderEvent,
    },
}

pub struct Share {
    messages: Sender<Message>,
    thread: Option<JoinHandle<()>>,
    encoder: String,
}

impl Share {
    pub fn start(
        source: Source,
        settings: ShareSettings,
        on_frame: Box<dyn FnMut(EncodedFrame) + Send>,
        on_end: Box<dyn FnOnce(EndReason) + Send>,
    ) -> Result<Self, Error> {
        let (messages, inbox) = std::sync::mpsc::channel();
        let (started_tx, started) = std::sync::mpsc::channel();
        let to_worker = messages.clone();
        let thread = std::thread::Builder::new()
            .name("ww-share".into())
            .spawn(move || {
                let _com = ComThread::enter();
                let mut worker = match Worker::new(&source, settings, to_worker, on_frame) {
                    Ok(worker) => worker,
                    Err(e) => {
                        let _ = started_tx.send(Err(e));
                        return;
                    }
                };
                let _ = started_tx.send(Ok(worker.encoder.name().to_owned()));
                match worker.run(&inbox) {
                    None => tracing::info!("screen share stopped"),
                    Some(reason) => {
                        tracing::info!(?reason, "screen share ended");
                        on_end(reason);
                    }
                }
            })?;

        let encoder = started
            .recv()
            .map_err(|_| Error::Stillborn("screen share"))??;
        tracing::info!(%encoder, ?settings, "screen share started");
        Ok(Self {
            messages,
            thread: Some(thread),
            encoder,
        })
    }

    pub fn reconfigure(&self, settings: ShareSettings) {
        let _ = self.messages.send(Message::Reconfigure(settings));
    }

    pub fn request_keyframe(&self) {
        let _ = self.messages.send(Message::Keyframe);
    }

    pub fn encoder(&self) -> &str {
        &self.encoder
    }
}

impl Drop for Share {
    fn drop(&mut self) {
        let _ = self.messages.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Worker {
    device: Device,
    capture: Capture,
    settings: ShareSettings,
    encoder: Encoder,
    generation: u64,
    converter: Option<Converter>,
    held: Option<CapturedFrame>,
    fresh: bool,
    keyframe: bool,
    fed_at: Option<Instant>,
    started: Instant,
    messages: Sender<Message>,
    on_frame: Box<dyn FnMut(EncodedFrame) + Send>,
}

fn notify(messages: &Sender<Message>, generation: u64) -> Notify {
    let messages = messages.clone();
    Arc::new(move |event| {
        let _ = messages.send(Message::Encoder { generation, event });
    })
}

impl Worker {
    fn new(
        source: &Source,
        settings: ShareSettings,
        messages: Sender<Message>,
        on_frame: Box<dyn FnMut(EncodedFrame) + Send>,
    ) -> Result<Self, Error> {
        let device = Device::new()?;
        let encoder = Encoder::new(&device, &settings, notify(&messages, 0))?;
        let to_worker = messages.clone();
        let capture = Capture::start(&device, source, move |event| {
            let _ = to_worker.send(Message::Capture(event));
        })?;
        Ok(Self {
            device,
            capture,
            settings,
            encoder,
            generation: 0,
            converter: None,
            held: None,
            fresh: false,
            keyframe: true,
            fed_at: None,
            started: Instant::now(),
            messages,
            on_frame,
        })
    }

    fn run(&mut self, inbox: &Receiver<Message>) -> Option<EndReason> {
        loop {
            let message = match self.due() {
                Some(due) => {
                    match inbox.recv_timeout(due.saturating_duration_since(Instant::now())) {
                        Ok(message) => Some(message),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return None,
                    }
                }
                None => Some(inbox.recv().ok()?),
            };
            if let Some(message) = message {
                match self.handle(message) {
                    Ok(true) => {}
                    Ok(false) => return None,
                    Err(reason) => return Some(reason),
                }
            }
            if let Err(e) = self.encode() {
                return Some(EndReason::Failed(e));
            }
        }
    }

    fn handle(&mut self, message: Message) -> Result<bool, EndReason> {
        match message {
            Message::Stop => return Ok(false),
            Message::Keyframe => self.keyframe = true,
            Message::Reconfigure(settings) => {
                self.rebuild(settings).map_err(EndReason::Failed)?;
            }
            Message::Capture(CaptureEvent::Frame) => {
                if let Some(frame) = self.capture.latest().map_err(EndReason::Failed)? {
                    self.held = Some(frame);
                    self.fresh = true;
                }
            }
            Message::Capture(CaptureEvent::Closed) => return Err(EndReason::SourceClosed),
            Message::Encoder { generation, event } if generation == self.generation => {
                let output = self.encoder.handle(event).map_err(EndReason::Failed)?;
                if let Some(output) = output {
                    let (width, height) = self.settings.size();
                    (self.on_frame)(EncodedFrame {
                        data: output.data,
                        keyframe: output.keyframe,
                        width,
                        height,
                    });
                }
            }
            Message::Encoder { .. } => {}
        }
        Ok(true)
    }

    fn rebuild(&mut self, settings: ShareSettings) -> Result<(), Error> {
        if settings == self.settings {
            return Ok(());
        }
        self.encoder.stop();
        self.generation += 1;
        self.encoder = Encoder::new(
            &self.device,
            &settings,
            notify(&self.messages, self.generation),
        )?;
        self.settings = settings;
        self.converter = None;
        self.keyframe = true;
        self.fed_at = None;
        tracing::info!(?settings, "screen share reconfigured");
        Ok(())
    }

    fn wants_to_encode(&self) -> bool {
        (self.fresh || self.keyframe) && self.held.is_some() && self.encoder.ready_input().is_some()
    }

    fn due(&self) -> Option<Instant> {
        let due = self.fed_at? + self.settings.interval();
        (self.wants_to_encode() && due > Instant::now()).then_some(due)
    }

    fn encode(&mut self) -> Result<(), Error> {
        let now = Instant::now();
        if !self.wants_to_encode() || self.due().is_some() {
            return Ok(());
        }
        let (Some(frame), Some(index)) = (&self.held, self.encoder.ready_input()) else {
            return Ok(());
        };
        let size = self.settings.size();
        let converter = match self.converter.take() {
            Some(converter) if converter.input_size() == frame.texture_size => converter,
            _ => Converter::new(
                &self.device,
                frame.texture_size,
                size,
                Some(self.settings.fps()),
                Direction::RgbToYuv,
                self.encoder.inputs(),
            )?,
        };
        let converter = self.converter.insert(converter);
        converter.blit(
            &self.device,
            &frame.texture,
            0,
            rect(frame.content.0, frame.content.1),
            index,
            fit(frame.content, size),
        )?;
        self.encoder.feed(
            index,
            mf::hns(now.duration_since(self.started)),
            mf::hns(self.settings.interval()),
            self.keyframe,
        )?;
        self.fresh = false;
        self.keyframe = false;
        self.fed_at = Some(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_the_encoder_can_take_is_accepted() {
        let good = ShareSettings::new((1920, 1080), 60, 10_000_000).unwrap();
        assert_eq!(
            (good.size(), good.fps(), good.bitrate()),
            ((1920, 1080), 60, 10_000_000)
        );
        assert_eq!(good.interval(), Duration::from_secs(1) / 60);
        assert!(ShareSettings::new(ShareSettings::LARGEST, 1, 1).is_ok());
        for (size, fps, bitrate) in [
            ((1921, 1080), 60, 1),
            ((1920, 1081), 60, 1),
            ((0, 1080), 60, 1),
            ((4098, 2304), 60, 1),
            ((4096, 2306), 60, 1),
            ((1920, 1080), 0, 1),
            ((1920, 1080), 60, 0),
        ] {
            assert!(
                matches!(
                    ShareSettings::new(size, fps, bitrate),
                    Err(Error::Unencodable { .. })
                ),
                "{size:?} {fps} {bitrate}"
            );
        }
    }
}
