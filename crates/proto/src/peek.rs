use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Glance {
    pub protocol: u16,
    pub name: String,
    pub rooms: u32,
    pub people: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{encode_frame, FrameDecoder};

    #[test]
    fn a_glance_reads_the_same_in_every_version() {
        let glance = Glance {
            protocol: 1,
            name: "DESKTOP-AV83".into(),
            rooms: 1,
            people: 3,
        };
        let frame = encode_frame(&glance).unwrap();
        assert_eq!(frame[..4], [0, 16, 1, 12]);
        assert_eq!(&frame[4..16], b"DESKTOP-AV83");
        assert_eq!(frame[16..], [1, 3]);

        let mut decoder = FrameDecoder::new();
        decoder.push(&frame);
        assert_eq!(decoder.next_message::<Glance>().unwrap(), Some(glance));
    }
}
