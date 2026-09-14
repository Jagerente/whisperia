use std::{mem::ManuallyDrop, sync::Arc};

use windows::{
    core::{implement, IUnknownImpl, Interface, Ref, PWSTR},
    Win32::{
        Foundation::E_NOTIMPL,
        Graphics::{
            Direct3D11::{ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_RESOURCE_MISC_FLAG},
            Dxgi::Common::DXGI_FORMAT_NV12,
        },
        Media::MediaFoundation::{
            eAVEncCommonRateControlMode_CBR, eAVEncCommonRateControlMode_LowDelayVBR,
            eAVEncH264VProfile_High, CODECAPI_AVEncCommonMeanBitRate,
            CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVDefaultBPictureCount,
            CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode,
            ICodecAPI, IMFActivate, IMFAsyncCallback, IMFAsyncCallback_Impl, IMFAsyncResult,
            IMFAttributes, IMFDXGIDeviceManager, IMFMediaEventGenerator, IMFMediaType, IMFSample,
            IMFShutdown, IMFTrackedSample, IMFTransform, METransformHaveOutput,
            METransformNeedInput, MFCreateAttributes, MFCreateDXGISurfaceBuffer,
            MFCreateTrackedSample, MFMediaType_Video, MFNominalRange_16_235,
            MFSampleExtension_CleanPoint, MFTEnum2, MFT_FRIENDLY_NAME_Attribute,
            MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoPrimaries_BT709, MFVideoTransFunc_709,
            MFVideoTransferMatrix_BT709, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_ADAPTER_LUID,
            MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_MESSAGE_COMMAND_FLUSH,
            MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_OF_STREAM,
            MFT_MESSAGE_NOTIFY_END_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
            MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER,
            MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO,
            MF_E_TRANSFORM_NEED_MORE_INPUT, MF_LOW_LATENCY, MF_MT_AVG_BITRATE, MF_MT_MPEG2_PROFILE,
            MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_TRANSFER_FUNCTION, MF_MT_VIDEO_NOMINAL_RANGE,
            MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX, MF_TRANSFORM_ASYNC_UNLOCK,
        },
        System::Com::CoTaskMemFree,
    },
};

use crate::{
    d3d::{AdapterLuid, Device},
    mf, present,
    share::ShareSettings,
    Context, Error,
};

const GOP_SECONDS: u32 = 10;
const INPUT_TEXTURES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EncoderEvent {
    NeedInput,
    HaveOutput,
    InputReleased(usize),
    Gone(windows::core::Error),
}

pub(crate) type Notify = Arc<dyn Fn(EncoderEvent) + Send + Sync>;

pub(crate) struct Output {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

pub(crate) struct Encoder {
    transform: IMFTransform,
    activate: IMFActivate,
    codec: Option<ICodecAPI>,
    own_output: Option<IMFSample>,
    sequence_header: Vec<u8>,
    name: String,
    credit: u32,
    inputs: Vec<ID3D11Texture2D>,
    busy: Vec<bool>,
    notify: Notify,
    stopped: bool,
    _manager: IMFDXGIDeviceManager,
}

struct Candidate {
    activate: IMFActivate,
    name: String,
}

impl Encoder {
    pub fn new(device: &Device, settings: &ShareSettings, notify: Notify) -> Result<Self, Error> {
        mf::startup()?;
        let manager = mf::device_manager(device)?;
        let mut unusable = None;
        for candidate in enumerate(device.adapter().ok())? {
            match Self::open(&candidate, &manager, device, settings, notify.clone()) {
                Ok(encoder) => return Ok(encoder),
                Err(e) => {
                    tracing::warn!("encoder \"{}\" cannot be used: {e}", candidate.name);

                    let _ = unsafe { candidate.activate.ShutdownObject() };
                    unusable.get_or_insert(Error::EncoderUnusable {
                        name: candidate.name,
                        source: Box::new(e),
                    });
                }
            }
        }
        Err(unusable.unwrap_or(Error::NoEncoder))
    }

