use whisperia_proto::{audio::MAX_PAYLOAD, FRAME_SAMPLES, SAMPLE_RATE};

use crate::jitter::Playout;

const BITRATE: i32 = 40_000;
const EXPECTED_LOSS_PERCENT: i32 = 5;

#[derive(Debug, thiserror::Error)]
#[error("opus: {0}")]
pub struct CodecError(#[from] opus::Error);

pub struct VoiceEncoder {
    inner: opus::Encoder,
}

impl VoiceEncoder {
    pub fn new() -> Result<Self, CodecError> {
        let mut inner =
            opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)?;
        inner.set_bitrate(opus::Bitrate::Bits(BITRATE))?;
        inner.set_vbr(true)?;
        inner.set_complexity(9)?;
        inner.set_signal(opus::Signal::Voice)?;
        inner.set_inband_fec(true)?;
        inner.set_packet_loss_perc(EXPECTED_LOSS_PERCENT)?;

        inner.set_dtx(false)?;
        Ok(Self { inner })
    }

    pub fn encode(
        &mut self,
        pcm: &[f32],
        out: &mut [u8; MAX_PAYLOAD],
    ) -> Result<usize, CodecError> {
        debug_assert_eq!(pcm.len(), FRAME_SAMPLES);
        Ok(self.inner.encode_float(pcm, out)?)
    }
}

pub struct ShareSoundEncoder {
    inner: opus::Encoder,
}

impl ShareSoundEncoder {
    const BITRATE: i32 = 128_000;

    pub fn new() -> Result<Self, CodecError> {
        let mut inner = opus::Encoder::new(
            SAMPLE_RATE,
            opus::Channels::Stereo,
            opus::Application::Audio,
        )?;
        inner.set_bitrate(opus::Bitrate::Bits(Self::BITRATE))?;
        inner.set_vbr(true)?;
        inner.set_inband_fec(true)?;
        inner.set_packet_loss_perc(EXPECTED_LOSS_PERCENT)?;
        Ok(Self { inner })
    }

    pub fn encode(
        &mut self,
        pcm: &[f32],
        out: &mut [u8; MAX_PAYLOAD],
    ) -> Result<usize, CodecError> {
        debug_assert_eq!(pcm.len(), FRAME_SAMPLES * 2);
        Ok(self.inner.encode_float(pcm, out)?)
    }
}

pub struct VoiceDecoder {
    inner: opus::Decoder,
}

impl VoiceDecoder {
    pub fn new() -> Result<Self, CodecError> {
        Ok(Self {
            inner: opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono)?,
        })
    }

    pub fn decode(&mut self, what: Playout<'_>, out: &mut [f32]) -> Result<bool, CodecError> {
        debug_assert_eq!(out.len(), FRAME_SAMPLES);
        let decoded = match what {
            Playout::Silence => {
                out.fill(0.0);
                return Ok(false);
            }
            Playout::Packet(packet) => self.inner.decode_float(packet, out, false)?,
            Playout::Fec(next_packet) => self.inner.decode_float(next_packet, out, true)?,
            Playout::Conceal => self.inner.decode_float(&[], out, false)?,
        };

        out[decoded.min(FRAME_SAMPLES)..].fill(0.0);
        Ok(true)
    }
}

pub struct Limiter(opus::SoftClip);

impl Limiter {
    pub fn new() -> Self {
        Self(opus::SoftClip::new(opus::Channels::Mono))
    }

    pub fn apply(&mut self, frame: &mut [f32]) {
        self.0.apply(frame);
    }
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_bounds_the_mix() {
        let mut frame: Vec<f32> = (0..FRAME_SAMPLES)
            .map(|i| (i as f32 / 20.0).sin() * 2.5)
            .collect();
        Limiter::new().apply(&mut frame);
        assert!(frame.iter().all(|s| s.abs() <= 1.0));
    }

    fn tone(frame: usize) -> Vec<f32> {
        (0..FRAME_SAMPLES)
            .map(|i| {
                let t = (frame * FRAME_SAMPLES + i) as f32 / SAMPLE_RATE as f32;
                0.4 * (t * 440.0 * std::f32::consts::TAU).sin()
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn round_trip_with_loss_concealment_and_fec() {
        let mut enc = VoiceEncoder::new().unwrap();
        let mut dec = VoiceDecoder::new().unwrap();
        let mut packets = Vec::new();
        for f in 0..50 {
            let mut buf = [0u8; MAX_PAYLOAD];
            let n = enc.encode(&tone(f), &mut buf).unwrap();
            assert!(
                n > 0 && n < 200,
                "10 ms at 40 kbit/s is about 50 bytes, got {n}"
            );
            packets.push(buf[..n].to_vec());
        }

        let mut out = vec![0f32; FRAME_SAMPLES];
        for (f, p) in packets.iter().enumerate() {
            let what = match f {
                30 => Playout::Conceal,
                40 => Playout::Fec(&packets[41]),
                _ => Playout::Packet(p),
            };
            assert!(dec.decode(what, &mut out).unwrap());
            if f > 5 {
                assert!(
                    rms(&out) > 0.1,
                    "frame {f} is unexpectedly quiet: {}",
                    rms(&out)
                );
            }
        }
        assert!(!dec.decode(Playout::Silence, &mut out).unwrap());
        assert_eq!(rms(&out), 0.0);
    }

    #[test]
    fn the_sound_of_a_share_is_sent_in_stereo_and_heard_in_mono() {
        let mut enc = ShareSoundEncoder::new().unwrap();
        let mut dec = VoiceDecoder::new().unwrap();
        let mut out = vec![0f32; FRAME_SAMPLES];
        for f in 0..30 {
            let stereo: Vec<f32> = tone(f).iter().flat_map(|s| [*s, 0.0]).collect();
            let mut buf = [0u8; MAX_PAYLOAD];
            let n = enc.encode(&stereo, &mut buf).unwrap();
            assert!(n > 0 && n <= MAX_PAYLOAD, "fits a datagram: {n}");
            assert!(dec.decode(Playout::Packet(&buf[..n]), &mut out).unwrap());
            if f > 5 {
                assert!(rms(&out) > 0.05, "frame {f}: {}", rms(&out));
            }
        }
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        let mut dec = VoiceDecoder::new().unwrap();
        let mut out = vec![0f32; FRAME_SAMPLES];
        let _ = dec.decode(Playout::Packet(&[0xFF; 300]), &mut out);
        let _ = dec.decode(Playout::Packet(&[0x00]), &mut out);
    }
}
