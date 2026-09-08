use serde::{Deserialize, Serialize};

use crate::PeerId;

pub const HEADER_LEN: usize = 11;

pub const MAX_PICTURE: usize = 4 * 1024 * 1024;
pub const MAX_IN_FLIGHT: usize = 8;

const FLAG_KEYFRAME: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaHello {
    pub protocol: u16,
    pub peer: PeerId,
    pub token: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VideoError {
    #[error("picture stream too short ({0} bytes)")]
    TooShort(usize),
    #[error("picture of {0} bytes is out of range")]
    BadSize(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureHeader {
    pub sharer: PeerId,
    pub seq: u32,
    pub keyframe: bool,
    pub width: u16,
    pub height: u16,
}

impl PictureHeader {
    pub fn parse(stream: &[u8]) -> Result<(Self, &[u8]), VideoError> {
        if stream.len() < HEADER_LEN {
            return Err(VideoError::TooShort(stream.len()));
        }
        let (header, picture) = stream.split_at(HEADER_LEN);
        if picture.is_empty() || picture.len() > MAX_PICTURE {
            return Err(VideoError::BadSize(picture.len()));
        }
        Ok((
            Self {
                sharer: u16::from_be_bytes([header[0], header[1]]),
                seq: u32::from_be_bytes([header[2], header[3], header[4], header[5]]),
                keyframe: header[6] & FLAG_KEYFRAME != 0,
                width: u16::from_be_bytes([header[7], header[8]]),
                height: u16::from_be_bytes([header[9], header[10]]),
            },
            picture,
        ))
    }

    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..2].copy_from_slice(&self.sharer.to_be_bytes());
        out[2..6].copy_from_slice(&self.seq.to_be_bytes());
        out[6] = if self.keyframe { FLAG_KEYFRAME } else { 0 };
        out[7..9].copy_from_slice(&self.width.to_be_bytes());
        out[9..11].copy_from_slice(&self.height.to_be_bytes());
        out
    }
}

pub fn stamp_sharer(stream: &mut [u8], sharer: PeerId) -> Result<(), VideoError> {
    if stream.len() < HEADER_LEN {
        return Err(VideoError::TooShort(stream.len()));
    }
    stream[0..2].copy_from_slice(&sharer.to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_stamping() {
        let header = PictureHeader {
            sharer: 0,
            seq: 0xDEAD_BEEF,
            keyframe: true,
            width: 2560,
            height: 1440,
        };
        let mut stream = header.to_bytes().to_vec();
        stream.extend_from_slice(&[1, 2, 3]);
        assert_eq!(
            PictureHeader::parse(&stream).unwrap(),
            (header, &[1u8, 2, 3][..])
        );

        stamp_sharer(&mut stream, 513).unwrap();
        let (stamped, picture) = PictureHeader::parse(&stream).unwrap();
        assert_eq!(
            stamped,
            PictureHeader {
                sharer: 513,
                ..header
            }
        );
        assert_eq!(picture, [1, 2, 3]);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(PictureHeader::parse(&[0; 5]), Err(VideoError::TooShort(5)));
        assert_eq!(
            PictureHeader::parse(&[0; HEADER_LEN]),
            Err(VideoError::BadSize(0))
        );
        let big = vec![0u8; HEADER_LEN + MAX_PICTURE + 1];
        assert_eq!(
            PictureHeader::parse(&big),
            Err(VideoError::BadSize(MAX_PICTURE + 1))
        );
        assert_eq!(stamp_sharer(&mut [0; 3], 7), Err(VideoError::TooShort(3)));
    }
}