    fn open(
        candidate: &Candidate,
        manager: &IMFDXGIDeviceManager,
        device: &Device,
        settings: &ShareSettings,
        notify: Notify,
    ) -> Result<Self, Error> {
        let transform: IMFTransform =
            unsafe { candidate.activate.ActivateObject() }.context("activating the encoder")?;
        let opened = Self::configure(&transform, candidate, manager, device, settings, notify);
        if opened.is_err() {
            shut_down(&transform);
        }
        opened
    }

    fn configure(
        transform: &IMFTransform,
        candidate: &Candidate,
        manager: &IMFDXGIDeviceManager,
        device: &Device,
        settings: &ShareSettings,
        notify: Notify,
    ) -> Result<Self, Error> {
        let (size, fps, bitrate) = (settings.size(), settings.fps(), settings.bitrate());
        let attributes =
            unsafe { transform.GetAttributes() }.context("reading the encoder's attributes")?;
        mf::set_u32(&attributes, &MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
        let _ = mf::set_u32(&attributes, &MF_LOW_LATENCY, 1);

        unsafe { transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize) }
            .context("handing the video card to the encoder")?;

        let output = mf::video_type(&MFVideoFormat_H264, Some(size), Some(fps))?;
        mf::set_u32(&output, &MF_MT_AVG_BITRATE, bitrate)?;
        mf::set_u32(
            &output,
            &MF_MT_MPEG2_PROFILE,
            eAVEncH264VProfile_High.0 as u32,
        )?;
        describe_colours(&output);

        unsafe { transform.SetOutputType(0, &output, 0) }
            .context("setting the encoder's output format")?;
        let input = mf::video_type(&MFVideoFormat_NV12, Some(size), Some(fps))?;
        describe_colours(&input);

        unsafe { transform.SetInputType(0, &input, 0) }
            .context("setting the encoder's input format")?;

        let codec: Option<ICodecAPI> = transform.cast().ok();
        if let Some(codec) = &codec {
            let rate_control = [
                eAVEncCommonRateControlMode_LowDelayVBR,
                eAVEncCommonRateControlMode_CBR,
            ]
            .iter()
            .any(|mode| mf::codec_u32(codec, &CODECAPI_AVEncCommonRateControlMode, mode.0 as u32));
            let accepted = [
                rate_control,
                mf::codec_u32(codec, &CODECAPI_AVEncCommonMeanBitRate, bitrate),
                mf::codec_bool(codec, &CODECAPI_AVLowLatencyMode, true),
                mf::codec_u32(codec, &CODECAPI_AVEncMPVDefaultBPictureCount, 0),
                mf::codec_u32(codec, &CODECAPI_AVEncMPVGOPSize, fps * GOP_SECONDS),
            ];
            tracing::debug!(
                name = candidate.name,
                ?accepted,
                "encoder settings (rate control, bitrate, low latency, no B frames, GOP)"
            );
        }

        let info = unsafe { transform.GetOutputStreamInfo(0) }
            .context("asking the encoder about its output")?;
        let own_output = match info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 {
            0 => Some(mf::empty_sample(info.cbSize.max(largest_frame(size)))?),
            _ => None,
        };

        let generator: IMFMediaEventGenerator = transform
            .cast()
            .context("the encoder is not an asynchronous transform")?;
        let events: IMFAsyncCallback = Events {
            notify: notify.clone(),
        }
        .into();
        listen(&generator, &events).context("listening to the encoder")?;

        unsafe { transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0) }
            .context("starting the encoder")?;

        unsafe { transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0) }
            .context("starting the encoder")?;

