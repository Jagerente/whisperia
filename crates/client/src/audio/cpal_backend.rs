use std::sync::Arc;

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, SizedSample,
};

use super::{
    ring_capacity, Alarm, AudioBackend, AudioError, CaptureStream, DeviceList, DeviceWatch,
    DevicesChanged, Failure, PlaybackStream, Stream, StreamShared, Underruns, Wake,
};

pub struct CpalBackend;

struct Opened(cpal::Stream);

impl Stream for Opened {
    fn play(&self) -> Result<(), AudioError> {
        self.0.play().map_err(backend_err)
    }
}

fn backend_err(e: impl std::fmt::Display) -> AudioError {
    AudioError::Backend(e.to_string())
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_owned())
}

fn find_device(
    kind: &'static str,
    wanted: Option<&str>,
    default: Option<cpal::Device>,
    all: impl Iterator<Item = cpal::Device>,
) -> Result<cpal::Device, AudioError> {
    match wanted {
        None => {
            let id = default
                .and_then(|default| default.id().ok())
                .ok_or(AudioError::NoDevice(kind))?;
            all.into_iter()
                .find(|d| d.id().ok().as_ref() == Some(&id))
                .ok_or(AudioError::NoDevice(kind))
        }
        Some(name) => all
            .into_iter()
            .find(|d| device_name(d).as_deref() == Some(name))
            .ok_or_else(|| AudioError::NotFound {
                kind,
                name: name.to_owned(),
            }),
    }
}

fn failure(kind: cpal::ErrorKind) -> Option<Failure> {
    match kind {
        cpal::ErrorKind::Xrun
        | cpal::ErrorKind::DeviceChanged
        | cpal::ErrorKind::RealtimeDenied => None,
        cpal::ErrorKind::StreamInvalidated | cpal::ErrorKind::DeviceNotAvailable => {
            Some(Failure::Invalidated)
        }
        _ => Some(Failure::Broken),
    }
}

fn raise(alarm: Alarm) -> impl FnMut(cpal::Error) + Send + 'static {
    move |e| {
        if let Some(failure) = failure(e.kind()) {
            tracing::warn!("sound device: {e}");
            alarm(failure);
        }
    }
}

impl AudioBackend for CpalBackend {
    fn devices(&self) -> DeviceList {
        list_devices()
    }

    fn open_capture(
        &self,
        device: Option<&str>,
        wake: Arc<Wake>,
        alarm: Alarm,
    ) -> Result<CaptureStream, AudioError> {
        open_capture(device, wake, alarm)
    }

    fn open_playback(
        &self,
        device: Option<&str>,
        wake: Arc<Wake>,
        alarm: Alarm,
    ) -> Result<PlaybackStream, AudioError> {
        open_playback(device, wake, alarm)
    }

    fn watch_devices(&self, changed: DevicesChanged) -> Result<DeviceWatch, AudioError> {
        #[cfg(windows)]
        {
            super::devices::watch(changed)
        }
        #[cfg(not(windows))]
        {
            let _ = changed;
            Err(AudioError::Backend(
                "device changes are not heard on this system".into(),
            ))
        }
    }
}

fn list_devices() -> DeviceList {
    let host = cpal::default_host();
    let names = |devices: Option<Vec<cpal::Device>>| {
        devices
            .unwrap_or_default()
            .iter()
            .filter_map(device_name)
            .collect()
    };
    DeviceList {
        inputs: names(host.input_devices().ok().map(Iterator::collect)),
        outputs: names(host.output_devices().ok().map(Iterator::collect)),
    }
}

fn open_capture(
    device: Option<&str>,
    wake: Arc<Wake>,
    alarm: Alarm,
) -> Result<CaptureStream, AudioError> {
    let host = cpal::default_host();
    let all = host.input_devices().map_err(backend_err)?;
    let device = find_device("input", device, host.default_input_device(), all)?;
    let supported = device.default_input_config().map_err(backend_err)?;
    let config = supported.config();

    let (producer, consumer) = rtrb::RingBuffer::new(ring_capacity(config.sample_rate));
    let shared = Arc::new(StreamShared::default());
    let ctx = CaptureCtx {
        producer,
        shared: shared.clone(),
        wake,
        channels: config.channels as usize,
    };

    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_capture::<f32>(&device, config, ctx, alarm),
        SampleFormat::I16 => build_capture::<i16>(&device, config, ctx, alarm),
        SampleFormat::I32 => build_capture::<i32>(&device, config, ctx, alarm),
        SampleFormat::U16 => build_capture::<u16>(&device, config, ctx, alarm),
        other => Err(AudioError::Format(other.to_string())),
    }?;

    Ok(CaptureStream {
        sample_rate: config.sample_rate,
        samples: consumer,
        shared,
        stream: Box::new(Opened(stream)),
    })
}

