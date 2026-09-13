use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use whisperia_proto::{
    audio::{AudioKind, AudioPacket},
    PeerId, FRAME_MS, FRAME_SAMPLES,
};

use super::{Controls, Mode, PeerTelemetry, Received, Telemetry};
use crate::{
    audio::{Alarm, Failure, StreamShared},
    codec::{Limiter, VoiceDecoder},
    dsp::{cue::CuePlayer, level::Meter, resample::FromWire, volume_to_gain},
    jitter::JitterBuffer,
};

const IDLE_WAKE: Duration = Duration::from_millis(20);
const MARGIN_HOLD: Duration = Duration::from_secs(30);
const TELEMETRY_EVERY_FRAMES: u32 = 5;
const LOOPBACK_PRIMED_FRAMES: usize = 2;
const LOOPBACK_MOST_FRAMES: usize = 6;

pub(super) enum OutputCommand {
    Forget(PeerId),
    ForgetShare(PeerId),
    Loopback(Option<rtrb::Consumer<f32>>),
}

pub(super) struct OutputParts {
    pub playing: rtrb::Producer<f32>,
    pub shared: Arc<StreamShared>,
    pub sample_rate: u32,
    pub resampler: FromWire,
    pub controls: Arc<Controls>,
    pub telemetry: Arc<Telemetry>,
    pub received: Arc<Mutex<Receiver<Received>>>,
    pub orders: Receiver<OutputCommand>,
    pub alarm: Alarm,
}

struct Talker {
    jitter: JitterBuffer,
    decoder: VoiceDecoder,
    meter: Meter,
    gain: f32,
}

#[derive(Default)]
struct Mixer {
    talkers: HashMap<(PeerId, AudioKind), Talker>,
    volumes_seen: Option<u32>,
    voice: Vec<f32>,
}

impl Mixer {
    fn insert(&mut self, datagram: &[u8], at: Instant, controls: &Controls) {
        let Ok(packet) = AudioPacket::parse(datagram) else {
            return;
        };
        let key = (packet.sender, packet.kind);
        let talker = match self.talkers.entry(key) {
            Entry::Occupied(known) => known.into_mut(),
            Entry::Vacant(new) => match VoiceDecoder::new() {
                Ok(decoder) => new.insert(Talker {
                    jitter: JitterBuffer::new(at),
                    decoder,
                    meter: Meter::default(),
                    gain: volume_to_gain(controls.volume_db(packet.sender, packet.kind)),
                }),
                Err(e) => return tracing::error!("cannot create a decoder: {e}"),
            },
        };
        talker.jitter.insert(packet.seq, packet.payload, at);
    }

    fn follow_volumes(&mut self, controls: &Controls) {
        let version = controls.volumes_version();
        if self.volumes_seen == Some(version) {
            return;
        }
        self.volumes_seen = Some(version);
        for ((id, kind), talker) in &mut self.talkers {
            talker.gain = volume_to_gain(controls.volume_db(*id, *kind));
        }
    }

    fn forget(&mut self, peer: PeerId) {
        self.talkers.retain(|(id, _), _| *id != peer);
    }

    fn forget_share(&mut self, peer: PeerId) {
        self.talkers.remove(&(peer, AudioKind::Share));
    }

    fn silence(&mut self) {
        self.talkers.values_mut().for_each(|t| t.jitter.reset());
    }

    fn mix(&mut self, mix: &mut [f32]) {
        self.voice.resize(mix.len(), 0.0);
        for talker in self.talkers.values_mut() {
            let what = talker.jitter.pop();
            match talker.decoder.decode(what, &mut self.voice) {
                Ok(true) => {
                    talker.meter.feed(&self.voice);
                    let gain = talker.gain;
                    mix.iter_mut()
                        .zip(&self.voice)
                        .for_each(|(m, v)| *m += v * gain);
                }
                Ok(false) => talker.meter.feed_silence(),
                Err(e) => {
                    tracing::debug!("decode: {e}");
                    talker.meter.feed_silence();
                }
            }
        }
    }

    fn report(&self, telemetry: &Telemetry) {
        for (kind, table) in [
            (AudioKind::Voice, &telemetry.peers),
            (AudioKind::Share, &telemetry.shares),
        ] {
            let mut table = table.lock().unwrap_or_else(|e| e.into_inner());
            table.clear();
            table.extend(self.talkers.iter().filter(|((_, of), _)| *of == kind).map(
                |((id, _), t)| PeerTelemetry {
                    id: *id,
                    level_db: t.meter.db(),
                    talking: t.meter.talking(),
                    jitter: t.jitter.stats(),
                    target_frames: t.jitter.target(),
                    depth_frames: t.jitter.depth(),
                },
            ));
        }
    }
}