        let inputs = (0..INPUT_TEXTURES)
            .map(|_| {
                device.texture(
                    size,
                    DXGI_FORMAT_NV12,
                    D3D11_BIND_RENDER_TARGET,
                    D3D11_RESOURCE_MISC_FLAG(0),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            sequence_header: sequence_header(transform),
            transform: transform.clone(),
            activate: candidate.activate.clone(),
            codec,
            own_output,
            name: candidate.name.clone(),
            credit: 0,
            busy: vec![false; inputs.len()],
            inputs,
            notify,
            stopped: false,
            _manager: manager.clone(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn inputs(&self) -> &[ID3D11Texture2D] {
        &self.inputs
    }

    pub fn ready_input(&self) -> Option<usize> {
        if self.credit == 0 {
            return None;
        }
        self.busy.iter().position(|busy| !busy)
    }

    pub fn handle(&mut self, event: EncoderEvent) -> Result<Option<Output>, Error> {
        match event {
            EncoderEvent::NeedInput => {
                self.credit += 1;
                Ok(None)
            }
            EncoderEvent::InputReleased(index) => {
                if let Some(busy) = self.busy.get_mut(index) {
                    *busy = false;
                }
                Ok(None)
            }
            EncoderEvent::HaveOutput => self.take_output(),
            EncoderEvent::Gone(source) => Err(Error::Api {
                what: "the video encoder stopped",
                source,
            }),
        }
    }

    pub fn feed(
        &mut self,
        index: usize,
        time_hns: i64,
        duration_hns: i64,
        keyframe: bool,
    ) -> Result<(), Error> {
        let texture = present(self.inputs.get(index), "the encoder's input")?;
        if keyframe {
            let forced = self
                .codec
                .as_ref()
                .is_some_and(|codec| mf::codec_u32(codec, &CODECAPI_AVEncVideoForceKeyFrame, 1));
            if !forced {
                tracing::debug!("the encoder does not make keyframes on request");
            }
        }

        let buffer = unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false) }
            .context("wrapping a texture for the encoder")?;

        let tracked: IMFTrackedSample =
            unsafe { MFCreateTrackedSample() }.context("creating a media sample")?;
        let sample: IMFSample = tracked.cast().context("creating a media sample")?;
        mf::fill(&sample, &buffer, time_hns)?;

        let _ = unsafe { sample.SetSampleDuration(duration_hns) };
        let released: IMFAsyncCallback = Released {
            index,
            notify: self.notify.clone(),
        }
        .into();

        unsafe { tracked.SetAllocator(&released, None) }.context("watching the encoder's input")?;
        self.busy[index] = true;

        unsafe { self.transform.ProcessInput(0, &sample, 0) }.context("feeding the encoder")?;
        self.credit = self.credit.saturating_sub(1);
        Ok(())
    }

    fn take_output(&mut self) -> Result<Option<Output>, Error> {
        if let Some(own) = &self.own_output {
            mf::empty_again(own)?;
        }
        let mut output = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(self.own_output.clone()),
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
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
            Err(e) => return Err(e).context("collecting the encoder's output"),
        }
        let Some(sample) = sample else {
            return Ok(None);
        };

        let keyframe = unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }
            .is_ok_and(|clean| clean != 0);
        let mut data = mf::sample_bytes(&sample)?;
        if keyframe && !starts_with_parameter_sets(&data) {
            if self.sequence_header.is_empty() {
                self.sequence_header = sequence_header(&self.transform);
            }
            data.splice(0..0, self.sequence_header.iter().copied());
        }
        Ok(Some(Output { data, keyframe }))
    }