fn open_playback(
    device: Option<&str>,
    wake: Arc<Wake>,
    alarm: Alarm,
) -> Result<PlaybackStream, AudioError> {
    let host = cpal::default_host();
    let all = host.output_devices().map_err(backend_err)?;
    let device = find_device("output", device, host.default_output_device(), all)?;
    let supported = device.default_output_config().map_err(backend_err)?;
    let config = supported.config();

    let (producer, consumer) = rtrb::RingBuffer::new(ring_capacity(config.sample_rate));
    let shared = Arc::new(StreamShared::default());
    let ctx = PlaybackCtx {
        consumer,
        shared: shared.clone(),
        wake,
        channels: config.channels as usize,
        underruns: Underruns::default(),
    };

    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_playback::<f32>(&device, config, ctx, alarm),
        SampleFormat::I16 => build_playback::<i16>(&device, config, ctx, alarm),
        SampleFormat::I32 => build_playback::<i32>(&device, config, ctx, alarm),
        SampleFormat::U16 => build_playback::<u16>(&device, config, ctx, alarm),
        other => Err(AudioError::Format(other.to_string())),
    }?;

    Ok(PlaybackStream {
        sample_rate: config.sample_rate,
        samples: producer,
        shared,
        stream: Box::new(Opened(stream)),
    })
}

struct CaptureCtx {
    producer: rtrb::Producer<f32>,
    shared: Arc<StreamShared>,
    wake: Arc<Wake>,
    channels: usize,
}

fn build_capture<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut ctx: CaptureCtx,
    alarm: Alarm,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let channels = ctx.channels.max(1);
                let frames = data.len() / channels;
                ctx.shared.set_period(frames);

                let room = frames.min(ctx.producer.slots());
                if let Ok(chunk) = ctx.producer.write_chunk_uninit(room) {
                    let used = channels.min(2);
                    chunk.fill_from_iter(data.chunks_exact(channels).map(|frame| {
                        frame[..used]
                            .iter()
                            .map(|s| f32::from_sample(*s))
                            .sum::<f32>()
                            / used as f32
                    }));
                }
                if room < frames {
                    ctx.shared.count_lost((frames - room) as u64);
                }
                ctx.wake.wake();
            },
            raise(alarm),
            None,
        )
        .map_err(backend_err)
}

struct PlaybackCtx {
    consumer: rtrb::Consumer<f32>,
    shared: Arc<StreamShared>,
    wake: Arc<Wake>,
    channels: usize,
    underruns: Underruns,
}

fn build_playback<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut ctx: PlaybackCtx,
    alarm: Alarm,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let channels = ctx.channels.max(1);
                let frames = data.len() / channels;
                ctx.shared.set_period(frames);

                let available = frames.min(ctx.consumer.slots());
                let mut out = data.chunks_exact_mut(channels);
                if let Ok(chunk) = ctx.consumer.read_chunk(available) {
                    let (head, tail) = chunk.as_slices();
                    for (sample, frame) in head.iter().chain(tail).zip(&mut out) {
                        frame.fill(T::from_sample(*sample));
                    }
                    chunk.commit_all();
                }
                for frame in out {
                    frame.fill(T::EQUILIBRIUM);
                }
                if ctx.underruns.is_underrun(frames, available) {
                    ctx.shared.count_lost(1);
                }
                ctx.wake.wake();
            },
            raise(alarm),
            None,
        )
        .map_err(backend_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_that_lost_its_device_is_opened_again_and_a_broken_one_is_not() {
        assert_eq!(failure(cpal::ErrorKind::Xrun), None);
        assert_eq!(
            failure(cpal::ErrorKind::StreamInvalidated),
            Some(Failure::Invalidated)
        );
        assert_eq!(
            failure(cpal::ErrorKind::DeviceNotAvailable),
            Some(Failure::Invalidated)
        );
        assert_eq!(failure(cpal::ErrorKind::DeviceBusy), Some(Failure::Broken));
    }

    #[test]
    #[ignore = "listens to the real sound devices"]
    fn windows_tells_of_device_changes() {
        let watch = CpalBackend.watch_devices(Arc::new(|_| {}));
        assert!(watch.is_ok(), "{:?}", watch.err());
    }

    #[test]
    #[ignore = "opens the real sound devices"]
    fn the_default_devices_open_as_the_devices_they_are() {
        assert!(!CpalBackend.devices().outputs.is_empty());
        let speakers = CpalBackend.open_playback(None, Arc::default(), Box::new(|_| {}));
        assert!(speakers.is_ok(), "{:?}", speakers.err());
        let microphone = CpalBackend.open_capture(None, Arc::default(), Box::new(|_| {}));
        assert!(microphone.is_ok(), "{:?}", microphone.err());
    }
}
