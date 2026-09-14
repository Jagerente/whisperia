use std::mem::ManuallyDrop;

use windows::Win32::{
    Foundation::RECT,
    Graphics::{
        Direct3D11::{
            ID3D11Texture2D, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
            ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView, D3D11_TEX2D_VPIV,
            D3D11_TEX2D_VPOV, D3D11_VIDEO_COLOR, D3D11_VIDEO_COLOR_0, D3D11_VIDEO_COLOR_RGBA,
            D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
            D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
            D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
            D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
        },
        Dxgi::Common::{
            DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_TYPE,
            DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_RATIONAL,
        },
    },
};

use crate::{d3d::Device, present, Context, Error};

const SCREEN: DXGI_COLOR_SPACE_TYPE = DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709;
const STREAM: DXGI_COLOR_SPACE_TYPE = DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    RgbToYuv,
    YuvToRgb,
}

impl Direction {
    fn colour_spaces(self) -> (DXGI_COLOR_SPACE_TYPE, DXGI_COLOR_SPACE_TYPE) {
        match self {
            Self::RgbToYuv => (SCREEN, STREAM),
            Self::YuvToRgb => (STREAM, SCREEN),
        }
    }
}

pub(crate) struct Converter {
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    input_size: (u32, u32),
    outputs: Vec<ID3D11VideoProcessorOutputView>,
}

impl Converter {
    pub fn new(
        device: &Device,
        input_size: (u32, u32),
        output_size: (u32, u32),
        fps: Option<u32>,
        direction: Direction,
        outputs: &[ID3D11Texture2D],
    ) -> Result<Self, Error> {
        let rate = fps.map_or_else(DXGI_RATIONAL::default, |fps| DXGI_RATIONAL {
            Numerator: fps,
            Denominator: 1,
        });
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: input_size.0,
            InputHeight: input_size.1,
            OutputFrameRate: rate,
            OutputWidth: output_size.0,
            OutputHeight: output_size.1,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };

        let enumerator = unsafe { device.video.CreateVideoProcessorEnumerator(&desc) }
            .context("creating the video processor enumerator")?;

        let processor = unsafe { device.video.CreateVideoProcessor(&enumerator, 0) }
            .context("creating the video processor")?;

        let (stream, output) = direction.colour_spaces();
        let black = D3D11_VIDEO_COLOR {
            Anonymous: D3D11_VIDEO_COLOR_0 {
                RGBA: D3D11_VIDEO_COLOR_RGBA {
                    R: 0.0,
                    G: 0.0,
                    B: 0.0,
                    A: 1.0,
                },
            },
        };
        let context = &device.video_context;

        unsafe {
            context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            )
        };

        unsafe { context.VideoProcessorSetStreamColorSpace1(&processor, 0, stream) };

        unsafe { context.VideoProcessorSetOutputColorSpace1(&processor, output) };

        unsafe { context.VideoProcessorSetOutputBackgroundColor(&processor, false, &black) };

        unsafe { context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false) };

        let outputs = outputs
            .iter()
            .map(|texture| output_view(device, &enumerator, texture))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            enumerator,
            processor,
            input_size,
            outputs,
        })
    }

    pub fn input_size(&self) -> (u32, u32) {
        self.input_size
    }

    pub fn blit(
        &self,
        device: &Device,
        input: &ID3D11Texture2D,
        input_slice: u32,
        source: RECT,
        output: usize,
        target: RECT,
    ) -> Result<(), Error> {
        let output = present(self.outputs.get(output), "the video processor output")?;
        let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: input_slice,
                },
            },
        };
        let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
        let context = &device.video_context;

        unsafe {
            device.video.CreateVideoProcessorInputView(
                input,
                &self.enumerator,
                &input_desc,
                Some(&mut input_view),
            )
        }
        .context("creating the video processor input view")?;

        unsafe {
            context.VideoProcessorSetStreamSourceRect(&self.processor, 0, true, Some(&source))
        };

        unsafe { context.VideoProcessorSetStreamDestRect(&self.processor, 0, true, Some(&target)) };

        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(input_view),
            ..Default::default()
        };

        let result = unsafe {
            context.VideoProcessorBlt(&self.processor, output, 0, std::slice::from_ref(&stream))
        };
        drop(ManuallyDrop::into_inner(stream.pInputSurface));
        result.context("converting a picture on the video card")
    }
}

fn output_view(
    device: &Device,
    enumerator: &ID3D11VideoProcessorEnumerator,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11VideoProcessorOutputView, Error> {
    let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
        ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
        },
    };
    let mut view = None;

    unsafe {
        device
            .video
            .CreateVideoProcessorOutputView(texture, enumerator, &desc, Some(&mut view))
    }
    .context("creating the video processor output view")?;
    present(view, "the video processor output view")
}

pub(crate) fn rect(width: u32, height: u32) -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    }
}