struct FillPolicy {
    frame: usize,
    underruns_seen: u64,
    margin_until: Option<Instant>,
}

impl FillPolicy {
    fn new(sample_rate: u32) -> Self {
        Self {
            frame: sample_rate as usize * FRAME_MS as usize / 1000,
            underruns_seen: 0,
            margin_until: None,
        }
    }

    fn target(&mut self, now: Instant, period: u32, underruns: u64) -> usize {
        if underruns != self.underruns_seen {
            self.underruns_seen = underruns;
            self.margin_until = Some(now + MARGIN_HOLD);
        }
        if self.margin_until.is_some_and(|until| now > until) {
            self.margin_until = None;
        }
        let period = match period {
            0 => self.frame,
            frames => frames as usize,
        };
        period
            + if self.margin_until.is_some() {
                self.frame
            } else {
                0
            }
    }
}

struct Loopback {
    from_mic: rtrb::Consumer<f32>,
    primed: bool,
}

impl Loopback {
    fn new(from_mic: rtrb::Consumer<f32>) -> Self {
        Self {
            from_mic,
            primed: false,
        }
    }

    fn mix_into(&mut self, mix: &mut [f32], gain: f32) {
        let most = FRAME_SAMPLES * LOOPBACK_MOST_FRAMES;
        let kept = FRAME_SAMPLES * LOOPBACK_PRIMED_FRAMES;
        if self.from_mic.slots() >= most {
            if let Ok(chunk) = self.from_mic.read_chunk(self.from_mic.slots() - kept) {
                chunk.commit_all();
            }
        }
        let needed = if self.primed {
            1
        } else {
            LOOPBACK_PRIMED_FRAMES
        };
        self.primed = self.from_mic.slots() >= FRAME_SAMPLES * needed;
        if !self.primed {
            return;
        }
        if let Ok(chunk) = self.from_mic.read_chunk(FRAME_SAMPLES) {
            let (head, tail) = chunk.as_slices();
            mix.iter_mut()
                .zip(head.iter().chain(tail))
                .for_each(|(m, s)| *m += s * gain);
            chunk.commit_all();
        }
    }
}

