use std::{
    sync::{
        atomic::Ordering,
        mpsc::{Receiver, TryRecvError},
        Arc, Mutex,
    },
    time::Duration,
};

use whisperia_proto::{
    audio::{AudioKind, AudioPacket, HEADER_LEN, MAX_PAYLOAD},
    FRAME_SAMPLES,
};

use super::{Controls, Mode, Outgoing, Telemetry};
use crate::{
    audio::{Alarm, Failure},
    codec::VoiceEncoder,
    dsp::{mic::MicChain, resample::ToWire},
};

const IDLE_WAKE: Duration = Duration::from_millis(20);
const MAX_BACKLOG_FRAMES: usize = 6;

pub(super) struct Capture {
    pub samples: rtrb::Consumer<f32>,
    pub resampler: ToWire,
    pub alarm: Alarm,
}

pub(super) enum InputCommand {
    Capture(Option<Capture>),
    Loopback(Option<rtrb::Producer<f32>>),
}

pub(super) struct InputParts {
    pub encoder: VoiceEncoder,
    pub controls: Arc<Controls>,
    pub telemetry: Arc<Telemetry>,
    pub outgoing: Arc<Mutex<Option<Outgoing>>>,
    pub orders: Receiver<InputCommand>,
}

struct Worker {
    parts: InputParts,
    chain: MicChain,
    capture: Option<Capture>,
    loopback: Option<rtrb::Producer<f32>>,
    device_samples: Vec<f32>,
    packet: [u8; MAX_PAYLOAD],
    seq: u32,
    encode_failed: bool,
}

pub(super) fn run(parts: InputParts) {
    let mut worker = Worker {
        parts,
        chain: MicChain::new(),
        capture: None,
        loopback: None,
        device_samples: Vec::new(),
        packet: [0u8; MAX_PAYLOAD],
        seq: 0,
        encode_failed: false,
    };
    loop {
        if worker.capture.is_none() {
            match worker.parts.orders.recv() {
                Ok(order) => worker.obey(order),
                Err(_) => return,
            }
            continue;
        }
        loop {
            match worker.parts.orders.try_recv() {
                Ok(order) => worker.obey(order),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        worker.drain();
        std::thread::park_timeout(IDLE_WAKE);
    }
}

impl Worker {
    fn obey(&mut self, order: InputCommand) {
        match order {
            InputCommand::Capture(capture) => {
                if capture.is_none() {
                    self.parts
                        .telemetry
                        .mic_db
                        .set(crate::dsp::level::SILENCE_DB);
                    self.parts
                        .telemetry
                        .mic_talking
                        .store(false, Ordering::Relaxed);
                }
                if let Some(capture) = &capture {
                    self.device_samples
                        .resize(capture.resampler.input_max(), 0.0);
                }
                self.capture = capture;
            }
            InputCommand::Loopback(to_ears) => self.loopback = to_ears,
        }
    }

    fn drain(&mut self) {
        let mut frame = [0f32; FRAME_SAMPLES];
        while let Some(capture) = &mut self.capture {
            let needed = capture.resampler.input_needed();
            let available = capture.samples.slots();
            if available < needed {
                return;
            }
            if available > needed * MAX_BACKLOG_FRAMES {
                discard(&mut capture.samples, available - needed);
            }
            let input = &mut self.device_samples[..needed];
            let Ok(chunk) = capture.samples.read_chunk(needed) else {
                return;
            };
            let (head, tail) = chunk.as_slices();
            input[..head.len()].copy_from_slice(head);
            input[head.len()..].copy_from_slice(tail);
            chunk.commit_all();
            if let Err(e) = capture.resampler.process(input, &mut frame) {
                tracing::error!("microphone resampler: {e}");
                (capture.alarm)(Failure::Broken);
                self.capture = None;
                return;
            }
            self.handle(&mut frame);
        }
    }

    fn handle(&mut self, frame: &mut [f32; FRAME_SAMPLES]) {
        let p = &self.parts;
        let mode = p.controls.mode();
        let muted = mode == Mode::Room && p.controls.muted.load(Ordering::Relaxed);
        self.chain.process(frame, p.controls.mic_settings(), muted);
        if !muted {
            p.telemetry.agc_gain_db.set(self.chain.agc_gain_db());
        }
        p.telemetry.mic_db.set(self.chain.meter().db());
        p.telemetry
            .mic_talking
            .store(self.chain.meter().talking() && !muted, Ordering::Relaxed);
        match mode {
            Mode::Idle => {}
            Mode::MicTest => {
                if let Some(chunk) = self
                    .loopback
                    .as_mut()
                    .and_then(|to_ears| to_ears.write_chunk_uninit(FRAME_SAMPLES).ok())
                {
                    chunk.fill_from_iter(frame.iter().copied());
                }
            }
            Mode::Room => {
                let seq = self.seq;
                self.seq = seq.wrapping_add(1);
                if muted {
                    return;
                }
                let len = match self.parts.encoder.encode(frame, &mut self.packet) {
                    Ok(len) => len,
                    Err(e) => {
                        if !self.encode_failed {
                            tracing::error!("encode: {e}");
                            self.encode_failed = true;
                        }
                        return;
                    }
                };
                let mut datagram = Vec::with_capacity(HEADER_LEN + len);
                AudioPacket {
                    kind: AudioKind::Voice,
                    sender: 0,
                    seq,
                    payload: &self.packet[..len],
                }
                .write_to(&mut datagram);
                let sent = self
                    .parts
                    .outgoing
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                    .is_some_and(|outgoing| outgoing.try_send(datagram.into()).is_ok());
                if sent {
                    self.parts
                        .telemetry
                        .frames_sent
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

fn discard(ring: &mut rtrb::Consumer<f32>, at_most: usize) {
    let n = ring.slots().min(at_most);
    if let Ok(chunk) = ring.read_chunk(n) {
        chunk.commit_all();
    }
}
