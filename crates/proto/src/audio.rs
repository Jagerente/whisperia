use crate::PeerId;

pub const KIND_AUDIO: u8 = 1;
pub const KIND_SHARE_AUDIO: u8 = 2;
pub const HEADER_LEN: usize = 7;
pub const MAX_PAYLOAD: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AudioKind {
    Voice,
    Share,
}

impl AudioKind {
    fn byte(self) -> u8 {
        match self {
            Self::Voice => KIND_AUDIO,
            Self::Share => KIND_SHARE_AUDIO,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AudioError {
    #[error("datagram too short ({0} bytes)")]
    TooShort(usize),
    #[error("unknown datagram kind {0}")]
    UnknownKind(u8),
    #[error("opus payload of {0} bytes is out of range")]
    BadPayload(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPacket<'a> {
    pub kind: AudioKind,
    pub sender: PeerId,
    pub seq: u32,
    pub payload: &'a [u8],
}

impl<'a> AudioPacket<'a> {
    pub fn parse(datagram: &'a [u8]) -> Result<Self, AudioError> {
        if datagram.len() < HEADER_LEN {
            return Err(AudioError::TooShort(datagram.len()));
        }
        let kind = match datagram[0] {
            KIND_AUDIO => AudioKind::Voice,
            KIND_SHARE_AUDIO => AudioKind::Share,
            other => return Err(AudioError::UnknownKind(other)),
        };
        let payload = &datagram[HEADER_LEN..];
        if payload.is_empty() || payload.len() > MAX_PAYLOAD {
            return Err(AudioError::BadPayload(payload.len()));
        }
        Ok(Self {
            kind,
            sender: u16::from_be_bytes([datagram[1], datagram[2]]),
            seq: u32::from_be_bytes([datagram[3], datagram[4], datagram[5], datagram[6]]),
            payload,
        })
    }

    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.reserve(HEADER_LEN + self.payload.len());
        out.push(self.kind.byte());
        out.extend_from_slice(&self.sender.to_be_bytes());
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(self.payload);
    }
}

pub fn stamp_sender(datagram: &mut [u8], sender: PeerId) -> Result<(), AudioError> {
    if datagram.len() < HEADER_LEN {
        return Err(AudioError::TooShort(datagram.len()));
    }
    datagram[1..3].copy_from_slice(&sender.to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for kind in [AudioKind::Voice, AudioKind::Share] {
            let p = AudioPacket {
                kind,
                sender: 7,
                seq: 0xDEAD_BEEF,
                payload: &[1, 2, 3],
            };
            let mut buf = Vec::new();
            p.write_to(&mut buf);
            assert_eq!(buf.len(), HEADER_LEN + 3);
            assert_eq!(AudioPacket::parse(&buf).unwrap(), p);
        }
    }

    #[test]
    fn stamping_replaces_sender_only() {
        let mut buf = Vec::new();
        AudioPacket {
            kind: AudioKind::Share,
            sender: 0,
            seq: 42,
            payload: &[9; 10],
        }
        .write_to(&mut buf);
        stamp_sender(&mut buf, 513).unwrap();
        let p = AudioPacket::parse(&buf).unwrap();
        assert_eq!(
            (p.kind, p.sender, p.seq, p.payload),
            (AudioKind::Share, 513, 42, &[9u8; 10][..])
        );
    }

    #[test]
    fn a_datagram_too_short_to_stamp_is_refused() {
        assert_eq!(stamp_sender(&mut [1, 0], 7), Err(AudioError::TooShort(2)));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(AudioPacket::parse(&[1, 0, 0]), Err(AudioError::TooShort(3)));
        assert_eq!(
            AudioPacket::parse(&[3, 0, 0, 0, 0, 0, 0, 1]),
            Err(AudioError::UnknownKind(3))
        );
        assert_eq!(
            AudioPacket::parse(&[1, 0, 0, 0, 0, 0, 0]),
            Err(AudioError::BadPayload(0))
        );
        let big = vec![1u8; HEADER_LEN + MAX_PAYLOAD + 1];
        assert_eq!(
            AudioPacket::parse(&big),
            Err(AudioError::BadPayload(MAX_PAYLOAD + 1))
        );
    }
}
