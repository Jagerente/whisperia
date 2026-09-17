use std::{
    collections::VecDeque,
    sync::mpsc::{Receiver, Sender, TryRecvError},
};

use wasapi::{
    AudioCaptureClient, AudioClient, Direction, Handle, SampleType, StreamMode, WaveFormat,
};

use super::SoundOut;
use crate::com::Com;

const APP_SOUND_RATE: usize = 48_000;
const APP_SOUND_CHANNELS: usize = 2;
const STOP_NOTICED_WITHIN_MS: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundTarget {
    Process(u32),
    EverythingBut(u32),
}

#[derive(Debug, thiserror::Error)]
pub enum SoundError {
    #[error("cannot record the sound of what is shared: {0}")]
    Wasapi(#[from] wasapi::WasapiError),
    #[error("cannot start a thread: {0}")]
    Thread(#[from] std::io::Error),
    #[error("the sound thread stopped while starting")]
    Stillborn,
}

pub struct AppSound {
    _stop: Sender<()>,
}

impl AppSound {
    pub fn start(target: SoundTarget, mut on_samples: SoundOut) -> Result<Self, SoundError> {
        let (stop, stopped) = std::sync::mpsc::channel();
        let (started_tx, started) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("ww-share-sound".into())
            .spawn(move || {
                let _com = Com::enter();
                let recording = match Recording::open(target) {
                    Ok(recording) => recording,
                    Err(e) => {
                        let _ = started_tx.send(Err(e));
                        return;
                    }
                };
                let _ = started_tx.send(Ok(()));
                if let Err(e) = recording.run(&stopped, &mut *on_samples) {
                    tracing::warn!("the sound of what is shared stopped: {e}");
                }
            })?;
        started.recv().map_err(|_| SoundError::Stillborn)??;
        Ok(Self { _stop: stop })
    }
}

struct Recording {
    client: AudioClient,
    capture: AudioCaptureClient,
    event: Handle,
}

impl Recording {
    fn open(target: SoundTarget) -> Result<Self, SoundError> {
        let (process, include_tree) = match target {
            SoundTarget::Process(process) => (process, true),
            SoundTarget::EverythingBut(process) => (process, false),
        };
        let format = WaveFormat::new(
            32,
            32,
            &SampleType::Float,
            APP_SOUND_RATE,
            APP_SOUND_CHANNELS,
            None,
        );
        let mut client = AudioClient::new_application_loopback_client(process, include_tree)?;
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        };
        client.initialize_client(&format, &Direction::Capture, &mode)?;
        let event = client.set_get_eventhandle()?;
        let capture = client.get_audiocaptureclient()?;
        client.start_stream()?;
        Ok(Self {
            client,
            capture,
            event,
        })
    }

    fn run(
        &self,
        stopped: &Receiver<()>,
        on_samples: &mut dyn FnMut(&[f32]),
    ) -> Result<(), SoundError> {
        let mut bytes: VecDeque<u8> = VecDeque::new();
        let mut samples: Vec<f32> = Vec::new();
        let going = || matches!(stopped.try_recv(), Err(TryRecvError::Empty));
        while going() {
            let _ = self.event.wait_for_event(STOP_NOTICED_WITHIN_MS);
            while self.capture.get_next_packet_size()?.unwrap_or(0) > 0 {
                bytes.clear();
                let info = self.capture.read_from_device_to_deque(&mut bytes)?;
                samples.clear();
                if info.flags.silent {
                    samples.resize(bytes.len() / 4, 0.0);
                } else {
                    samples.extend(
                        bytes
                            .make_contiguous()
                            .chunks_exact(4)
                            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    );
                }
                if !going() {
                    break;
                }
                on_samples(&samples);
            }
        }
        let _ = self.client.stop_stream();
        Ok(())
    }
}
