use crate::config::ShareConfig;

pub const SMALLEST_STREAM: (u32, u32) = (256, 144);
pub const FPS_RANGE: std::ops::RangeInclusive<u32> = 5..=120;
const BASE_BITRATE: u64 = 2_000_000;
const BITS_PER_100_PIXELS: u64 = 6;
const HIGHEST_BITRATE: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stream {
    pub size: (u32, u32),
    pub fps: u32,
    pub bitrate: u32,
}

pub fn screen_label(index: usize) -> String {
    format!("Screen {}", index + 1)
}

pub fn screen_detail(size: (u32, u32)) -> String {
    format!("{} x {}", size.0, size.1)
}

pub fn height_limit(config: &ShareConfig) -> Option<u32> {
    (config.max_height > 0).then_some(config.max_height)
}

pub fn stream(source: (u32, u32), config: &ShareConfig, largest: (u32, u32)) -> Stream {
    let fps = config.fps.clamp(*FPS_RANGE.start(), *FPS_RANGE.end());
    let size = stream_size(source, height_limit(config), largest);
    Stream {
        size,
        fps,
        bitrate: bitrate(size, fps),
    }
}

pub fn stream_size(source: (u32, u32), limit: Option<u32>, largest: (u32, u32)) -> (u32, u32) {
    let (width, height) = (u64::from(source.0.max(1)), u64::from(source.1.max(1)));
    let limit = u64::from(limit.map_or(largest.1, |lines| lines.min(largest.1)));
    let (mut num, mut den) = (1u64, 1u64);
    if height > limit {
        (num, den) = (limit, height);
    }
    if width * num > u64::from(largest.0) * den {
        (num, den) = (u64::from(largest.0), width);
    }
    let even = |v: u64| ((v.max(2) / 2) * 2) as u32;
    (
        even(width * num / den).max(SMALLEST_STREAM.0),
        even(height * num / den).max(SMALLEST_STREAM.1),
    )
}

#[derive(Debug, Clone)]
pub struct Wanted {
    source: (u32, u32),
    config: ShareConfig,
    largest: (u32, u32),
    stream: Stream,
}

impl Wanted {
    pub fn new(source: (u32, u32), config: &ShareConfig, largest: (u32, u32)) -> Self {
        Self {
            source,
            config: config.clone(),
            largest,
            stream: stream(source, config, largest),
        }
    }

    pub fn stream(&self) -> Stream {
        self.stream
    }

    pub fn reconfigure(&mut self, config: &ShareConfig) -> Option<Stream> {
        self.config = config.clone();
        self.settle()
    }

    pub fn follow(&mut self, source: (u32, u32)) -> Option<Stream> {
        self.source = source;
        self.settle()
    }

    fn settle(&mut self) -> Option<Stream> {
        let next = stream(self.source, &self.config, self.largest);
        (next != self.stream).then(|| {
            self.stream = next;
            next
        })
    }
}

pub fn bitrate(size: (u32, u32), fps: u32) -> u32 {
    let pixels_per_second = u64::from(size.0) * u64::from(size.1) * u64::from(fps);
    (BASE_BITRATE + pixels_per_second * BITS_PER_100_PIXELS / 100).min(HIGHEST_BITRATE) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const LARGEST: (u32, u32) = (4096, 2304);

    #[test]
    fn the_bitrate_follows_size_and_rate() {
        let mbit = |size, fps| bitrate(size, fps) as f32 / 1e6;
        assert!((3.0..5.0).contains(&mbit((1280, 720), 30)));
        assert!((8.0..11.0).contains(&mbit((1920, 1080), 60)));
        assert!((14.0..17.0).contains(&mbit((2560, 1440), 60)));
        assert!(mbit((1920, 1080), 15) < mbit((1920, 1080), 30));
        assert!(mbit((4096, 2304), 120) <= 100.0);
    }

    #[test]
    fn stream_sizes_are_bounded_and_even() {
        let size = |source, limit| stream_size(source, limit, LARGEST);
        assert_eq!(size((1920, 1080), Some(1440)), (1920, 1080));
        assert_eq!(size((3840, 2160), Some(1440)), (2560, 1440));
        assert_eq!(size((3840, 2160), None), (3840, 2160));
        assert_eq!(size((2560, 1440), Some(1080)), (1920, 1080));
        assert_eq!(size((1281, 721), None), (1280, 720));
        assert_eq!(size((7680, 1080), None), (4096, 576));
        assert_eq!(size((40, 20), Some(1440)), SMALLEST_STREAM);
        assert_eq!(size((0, 0), Some(1440)), SMALLEST_STREAM);
        let (w, h) = size((1000, 9000), None);
        assert!(h <= LARGEST.1 && w % 2 == 0 && h % 2 == 0);
    }

    #[test]
    fn the_settings_become_a_stream_the_encoder_takes() {
        let config = |fps, max_height| ShareConfig { fps, max_height };
        assert_eq!(height_limit(&config(60, 0)), None);
        assert_eq!(height_limit(&config(60, 720)), Some(720));
        let chosen = stream((2560, 1440), &config(30, 720), LARGEST);
        assert_eq!((chosen.size, chosen.fps), ((1280, 720), 30));
        assert_eq!(chosen.bitrate, bitrate((1280, 720), 30));
        assert_eq!(stream((2560, 1440), &config(1, 0), LARGEST).fps, 5);
        assert_eq!(stream((2560, 1440), &config(1000, 0), LARGEST).fps, 120);
        assert_eq!(
            stream((2560, 1440), &config(60, 0), LARGEST).size,
            (2560, 1440)
        );
    }

    #[test]
    fn a_share_follows_its_source_and_keeps_the_chosen_quality() {
        let mut wanted = Wanted::new(
            (1920, 1080),
            &ShareConfig {
                fps: 30,
                max_height: 720,
            },
            LARGEST,
        );
        assert_eq!(wanted.stream().size, (1280, 720));
        assert_eq!(
            wanted.follow((1000, 800)).map(|s| (s.size, s.fps)),
            Some(((900, 720), 30))
        );
        assert_eq!(
            wanted.follow((1000, 800)),
            None,
            "the same size is no change"
        );
        assert_eq!(
            wanted
                .reconfigure(&ShareConfig {
                    fps: 60,
                    max_height: 0
                })
                .map(|s| (s.size, s.fps)),
            Some(((1000, 800), 60))
        );
        assert_eq!(wanted.stream().size, (1000, 800));
    }

    #[test]
    fn sources_are_named_in_plain_text() {
        assert_eq!(screen_label(0), "Screen 1");
        assert_eq!(screen_label(1), "Screen 2");
        assert_eq!(screen_detail((2560, 1440)), "2560 x 1440");
        assert!(screen_detail((2560, 1440)).is_ascii());
    }
}
