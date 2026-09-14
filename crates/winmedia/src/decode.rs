use std::mem::ManuallyDrop;

use windows::{
    core::Interface,
    Win32::{
        Graphics::Direct3D11::ID3D11Texture2D,
        Media::MediaFoundation::{
            CLSID_MSH264DecoderMFT, IMFDXGIBuffer, IMFDXGIDeviceManager, IMFMediaType, IMFSample,
            IMFTransform, MFVideoFormat_H264, MFVideoFormat_NV12, MFT_MESSAGE_COMMAND_FLUSH,
            MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
            MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER, MF_E_NOTACCEPTING,
            MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_LOW_LATENCY,
            MF_MT_FRAME_SIZE, MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_SUBTYPE,
        },
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
    },
};

use crate::{d3d::Device, mf, present, Context, Error};

const MAX_OUTPUT_TYPES: u32 = 32;
const MAX_PICTURES_PER_INPUT: usize = 8;
const STAMP_STEP_HNS: i64 = mf::HNS_PER_SECOND / 60;

pub(crate) struct DecodedFrame {
    pub texture: ID3D11Texture2D,
    pub slice: u32,
    pub width: u32,
    pub height: u32,
    _sample: IMFSample,
}

pub(crate) enum Decoded {
    Picture(DecodedFrame),
    Nothing,
    NeedKeyframe(Error),
}

pub(crate) struct Decoder {
    transform: IMFTransform,
    display_size: Option<(u32, u32)>,
    time_hns: i64,
    broken: bool,
    pictures: Vec<DecodedFrame>,
    _manager: IMFDXGIDeviceManager,
}

impl Decoder {
    pub fn new(device: &Device) -> Result<Self, Error> {
        mf::startup()?;
        let manager = mf::device_manager(device)?;

        let transform: IMFTransform =
            unsafe { CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER) }
                .context("creating the H.264 decoder")?;

        if let Ok(attributes) = unsafe { transform.GetAttributes() } {
            let _ = mf::set_u32(&attributes, &MF_LOW_LATENCY, 1);
        }

        unsafe { transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize) }
            .context("handing the video card to the decoder")?;
        let input = mf::video_type(&MFVideoFormat_H264, None, None)?;

        unsafe { transform.SetInputType(0, &input, 0) }
            .context("setting the decoder's input format")?;
        let mut decoder = Self {
            transform,
            display_size: None,
            time_hns: 0,
            broken: false,
            pictures: Vec::new(),
            _manager: manager,
        };
        decoder.choose_output()?;
        for message in [
            MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
            MFT_MESSAGE_NOTIFY_START_OF_STREAM,
        ] {
            unsafe { decoder.transform.ProcessMessage(message, 0) }
                .context("starting the decoder")?;
        }
        Ok(decoder)
    }

    pub fn newest(&mut self, data: &[u8], keyframe: bool) -> Decoded {
        if self.broken && !keyframe {
            return Decoded::Nothing;
        }
        self.pictures.clear();
        match self.decode(data) {
            Ok(()) => {
                self.broken = false;
                self.pictures
                    .pop()
                    .map_or(Decoded::Nothing, Decoded::Picture)
            }
            Err(e) => {
                let _ = unsafe { self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0) };
                self.broken = true;
                Decoded::NeedKeyframe(e)
            }
        }
    }

    fn choose_output(&mut self) -> Result<(), Error> {
        for index in 0..MAX_OUTPUT_TYPES {
            let Ok(offered) = (unsafe { self.transform.GetOutputAvailableType(0, index) }) else {
                break;
            };
            if unsafe { offered.GetGUID(&MF_MT_SUBTYPE) }.ok() != Some(MFVideoFormat_NV12) {
                continue;
            }

            unsafe { self.transform.SetOutputType(0, &offered, 0) }
                .context("setting the decoder's output format")?;
            self.display_size = display_size(&offered);
            return Ok(());
        }
        Err(Error::NoNv12Output)
    }

    fn decode(&mut self, data: &[u8]) -> Result<(), Error> {
        self.time_hns += STAMP_STEP_HNS;
        let sample = mf::memory_sample(data, self.time_hns)?;

        match unsafe { self.transform.ProcessInput(0, &sample, 0) } {
            Ok(()) => {}
            Err(e) if e.code() == MF_E_NOTACCEPTING => {
                self.drain()?;

                unsafe { self.transform.ProcessInput(0, &sample, 0) }
                    .context("feeding the decoder")?;
            }
            Err(e) => return Err(e).context("feeding the decoder"),
        }
        self.drain()
    }

    fn drain(&mut self) -> Result<(), Error> {
        for _ in 0..MAX_PICTURES_PER_INPUT {
            let mut output = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(None),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;

            let result = unsafe { self.transform.ProcessOutput(0, &mut output, &mut status) };
            let [output] = output;
            let sample = ManuallyDrop::into_inner(output.pSample);
            drop(ManuallyDrop::into_inner(output.pEvents));
            match result {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.choose_output()?;
                    continue;
                }
                Err(e) => return Err(e).context("decoding a picture"),
            }
            let Some(sample) = sample else {
                return Ok(());
            };
            let picture = self.picture(sample)?;
            self.pictures.push(picture);
        }
        Ok(())
    }

    fn picture(&self, sample: IMFSample) -> Result<DecodedFrame, Error> {
        let buffer = unsafe { sample.GetBufferByIndex(0) }.context("reading a decoded picture")?;
        let dxgi: IMFDXGIBuffer = buffer.cast().map_err(|_| Error::NotOnVideoCard)?;
        let mut raw = std::ptr::null_mut();

        unsafe { dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw) }
            .context("reading a decoded picture")?;
        let texture = present(
            (!raw.is_null()).then(|| unsafe { ID3D11Texture2D::from_raw(raw) }),
            "the decoded picture's texture",
        )?;

        let slice = unsafe { dxgi.GetSubresourceIndex() }.context("reading a decoded picture")?;
        let (texture_width, texture_height) = crate::d3d::texture_size(&texture);
        let (width, height) = match self.display_size {
            Some((w, h)) => (w.min(texture_width), h.min(texture_height)),
            None => (texture_width, texture_height),
        };
        Ok(DecodedFrame {
            texture,
            slice,
            width,
            height,
            _sample: sample,
        })
    }
}

fn display_size(offered: &IMFMediaType) -> Option<(u32, u32)> {
    let coded = unsafe { offered.GetUINT64(&MF_MT_FRAME_SIZE) }
        .map(mf::unpack)
        .ok()
        .filter(|(w, h)| *w > 0 && *h > 0)?;
    let mut area = [0u8; 16];

    let visible = unsafe { offered.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut area, None) }
        .ok()
        .map(|()| aperture_size(&area))
        .filter(|(w, h)| *w > 0 && *h > 0 && *w <= coded.0 && *h <= coded.1);
    Some(visible.unwrap_or(coded))
}

fn aperture_size(area: &[u8; 16]) -> (u32, u32) {
    let field =
        |at: usize| i32::from_le_bytes([area[at], area[at + 1], area[at + 2], area[at + 3]]);
    (field(8).max(0) as u32, field(12).max(0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_visible_area_is_read_from_the_aperture() {
        let mut area = [0u8; 16];
        area[8..12].copy_from_slice(&1920i32.to_le_bytes());
        area[12..16].copy_from_slice(&1080i32.to_le_bytes());
        assert_eq!(aperture_size(&area), (1920, 1080));
        area[12..16].copy_from_slice(&(-5i32).to_le_bytes());
        assert_eq!(aperture_size(&area), (1920, 0));
    }
}
