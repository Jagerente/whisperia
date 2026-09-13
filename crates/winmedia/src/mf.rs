use std::{mem::ManuallyDrop, sync::OnceLock};

use windows::{
    core::GUID,
    Win32::{
        Foundation::{VARIANT_FALSE, VARIANT_TRUE},
        Media::MediaFoundation::{
            ICodecAPI, IMFAttributes, IMFDXGIDeviceManager, IMFMediaBuffer, IMFMediaType,
            IMFSample, MFCreateDXGIDeviceManager, MFCreateMediaType, MFCreateMemoryBuffer,
            MFCreateSample, MFMediaType_Video, MFStartup, MFVideoInterlace_Progressive,
            MFSTARTUP_LITE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
            MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_VERSION,
        },
        System::{
            Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED},
            Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_UI4},
        },
    },
};

use crate::{d3d::Device, present, Context, Error};

pub(crate) const HNS_PER_SECOND: i64 = 10_000_000;

pub(crate) fn hns(duration: std::time::Duration) -> i64 {
    i64::try_from(duration.as_nanos() / 100).unwrap_or(i64::MAX)
}

pub(crate) struct ComThread {
    initialized: bool,
}

impl ComThread {
    pub fn enter() -> Self {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self {
            initialized: result.is_ok(),
        }
    }
}

impl Drop for ComThread {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}

pub(crate) fn startup() -> Result<(), Error> {
    static STARTED: OnceLock<Result<(), windows::core::Error>> = OnceLock::new();
    STARTED
        .get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) })
        .clone()
        .context("starting Media Foundation")
}

pub(crate) fn device_manager(device: &Device) -> Result<IMFDXGIDeviceManager, Error> {
    let mut token = 0u32;
    let mut manager = None;

    unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager) }
        .context("creating the DXGI device manager")?;
    let manager: IMFDXGIDeviceManager = present(manager, "the DXGI device manager")?;

    unsafe { manager.ResetDevice(&device.device, token) }
        .context("handing the device to Media Foundation")?;
    Ok(manager)
}

pub(crate) fn pack(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

pub(crate) fn unpack(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

pub(crate) fn video_type(
    subtype: &GUID,
    size: Option<(u32, u32)>,
    fps: Option<u32>,
) -> Result<IMFMediaType, Error> {
    let media_type = unsafe { MFCreateMediaType() }.context("creating a media type")?;
    let set = "describing the video format";
    set_guid(&media_type, &MF_MT_MAJOR_TYPE, &MFMediaType_Video).context(set)?;
    set_guid(&media_type, &MF_MT_SUBTYPE, subtype).context(set)?;
    set_u32(
        &media_type,
        &MF_MT_INTERLACE_MODE,
        MFVideoInterlace_Progressive.0 as u32,
    )?;
    if let Some((width, height)) = size {
        set_u64(&media_type, &MF_MT_FRAME_SIZE, pack(width, height))?;
    }
    if let Some(fps) = fps {
        set_u64(&media_type, &MF_MT_FRAME_RATE, pack(fps, 1))?;
    }
    set_u64(&media_type, &MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
    Ok(media_type)
}

fn set_guid(attributes: &IMFAttributes, key: &GUID, value: &GUID) -> windows::core::Result<()> {
    unsafe { attributes.SetGUID(key, value) }
}

pub(crate) fn set_u32(attributes: &IMFAttributes, key: &GUID, value: u32) -> Result<(), Error> {
    unsafe { attributes.SetUINT32(key, value) }.context("setting a media attribute")
}

pub(crate) fn set_u64(attributes: &IMFAttributes, key: &GUID, value: u64) -> Result<(), Error> {
    unsafe { attributes.SetUINT64(key, value) }.context("setting a media attribute")
}

fn variant_u32(value: u32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_UI4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { ulVal: value },
            }),
        },
    }
}

fn variant_bool(value: bool) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    boolVal: if value { VARIANT_TRUE } else { VARIANT_FALSE },
                },
            }),
        },
    }
}

pub(crate) fn codec_u32(codec: &ICodecAPI, key: &GUID, value: u32) -> bool {
    let value = variant_u32(value);

    unsafe { codec.SetValue(key, &value) }.is_ok()
}

pub(crate) fn codec_bool(codec: &ICodecAPI, key: &GUID, value: bool) -> bool {
    let value = variant_bool(value);

    unsafe { codec.SetValue(key, &value) }.is_ok()
}

