use audioadapter_buffers::direct::SequentialSlice;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use whisperia_proto::{FRAME_SAMPLES, SAMPLE_RATE};

#[derive(Debug, thiserror::Error)]
pub enum ResampleError {
    #[error("unsupported device sample rate {0} Hz")]
    Rate(u32),
    #[error("resampler: {0}")]
    Construct(#[from] rubato::ResamplerConstructionError),
    #[error("resampler: {0}")]
    Process(#[from] rubato::ResampleError),
    #[error("resampler buffer: {0}")]
    Buffer(String),
}

fn sinc(ratio: f64, fixed: FixedAsync) -> Result<Async<f32>, ResampleError> {
    let params = SincInterpolationParameters {
        sinc_len: 64,
        f_cutoff: None,
        oversampling_factor: 128,
        interpolation: SincInterpolationType::Linear,
        window: WindowFunction::BlackmanHarris2,
    };
    Ok(Async::new_sinc(
        ratio,
        1.0,
        &params,
        FRAME_SAMPLES,
        1,
        fixed,
    )?)
}

fn check_rate(rate: u32) -> Result<(), ResampleError> {
    if (8_000..=384_000).contains(&rate) {
        Ok(())
    } else {
        Err(ResampleError::Rate(rate))
    }
}

fn buffer_err(e: impl std::fmt::Display) -> ResampleError {
    ResampleError::Buffer(e.to_string())
}

pub struct ToWire {
    inner: Option<Async<f32>>,
}

impl ToWire {
    pub fn new(device_rate: u32) -> Result<Self, ResampleError> {
        check_rate(device_rate)?;
        let inner = (device_rate != SAMPLE_RATE)
            .then(|| {
                sinc(
                    f64::from(SAMPLE_RATE) / f64::from(device_rate),
                    FixedAsync::Output,
                )
            })
            .transpose()?;
        Ok(Self { inner })
    }

    pub fn input_needed(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(FRAME_SAMPLES, |r| r.input_frames_next())
    }

    pub fn input_max(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(FRAME_SAMPLES, |r| r.input_frames_max())
    }

    pub fn process(&mut self, input: &[f32], out: &mut [f32]) -> Result<(), ResampleError> {
        debug_assert_eq!(out.len(), FRAME_SAMPLES);
        let Some(r) = self.inner.as_mut() else {
            out.copy_from_slice(input);
            return Ok(());
        };
        let src = SequentialSlice::new(input, 1, input.len()).map_err(buffer_err)?;
        let mut dst = SequentialSlice::new_mut(out, 1, FRAME_SAMPLES).map_err(buffer_err)?;
        r.process_into_buffer(&src, &mut dst, None)?;
        Ok(())
    }
}

pub struct FromWire {
    inner: Option<Async<f32>>,
    out: Vec<f32>,
}

impl FromWire {
    pub fn new(device_rate: u32) -> Result<Self, ResampleError> {
        check_rate(device_rate)?;
        let inner = (device_rate != SAMPLE_RATE)
            .then(|| {
                sinc(
                    f64::from(device_rate) / f64::from(SAMPLE_RATE),
                    FixedAsync::Input,
                )
            })
            .transpose()?;
        let out_len = inner
            .as_ref()
            .map_or(FRAME_SAMPLES, |r| r.output_frames_max());
        Ok(Self {
            inner,
            out: vec![0.0; out_len],
        })
    }

    pub fn process(&mut self, frame: &[f32]) -> Result<&[f32], ResampleError> {
        debug_assert_eq!(frame.len(), FRAME_SAMPLES);
        let Some(r) = self.inner.as_mut() else {
            self.out.copy_from_slice(frame);
            return Ok(&self.out);
        };
        let capacity = self.out.len();
        let src = SequentialSlice::new(frame, 1, FRAME_SAMPLES).map_err(buffer_err)?;
        let mut dst =
            SequentialSlice::new_mut(&mut self.out[..], 1, capacity).map_err(buffer_err)?;
        let (_, written) = r.process_into_buffer(&src, &mut dst, None)?;
        Ok(&self.out[..written])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, start: usize, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                ((start + i) as f32 / rate as f32 * 1000.0 * std::f32::consts::TAU).sin() * 0.5
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn passthrough_at_48k() {
        let mut up = ToWire::new(48_000).unwrap();
        assert_eq!(up.input_needed(), FRAME_SAMPLES);
        let input = sine(48_000, 0, FRAME_SAMPLES);
        let mut out = vec![0.0; FRAME_SAMPLES];
        up.process(&input, &mut out).unwrap();
        assert_eq!(out, input);

        let mut down = FromWire::new(48_000).unwrap();
        assert_eq!(down.process(&input).unwrap(), &input[..]);
    }

    #[test]
    fn rate_is_converted_in_both_directions_without_losing_samples() {
        let mut up = ToWire::new(44_100).unwrap();
        let (mut consumed, mut level) = (0usize, 0.0);
        let mut out = vec![0.0; FRAME_SAMPLES];
        for _ in 0..100 {
            let n = up.input_needed();
            assert!(n <= up.input_max());
            up.process(&sine(44_100, consumed, n), &mut out).unwrap();
            consumed += n;
            level = rms(&out);
        }

        assert!(
            (consumed as i64 - 44_100).abs() < 200,
            "consumed {consumed}"
        );
        assert!((level - 0.3535).abs() < 0.02, "rms {level}");

        let mut down = FromWire::new(44_100).unwrap();
        let mut produced = 0usize;
        for f in 0..100 {
            produced += down
                .process(&sine(48_000, f * FRAME_SAMPLES, FRAME_SAMPLES))
                .unwrap()
                .len();
        }
        assert!(
            (produced as i64 - 44_100).abs() < 200,
            "produced {produced}"
        );
    }

    #[test]
    fn absurd_rates_are_rejected() {
        assert!(matches!(ToWire::new(0), Err(ResampleError::Rate(0))));
        assert!(FromWire::new(1_000_000).is_err());
    }
}