pub(super) fn run(mut p: OutputParts, stop: &AtomicBool) {
    let capacity = p.playing.buffer().capacity();
    let mut mixer = Mixer::default();
    let mut fill = FillPolicy::new(p.sample_rate);
    let mut loopback: Option<Loopback> = None;
    let mut limiter = Limiter::new();
    let mut cues = CuePlayer::default();
    let mut mix = [0f32; FRAME_SAMPLES];
    let mut was_hearing = false;
    let mut was_in_room = false;
    let mut frames_since_report = 0u32;

    while !stop.load(Ordering::Relaxed) {
        for order in p.orders.try_iter() {
            match order {
                OutputCommand::Forget(peer) => mixer.forget(peer),
                OutputCommand::ForgetShare(peer) => mixer.forget_share(peer),
                OutputCommand::Loopback(from_mic) => loopback = from_mic.map(Loopback::new),
            }
        }

        let in_room = p.controls.mode() == Mode::Room;
        let hearing = in_room && !p.controls.deafened.load(Ordering::Relaxed);
        {
            let received = p.received.lock().unwrap_or_else(|e| e.into_inner());
            for (datagram, at) in received.try_iter() {
                if hearing {
                    mixer.insert(&datagram, at, &p.controls);
                }
            }
        }
        if was_in_room && !in_room {
            mixer = Mixer::default();
        } else if was_hearing && !hearing {
            mixer.silence();
        }
        (was_hearing, was_in_room) = (hearing, in_room);
        mixer.follow_volumes(&p.controls);
        p.controls.cues.take().for_each(|cue| cues.start(cue));

        let target = fill.target(Instant::now(), p.shared.period(), p.shared.lost());
        while capacity - p.playing.slots() < target {
            mix.fill(0.0);
            mixer.mix(&mut mix);
            let gain = volume_to_gain(p.controls.volume_db.get());
            mix.iter_mut().for_each(|s| *s *= gain);
            if let Some(loopback) = &mut loopback {
                loopback.mix_into(&mut mix, gain);
            }
            cues.mix_into(&mut mix);
            limiter.apply(&mut mix);

            match p.resampler.process(&mix) {
                Ok(out) => {
                    let n = out.len().min(p.playing.slots());
                    if let Ok(chunk) = p.playing.write_chunk_uninit(n) {
                        chunk.fill_from_iter(out.iter().copied());
                    }
                }
                Err(e) => {
                    tracing::error!("speaker resampler: {e}");
                    (p.alarm)(Failure::Broken);
                    return;
                }
            }
            frames_since_report += 1;
        }

        if frames_since_report >= TELEMETRY_EVERY_FRAMES {
            frames_since_report = 0;
            mixer.report(&p.telemetry);
        }
        std::thread::park_timeout(IDLE_WAKE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fill_follows_the_device_period() {
        let mut fill = FillPolicy::new(48_000);
        let t0 = Instant::now();
        assert_eq!(
            fill.target(t0, 0, 0),
            480,
            "one frame until the device says"
        );
        assert_eq!(fill.target(t0, 1056, 0), 1056);
    }

    #[test]
    fn an_underrun_adds_a_frame_of_margin_for_a_while() {
        let mut fill = FillPolicy::new(48_000);
        let t0 = Instant::now();
        assert_eq!(fill.target(t0, 480, 1), 960);
        assert_eq!(fill.target(t0 + MARGIN_HOLD / 2, 480, 1), 960);
        assert_eq!(fill.target(t0 + MARGIN_HOLD * 2, 480, 1), 480);
    }

    #[test]
    fn the_microphone_test_waits_for_two_frames_then_plays_each_as_it_comes() {
        let (mut to_ears, from_mic) = rtrb::RingBuffer::new(FRAME_SAMPLES * 8);
        let mut loopback = Loopback::new(from_mic);
        let mut mix = [0f32; FRAME_SAMPLES];
        let mut say = |frames: usize| {
            for _ in 0..frames * FRAME_SAMPLES {
                let _ = to_ears.push(0.5);
            }
        };
        say(1);
        loopback.mix_into(&mut mix, 1.0);
        assert_eq!(mix[0], 0.0, "not yet");
        say(1);
        loopback.mix_into(&mut mix, 1.0);
        assert_eq!(mix[0], 0.5);
        mix.fill(0.0);
        loopback.mix_into(&mut mix, 1.0);
        assert_eq!(mix[0], 0.5, "one frame is enough once primed");
    }

    #[test]
    fn the_microphone_test_never_falls_behind() {
        let (mut to_ears, from_mic) = rtrb::RingBuffer::new(FRAME_SAMPLES * 8);
        let mut loopback = Loopback::new(from_mic);
        for _ in 0..FRAME_SAMPLES * LOOPBACK_MOST_FRAMES {
            let _ = to_ears.push(0.5);
        }
        let mut mix = [0f32; FRAME_SAMPLES];
        loopback.mix_into(&mut mix, 1.0);
        assert_eq!(
            loopback.from_mic.slots(),
            FRAME_SAMPLES * (LOOPBACK_PRIMED_FRAMES - 1)
        );
    }

    #[test]
    fn someone_who_leaves_is_no_longer_mixed() {
        let controls = Controls::new(Default::default(), 0.0, 0.0, false);
        let mut mixer = Mixer::default();
        let mut encoder = crate::codec::VoiceEncoder::new().unwrap();
        let mut payload = [0u8; whisperia_proto::audio::MAX_PAYLOAD];
        let len = encoder.encode(&[0.1; FRAME_SAMPLES], &mut payload).unwrap();
        for (sender, kind) in [
            (3, AudioKind::Voice),
            (3, AudioKind::Share),
            (4, AudioKind::Voice),
        ] {
            let mut datagram = Vec::new();
            AudioPacket {
                kind,
                sender,
                seq: 0,
                payload: &payload[..len],
            }
            .write_to(&mut datagram);
            mixer.insert(&datagram, Instant::now(), &controls);
        }
        mixer.forget_share(3);
        assert_eq!(mixer.talkers.len(), 2);
        mixer.forget(3);
        assert_eq!(
            mixer.talkers.keys().collect::<Vec<_>>(),
            [&(4, AudioKind::Voice)]
        );
    }

    #[test]
    fn a_volume_change_reaches_the_talker() {
        let controls = Controls::new(Default::default(), 0.0, 0.0, false);
        let mut mixer = Mixer::default();
        let mut encoder = crate::codec::VoiceEncoder::new().unwrap();
        let mut payload = [0u8; whisperia_proto::audio::MAX_PAYLOAD];
        let len = encoder.encode(&[0.1; FRAME_SAMPLES], &mut payload).unwrap();
        let mut datagram = Vec::new();
        AudioPacket {
            kind: AudioKind::Voice,
            sender: 3,
            seq: 0,
            payload: &payload[..len],
        }
        .write_to(&mut datagram);
        mixer.insert(&datagram, Instant::now(), &controls);
        mixer.follow_volumes(&controls);
        controls.set_volumes(&[(3, -6.0)], &[]);
        mixer.follow_volumes(&controls);
        let gain = mixer.talkers[&(3, AudioKind::Voice)].gain;
        assert!((gain - 0.5).abs() < 0.01);
    }
}