struct Locked<'a> {
    buffer: &'a IMFMediaBuffer,
    bytes: *mut u8,
    capacity: usize,
    length: usize,
}

impl<'a> Locked<'a> {
    fn lock(buffer: &'a IMFMediaBuffer) -> Result<Self, Error> {
        let mut bytes = std::ptr::null_mut();
        let mut capacity = 0u32;
        let mut length = 0u32;

        unsafe { buffer.Lock(&mut bytes, Some(&mut capacity), Some(&mut length)) }
            .context("locking a media buffer")?;
        let locked = Self {
            buffer,
            bytes,
            capacity: capacity as usize,
            length: (length as usize).min(capacity as usize),
        };
        match locked.bytes.is_null() {
            true => Err(Error::Empty("a media buffer")),
            false => Ok(locked),
        }
    }

    fn filled(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.bytes, self.length) }
    }

    fn whole(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.bytes, self.capacity) }
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.buffer.Unlock() };
    }
}

pub(crate) fn memory_sample(data: &[u8], time_hns: i64) -> Result<IMFSample, Error> {
    let length = u32::try_from(data.len()).map_err(|_| Error::TooLarge(data.len()))?;

    let buffer = unsafe { MFCreateMemoryBuffer(length) }.context("allocating a media buffer")?;
    {
        let mut locked = Locked::lock(&buffer)?;
        let target = locked.whole();
        if target.len() < data.len() {
            return Err(Error::TooLarge(data.len()));
        }
        target[..data.len()].copy_from_slice(data);
    }

    unsafe { buffer.SetCurrentLength(length) }.context("filling a media buffer")?;
    sample_of(&buffer, time_hns)
}

pub(crate) fn empty_sample(capacity: u32) -> Result<IMFSample, Error> {
    let buffer =
        unsafe { MFCreateMemoryBuffer(capacity) }.context("allocating the encoder's output")?;
    sample_of(&buffer, 0)
}

pub(crate) fn sample_of(buffer: &IMFMediaBuffer, time_hns: i64) -> Result<IMFSample, Error> {
    let sample = unsafe { MFCreateSample() }.context("creating a media sample")?;
    fill(&sample, buffer, time_hns)?;
    Ok(sample)
}

pub(crate) fn fill(
    sample: &IMFSample,
    buffer: &IMFMediaBuffer,
    time_hns: i64,
) -> Result<(), Error> {
    unsafe { sample.AddBuffer(buffer) }.context("filling a media sample")?;

    unsafe { sample.SetSampleTime(time_hns) }.context("stamping a media sample")
}

pub(crate) fn sample_bytes(sample: &IMFSample) -> Result<Vec<u8>, Error> {
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }.context("reading a media sample")?;
    let locked = Locked::lock(&buffer)?;
    Ok(locked.filled().to_vec())
}

pub(crate) fn empty_again(sample: &IMFSample) -> Result<(), Error> {
    let buffer = unsafe { sample.GetBufferByIndex(0) }.context("reusing the encoder's output")?;

    unsafe { buffer.SetCurrentLength(0) }.context("reusing the encoder's output")?;

    unsafe { sample.DeleteAllItems() }.context("reusing the encoder's output")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_rates_pack_both_ways() {
        assert_eq!(pack(1920, 1080), 0x0000_0780_0000_0438);
        assert_eq!(unpack(pack(3840, 2160)), (3840, 2160));
        assert_eq!(unpack(pack(u32::MAX, 1)), (u32::MAX, 1));
    }

    #[test]
    fn durations_become_hundreds_of_nanoseconds() {
        assert_eq!(hns(std::time::Duration::from_secs(1)), HNS_PER_SECOND);
        assert_eq!(hns(std::time::Duration::from_millis(16)), 160_000);
        assert_eq!(hns(std::time::Duration::MAX), i64::MAX);
    }

    #[test]
    fn a_media_buffer_gives_back_what_was_put_in() {
        startup().unwrap();
        let sample = memory_sample(&[1, 2, 3, 4, 5], 7).unwrap();
        assert_eq!(sample_bytes(&sample).unwrap(), [1, 2, 3, 4, 5]);
        assert_eq!(unsafe { sample.GetSampleTime() }.unwrap(), 7);
        empty_again(&sample).unwrap();
        assert!(sample_bytes(&sample).unwrap().is_empty());
    }
}