pub(crate) fn fit(content: (u32, u32), frame: (u32, u32)) -> RECT {
    let (cw, ch) = (content.0.max(1) as u64, content.1.max(1) as u64);
    let (fw, fh) = (frame.0 as u64, frame.1 as u64);
    let (width, height) = if cw * fh > fw * ch {
        (fw, (fw * ch / cw).max(1))
    } else {
        ((fh * cw / ch).max(1), fh)
    };
    let left = (fw - width.min(fw)) / 2;
    let top = (fh - height.min(fh)) / 2;
    RECT {
        left: left as i32,
        top: top as i32,
        right: (left + width) as i32,
        bottom: (top + height) as i32,
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::Graphics::{
        Direct3D11::{
            D3D11_BIND_FLAG, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
            D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
            D3D11_RESOURCE_MISC_FLAG, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
        },
        Dxgi::Common::{
            DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_FORMAT_R8G8B8A8_UNORM,
            DXGI_SAMPLE_DESC,
        },
    };

    use super::*;

    #[test]
    fn pictures_are_fitted_with_bars_on_the_right_sides() {
        let wide = fit((1920, 1080), (1000, 1000));
        assert_eq!((wide.left, wide.right), (0, 1000));
        assert_eq!(wide.bottom - wide.top, 562);
        assert_eq!(wide.top, (1000 - 562) / 2);

        let tall = fit((1080, 1920), (1000, 1000));
        assert_eq!((tall.top, tall.bottom), (0, 1000));
        assert_eq!(tall.right - tall.left, 562);

        let exact = fit((1920, 1080), (1920, 1080));
        assert_eq!(
            (exact.left, exact.top, exact.right, exact.bottom),
            (0, 0, 1920, 1080)
        );

        let none = fit((1920, 1080), (0, 0));
        assert!(none.right >= none.left && none.bottom >= none.top);
    }

    const COLOURS: [[u8; 3]; 8] = [
        [0, 0, 0],
        [64, 64, 64],
        [128, 128, 128],
        [255, 255, 255],
        [200, 30, 30],
        [30, 200, 30],
        [30, 30, 200],
        [230, 140, 40],
    ];
    const BAND: u32 = 16;
    const SIZE: (u32, u32) = (BAND * COLOURS.len() as u32, BAND);

    fn studio_luma([r, g, b]: [u8; 3]) -> u8 {
        let full = 0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
        (16.0 + full * 219.0 / 255.0).round() as u8
    }

    fn painted(device: &Device) -> ID3D11Texture2D {
        let texture = device
            .texture(
                SIZE,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_BIND_FLAG(D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0),
                D3D11_RESOURCE_MISC_FLAG(0),
            )
            .unwrap();
        let row: Vec<u8> = (0..SIZE.0)
            .flat_map(|x| {
                let [r, g, b] = COLOURS[(x / BAND) as usize];
                [b, g, r, 255]
            })
            .collect();
        let pixels = row.repeat(SIZE.1 as usize);

        unsafe {
            device.context.UpdateSubresource(
                &texture,
                0,
                None,
                pixels.as_ptr().cast(),
                SIZE.0 * 4,
                0,
            )
        };
        texture
    }

    fn first_row(device: &Device, texture: &ID3D11Texture2D, format: DXGI_FORMAT) -> Vec<u8> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: SIZE.0,
            Height: SIZE.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;

        unsafe {
            device
                .device
                .CreateTexture2D(&desc, None, Some(&mut staging))
        }
        .unwrap();
        let staging = staging.unwrap();

        unsafe { device.context.CopyResource(&staging, texture) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();

        unsafe {
            device
                .context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .unwrap();

        let row =
            unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), SIZE.0 as usize * 4) }
                .to_vec();

        unsafe { device.context.Unmap(&staging, 0) };
        row
    }

    fn middles(row: &[u8], bytes_per_pixel: usize) -> Vec<&[u8]> {
        (0..COLOURS.len())
            .map(|band| {
                let x = band * BAND as usize + BAND as usize / 2;
                &row[x * bytes_per_pixel..(x + 1) * bytes_per_pixel]
            })
            .collect()
    }

    #[test]
    #[ignore = "needs a video card"]
    fn colours_keep_their_brightness_and_hue_both_ways() {
        let device = Device::new().unwrap();
        let screen = painted(&device);
        let stream = device
            .texture(
                SIZE,
                DXGI_FORMAT_NV12,
                D3D11_BIND_RENDER_TARGET,
                D3D11_RESOURCE_MISC_FLAG(0),
            )
            .unwrap();
        let shown = device
            .texture(
                SIZE,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                D3D11_BIND_FLAG(D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0),
                D3D11_RESOURCE_MISC_FLAG(0),
            )
            .unwrap();
        let everything = rect(SIZE.0, SIZE.1);
        Converter::new(
            &device,
            SIZE,
            SIZE,
            None,
            Direction::RgbToYuv,
            std::slice::from_ref(&stream),
        )
        .unwrap()
        .blit(&device, &screen, 0, everything, 0, everything)
        .unwrap();
        let luma: Vec<u8> = middles(&first_row(&device, &stream, DXGI_FORMAT_NV12), 1)
            .into_iter()
            .map(|pixel| pixel[0])
            .collect();
        Converter::new(
            &device,
            SIZE,
            SIZE,
            None,
            Direction::YuvToRgb,
            std::slice::from_ref(&shown),
        )
        .unwrap()
        .blit(&device, &stream, 0, everything, 0, everything)
        .unwrap();
        let shown_row = first_row(&device, &shown, DXGI_FORMAT_R8G8B8A8_UNORM);
        let back: Vec<[u8; 3]> = middles(&shown_row, 4)
            .into_iter()
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        let studio = COLOURS.map(studio_luma);
        let near = |a: u8, b: u8| a.abs_diff(b) <= 3;
        assert!(
            luma.iter().zip(&studio).all(|(a, b)| near(*a, *b)),
            "luma {luma:?}, BT.709 studio {studio:?}"
        );
        assert!(
            back.iter()
                .zip(&COLOURS)
                .all(|(a, b)| a.iter().zip(b).all(|(a, b)| near(*a, *b))),
            "back {back:?}, sent {COLOURS:?}"
        );
    }

    #[test]
    fn the_way_back_mirrors_the_way_out() {
        let (screen, stream) = Direction::RgbToYuv.colour_spaces();
        assert_eq!(Direction::YuvToRgb.colour_spaces(), (stream, screen));
        assert_eq!(stream, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
    }
}
