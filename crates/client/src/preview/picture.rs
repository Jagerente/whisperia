use std::io::Cursor;

use image::{imageops::FilterType, ImageReader, Limits};

const MOST_SIDE: u32 = 8192;
const MOST_DECODED_BYTES: u64 = 128 * 1024 * 1024;
const SHOWN_WIDTH: u32 = 400;
const SHOWN_HEIGHT: u32 = 300;
const SMALLEST_SIDE: u32 = 16;

#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}x{})", self.width, self.height)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PictureError {
    #[error("cannot read the picture: {0}")]
    Unreadable(#[from] image::ImageError),
    #[error("cannot tell what kind of picture this is: {0}")]
    Unknown(#[from] std::io::Error),
    #[error("a picture of {0}x{1} is too small to show")]
    TooSmall(u32, u32),
}

impl Picture {
    pub fn decode(bytes: &[u8]) -> Result<Self, PictureError> {
        let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(MOST_SIDE);
        limits.max_image_height = Some(MOST_SIDE);
        limits.max_alloc = Some(MOST_DECODED_BYTES);
        reader.limits(limits);
        let decoded = reader.decode()?;
        if decoded.width() < SMALLEST_SIDE || decoded.height() < SMALLEST_SIDE {
            return Err(PictureError::TooSmall(decoded.width(), decoded.height()));
        }
        let fitted = if decoded.width() > SHOWN_WIDTH || decoded.height() > SHOWN_HEIGHT {
            decoded.resize(SHOWN_WIDTH, SHOWN_HEIGHT, FilterType::Triangle)
        } else {
            decoded
        };
        let rgba = fitted.into_rgba8();
        Ok(Self {
            width: rgba.width(),
            height: rgba.height(),
            rgba: rgba.into_raw(),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn plain(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        Self {
            width,
            height,
            rgba: rgba.repeat((width * height) as usize),
        }
    }
}

#[cfg(test)]
mod tests {
    use image::{ImageFormat, RgbaImage};

    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        RgbaImage::from_pixel(width, height, image::Rgba([255, 0, 84, 255]))
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn a_large_picture_is_shrunk_to_the_card_keeping_its_shape() {
        let picture = Picture::decode(&png(1200, 630)).unwrap();
        assert_eq!((picture.width(), picture.height()), (400, 210));
        assert_eq!(picture.rgba().len(), 400 * 210 * 4);
        assert_eq!(&picture.rgba()[..4], &[255, 0, 84, 255]);
    }

    #[test]
    fn a_small_picture_stays_as_it_is() {
        let picture = Picture::decode(&png(120, 80)).unwrap();
        assert_eq!((picture.width(), picture.height()), (120, 80));
    }

    #[test]
    fn a_tiny_or_broken_picture_is_refused() {
        assert!(matches!(
            Picture::decode(&png(8, 8)),
            Err(PictureError::TooSmall(8, 8))
        ));
        assert!(Picture::decode(b"<html>not a picture</html>").is_err());
        assert!(Picture::decode(&png(400, 300)[..60]).is_err());
    }
}