    pub fn stop(&mut self) {
        if std::mem::replace(&mut self.stopped, true) {
            return;
        }
        for message in [
            MFT_MESSAGE_COMMAND_FLUSH,
            MFT_MESSAGE_NOTIFY_END_OF_STREAM,
            MFT_MESSAGE_NOTIFY_END_STREAMING,
            MFT_MESSAGE_SET_D3D_MANAGER,
        ] {
            let _ = unsafe { self.transform.ProcessMessage(message, 0) };
        }
        shut_down(&self.transform);

        let _ = unsafe { self.activate.ShutdownObject() };
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn shut_down(transform: &IMFTransform) {
    if let Ok(shutdown) = transform.cast::<IMFShutdown>() {
        let _ = unsafe { shutdown.Shutdown() };
    }
}

fn listen(
    generator: &IMFMediaEventGenerator,
    callback: &IMFAsyncCallback,
) -> windows::core::Result<()> {
    unsafe { generator.BeginGetEvent(callback, generator) }
}

#[implement(IMFAsyncCallback)]
struct Events {
    notify: Notify,
}

impl Events {
    fn next(&self, result: &IMFAsyncResult, again: &IMFAsyncCallback) -> windows::core::Result<()> {
        let generator: IMFMediaEventGenerator = unsafe { result.GetState() }?.cast()?;

        let event = unsafe { generator.EndGetEvent(result) }?;

        let kind = unsafe { event.GetType() }?;
        if kind == METransformNeedInput.0 as u32 {
            (self.notify)(EncoderEvent::NeedInput);
        } else if kind == METransformHaveOutput.0 as u32 {
            (self.notify)(EncoderEvent::HaveOutput);
        }
        listen(&generator, again)
    }
}

impl IMFAsyncCallback_Impl for Events_Impl {
    fn GetParameters(&self, _: *mut u32, _: *mut u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Invoke(&self, result: Ref<IMFAsyncResult>) -> windows::core::Result<()> {
        let again: IMFAsyncCallback = self.to_interface();
        if let Err(e) = result.ok().and_then(|result| self.next(result, &again)) {
            (self.notify)(EncoderEvent::Gone(e));
        }
        Ok(())
    }
}

#[implement(IMFAsyncCallback)]
struct Released {
    index: usize,
    notify: Notify,
}

impl IMFAsyncCallback_Impl for Released_Impl {
    fn GetParameters(&self, _: *mut u32, _: *mut u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Invoke(&self, _: Ref<IMFAsyncResult>) -> windows::core::Result<()> {
        (self.notify)(EncoderEvent::InputReleased(self.index));
        Ok(())
    }
}

fn enumerate(adapter: Option<AdapterLuid>) -> Result<Vec<Candidate>, Error> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let filter = adapter.map(adapter_filter).transpose()?;
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;

    unsafe {
        MFTEnum2(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            filter.as_ref(),
            &mut list,
            &mut count,
        )
    }
    .context("looking for H.264 encoders")?;
    if list.is_null() {
        return Ok(Vec::new());
    }

    let activates: Vec<IMFActivate> = (0..count as usize)
        .filter_map(|i| unsafe { list.add(i).read() })
        .collect();

    unsafe { CoTaskMemFree(Some(list.cast())) };
    Ok(activates
        .into_iter()
        .map(|activate| Candidate {
            name: friendly_name(&activate),
            activate,
        })
        .collect())
}

fn adapter_filter(adapter: AdapterLuid) -> Result<IMFAttributes, Error> {
    let mut filter: Option<IMFAttributes> = None;

    unsafe { MFCreateAttributes(&mut filter, 1) }.context("filtering encoders")?;
    let filter = present(filter, "the encoder filter")?;
    let luid = luid_bytes(adapter);

    unsafe { filter.SetBlob(&MFT_ENUM_ADAPTER_LUID, &luid) }.context("filtering encoders")?;
    Ok(filter)
}

fn luid_bytes(adapter: AdapterLuid) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&adapter.low.to_ne_bytes());
    bytes[4..].copy_from_slice(&adapter.high.to_ne_bytes());
    bytes
}

fn friendly_name(activate: &IMFActivate) -> String {
    let mut text = PWSTR::null();
    let mut length = 0u32;

    let asked = unsafe {
        activate.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut text, &mut length)
    };
    if asked.is_err() || text.is_null() {
        return "hardware encoder".into();
    }

    let name = unsafe { text.to_string() }.unwrap_or_default();

    unsafe { CoTaskMemFree(Some(text.0.cast())) };
    name
}

fn describe_colours(media_type: &IMFMediaType) {
    for (key, value) in [
        (&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0),
        (&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0),
        (&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0),
        (&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0),
    ] {
        let _ = mf::set_u32(media_type, key, value as u32);
    }
}

fn sequence_header(transform: &IMFTransform) -> Vec<u8> {
    let Ok(current) = (unsafe { transform.GetOutputCurrentType(0) }) else {
        return Vec::new();
    };
    let Ok(size) = (unsafe { current.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) }) else {
        return Vec::new();
    };
    let mut header = vec![0u8; size as usize];

    match unsafe { current.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut header, None) } {
        Ok(()) => header,
        Err(_) => Vec::new(),
    }
}

fn largest_frame(size: (u32, u32)) -> u32 {
    let nv12 = u128::from(size.0) * u128::from(size.1) * 3 / 2;
    u32::try_from(nv12).unwrap_or(u32::MAX)
}

pub(crate) fn nal_types(data: &[u8]) -> impl Iterator<Item = u8> + '_ {
    let mut at = 0;
    std::iter::from_fn(move || {
        while at + 3 < data.len() {
            if data[at] == 0 && data[at + 1] == 0 && data[at + 2] == 1 {
                let kind = data[at + 3] & 0x1F;
                at += 3;
                return Some(kind);
            }
            at += 1;
        }
        None
    })
}

fn starts_with_parameter_sets(data: &[u8]) -> bool {
    const SPS: u8 = 7;
    const SLICES: std::ops::RangeInclusive<u8> = 1..=5;
    for kind in nal_types(data) {
        if kind == SPS {
            return true;
        }
        if SLICES.contains(&kind) {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_sets_are_recognized() {
        let aud = [0, 0, 0, 1, 0x09, 0xF0];
        let sps = [0, 0, 0, 1, 0x67, 1, 2, 3];
        let pps = [0, 0, 1, 0x68, 4];
        let idr = [0, 0, 1, 0x65, 9, 9, 9];
        let with: Vec<u8> = [&aud[..], &sps, &pps, &idr].concat();
        let without: Vec<u8> = [&aud[..], &idr].concat();
        assert_eq!(nal_types(&with).collect::<Vec<_>>(), [9, 7, 8, 5]);
        assert!(starts_with_parameter_sets(&with));
        assert!(!starts_with_parameter_sets(&without));
        assert!(!starts_with_parameter_sets(&[]));
        assert!(!starts_with_parameter_sets(&[0, 0, 1]));
    }

    #[test]
    fn the_adapter_filter_is_the_luid_as_windows_lays_it_out() {
        let adapter = AdapterLuid::of(windows::Win32::Foundation::LUID {
            LowPart: 0x0403_0201,
            HighPart: -2,
        });
        assert_eq!(luid_bytes(adapter), [1, 2, 3, 4, 0xFE, 0xFF, 0xFF, 0xFF]);
        assert_eq!(size_of::<windows::Win32::Foundation::LUID>(), 8);
    }

    #[test]
    fn an_output_buffer_holds_the_largest_picture() {
        assert_eq!(largest_frame((1920, 1080)), 3_110_400);
        assert_eq!(largest_frame((u32::MAX, u32::MAX)), u32::MAX);
    }

    #[test]
    fn a_released_input_comes_back_as_an_event_with_its_number() {
        let (tx, rx) = std::sync::mpsc::channel();
        let notify: Notify = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        mf::startup().unwrap();
        let tracked: IMFTrackedSample = unsafe { MFCreateTrackedSample() }.unwrap();
        let released: IMFAsyncCallback = Released { index: 3, notify }.into();
        unsafe { tracked.SetAllocator(&released, None) }.unwrap();
        let held_by_the_encoder: IMFSample = tracked.cast().unwrap();
        drop(tracked);
        assert!(rx.try_recv().is_err(), "still held");
        drop(held_by_the_encoder);
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(EncoderEvent::InputReleased(3))
        );
    }
}
