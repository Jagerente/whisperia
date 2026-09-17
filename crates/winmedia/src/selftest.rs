use std::{
    sync::mpsc::{Receiver, Sender},
    time::{Duration, Instant},
};

use windows::{
    core::Interface,
    Win32::Graphics::{
        Direct3D11::{
            ID3D11Device1, ID3D11Device5, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D,
            D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_STAGING,
        },
        Dxgi::Common::{
            DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
        },
    },
};

use crate::{
    capture::{Capture, CaptureEvent},
    d3d::Device,
    mf::ComThread,
    present,
    shared::{ReaderQueue, SharedTextures},
    sources::{list_sources, ListedSource, Source},
    Context, EncodedFrame, EndReason, Error, Share, ShareSettings, SharedDecoder, SharedEvent,
};

const FPS: u32 = 60;
const PATIENCE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct Report {
    pub encoder: String,
    pub size: (u32, u32),
    pub frames: usize,
    pub keyframes: usize,
    pub bytes: usize,
    pub keyframe_ms: (f64, f64),
    pub decode_ms: (f64, f64),
    pub luma_psnr: f64,
    pub shifted_psnr: f64,
    pub colour_bias: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum Failure {
    #[error(transparent)]
    Media(#[from] Error),
    #[error("there is no screen to share")]
    NoScreen,
    #[error("nothing came for {0:?} while waiting for {1}")]
    Silence(Duration, &'static str),
    #[error("the share ended by itself: {0:?}")]
    Ended(EndReason),
    #[error("the decoder showed a {shown:?} picture of a {sent:?} stream")]
    WrongSize { shown: (u32, u32), sent: (u32, u32) },
}

enum Seen {
    Encoded(EncodedFrame),
    Ended(EndReason),
    Decoded(SharedEvent),
}

#[derive(Default)]
struct Times {
    sum: f64,
    most: f64,
    count: usize,
}

impl Times {
    fn add(&mut self, since: Instant) {
        let ms = since.elapsed().as_secs_f64() * 1e3;
        self.sum += ms;
        self.most = self.most.max(ms);
        self.count += 1;
    }

    fn average_and_most(&self) -> (f64, f64) {
        (self.sum / self.count.max(1) as f64, self.most)
    }
}

pub fn roundtrip(frames: usize, bitrate: u32) -> Result<Report, Failure> {
    let _com = ComThread::enter();
    let device = Device::new()?;
    let (id, size) = list_sources()
        .into_iter()
        .find_map(|source| match source {
            ListedSource::Screen { id, size } => Some((id, size)),
            ListedSource::Window { .. } => None,
        })
        .ok_or(Failure::NoScreen)?;
    let settings = ShareSettings::new(size, FPS, bitrate)?;
    let original = screenshot(&device, &Source::open(id)?, size)?;

    let (seen_tx, seen) = std::sync::mpsc::channel();
    let (frames_tx, ended_tx, decoded_tx) = (seen_tx.clone(), seen_tx.clone(), seen_tx);
    let share = Share::start(
        Source::open(id)?,
        settings,
        Box::new(move |frame| {
            let _ = frames_tx.send(Seen::Encoded(frame));
        }),
        Box::new(move |reason| {
            let _ = ended_tx.send(Seen::Ended(reason));
        }),
    )?;
    let decoder = SharedDecoder::open(
        device.adapter()?,
        Box::new(move |event| {
            let _ = decoded_tx.send(Seen::Decoded(event));
        }),
    )?;

    let mut report = Report {
        encoder: share.encoder().to_owned(),
        size,
        frames: 0,
        keyframes: 0,
        bytes: 0,
        keyframe_ms: (0.0, 0.0),
        decode_ms: (0.0, 0.0),
        luma_psnr: 0.0,
        shifted_psnr: 0.0,
        colour_bias: f64::INFINITY,
    };
    let (mut keyframe_times, mut decode_times) = (Times::default(), Times::default());
    let mut asked = Some(Instant::now());
    share.request_keyframe();
    let mut pushed = None;
    let mut reader = None;
    let mut shown = 0;
    while shown < frames {
        match next(&seen, "a picture")? {
            Seen::Encoded(frame) => {
                report.frames += 1;
                report.bytes += frame.data.len();
                if frame.keyframe {
                    report.keyframes += 1;
                    if let Some(at) = asked.take() {
                        keyframe_times.add(at);
                    }
                }
                pushed = Some(Instant::now());
                decoder.push(frame.data, frame.keyframe);
            }
            Seen::Decoded(SharedEvent::Textures(shared)) => {
                reader = Some(Reader::open(&device, shared)?);
            }
            Seen::Decoded(SharedEvent::Picture) => {
                if let Some(at) = pushed.take() {
                    decode_times.add(at);
                }
                if let Some(reader) = &mut reader {
                    reader.take()?;
                }
                shown += 1;
                if asked.is_none() {
                    asked = Some(Instant::now());
                    share.request_keyframe();
                }
            }
            Seen::Decoded(SharedEvent::NeedKeyframe) => {
                asked.get_or_insert_with(Instant::now);
                share.request_keyframe();
            }
            Seen::Decoded(SharedEvent::Failed(e)) => return Err(e.into()),
            Seen::Ended(reason) => return Err(Failure::Ended(reason)),
        }
    }
    report.keyframe_ms = keyframe_times.average_and_most();
    report.decode_ms = decode_times.average_and_most();

    let reader = present(reader, "the shared textures")?;
    let shown_size = reader.shared.size();
    if shown_size != size {
        return Err(Failure::WrongSize {
            shown: shown_size,
            sent: size,
        });
    }
    let texture = present(
        reader
            .shared
            .held()
            .and_then(|held| reader.textures.get(held)),
        "the picture shown",
    )?;
    let received = read_back(&device, texture, DXGI_FORMAT_R8G8B8A8_UNORM, size)?;
    drop(share);
    let afterwards = screenshot(&device, &Source::open(id)?, size)?;
    let received = rgb_of_rgba(&received);
    let best = [&original, &afterwards]
        .into_iter()
        .map(|screen| Likeness::of(&rgb_of_bgra(screen), &received, size))
        .max_by(|a, b| a.luma_psnr.total_cmp(&b.luma_psnr));
    if let Some(best) = best {
        report.luma_psnr = best.luma_psnr;
        report.shifted_psnr = best.shifted_psnr;
        report.colour_bias = best.colour_bias;
    }
    Ok(report)
}

const MARGIN: usize = 2;
const NEIGHBOURS: [(isize, isize); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

fn rgb_of_bgra(pixels: &[u8]) -> Vec<[f64; 3]> {
    pixels
        .chunks_exact(4)
        .map(|p| [p[2], p[1], p[0]].map(f64::from))
        .collect()
}

fn rgb_of_rgba(pixels: &[u8]) -> Vec<[f64; 3]> {
    pixels
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2]].map(f64::from))
        .collect()
}

fn luma([r, g, b]: [f64; 3]) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

struct Likeness {
    luma_psnr: f64,
    shifted_psnr: f64,
    colour_bias: f64,
}

impl Likeness {
    fn of(sent: &[[f64; 3]], shown: &[[f64; 3]], (width, height): (u32, u32)) -> Self {
        let (width, height) = (width as usize, height as usize);
        let psnr_moved = |(dx, dy): (isize, isize)| {
            let (mut squares, mut count) = (0.0, 0usize);
            for y in MARGIN..height.saturating_sub(MARGIN) {
                for x in MARGIN..width.saturating_sub(MARGIN) {
                    let there = y.wrapping_add_signed(dy) * width + x.wrapping_add_signed(dx);
                    squares += (luma(sent[y * width + x]) - luma(shown[there])).powi(2);
                    count += 1;
                }
            }
            psnr(squares, count)
        };
        let mean = |pixels: &[[f64; 3]], channel: usize| {
            pixels.iter().map(|pixel| pixel[channel]).sum::<f64>() / pixels.len().max(1) as f64
        };
        Self {
            luma_psnr: psnr_moved((0, 0)),
            shifted_psnr: NEIGHBOURS.into_iter().map(psnr_moved).fold(0.0, f64::max),
            colour_bias: (0..3)
                .map(|channel| (mean(sent, channel) - mean(shown, channel)).abs())
                .fold(0.0, f64::max),
        }
    }
}

fn next(seen: &Receiver<Seen>, what: &'static str) -> Result<Seen, Failure> {
    seen.recv_timeout(PATIENCE)
        .map_err(|_| Failure::Silence(PATIENCE, what))
}

fn screenshot(device: &Device, source: &Source, size: (u32, u32)) -> Result<Vec<u8>, Failure> {
    let (arrived_tx, arrived): (Sender<CaptureEvent>, _) = std::sync::mpsc::channel();
    let mut capture = Capture::start(device, source, move |event| {
        let _ = arrived_tx.send(event);
    })?;
    loop {
        match arrived.recv_timeout(PATIENCE) {
            Ok(CaptureEvent::Frame) => {}
            Ok(CaptureEvent::Closed) => return Err(Error::SourceGone.into()),
            Err(_) => return Err(Failure::Silence(PATIENCE, "the screen")),
        }
        if let Some(frame) = capture.latest()? {
            return Ok(read_back(
                device,
                &frame.texture,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                size,
            )?);
        }
    }
}

struct D3d11Fences {
    context: ID3D11DeviceContext4,
    written: ID3D11Fence,
    read: ID3D11Fence,
}

impl ReaderQueue for D3d11Fences {
    fn signal_read(&self, value: u64) -> Result<(), Error> {
        unsafe { self.context.Signal(&self.read, value) }.context("signalling the read fence")
    }

    fn wait_written(&self, value: u64) -> Result<(), Error> {
        unsafe { self.context.Wait(&self.written, value) }.context("waiting for the picture")
    }
}

struct Reader {
    textures: Vec<ID3D11Texture2D>,
    shared: SharedTextures,
    fences: D3d11Fences,
}

impl Reader {
    fn open(device: &Device, shared: SharedTextures) -> Result<Self, Error> {
        let device1: ID3D11Device1 = device.device.cast().context("ID3D11Device1")?;
        let device5: ID3D11Device5 = device.device.cast().context("ID3D11Device5")?;
        let textures = shared
            .textures
            .iter()
            .map(|handle| {
                unsafe { device1.OpenSharedResource1(handle.raw()) }
                    .context("opening a shared texture")
            })
            .collect::<Result<_, _>>()?;
        let fence = |handle: &crate::shared::SharedHandle| -> Result<ID3D11Fence, Error> {
            let mut fence = None;

            unsafe { device5.OpenSharedFence(handle.raw(), &mut fence) }
                .context("opening a shared fence")?;
            present(fence, "the shared fence")
        };
        let fences = D3d11Fences {
            context: device.context4()?,
            written: fence(&shared.written)?,
            read: fence(&shared.read)?,
        };
        Ok(Self {
            textures,
            shared,
            fences,
        })
    }

    fn take(&mut self) -> Result<Option<usize>, Error> {
        self.shared.take(&self.fences)
    }
}

struct Mapped<'a> {
    device: &'a Device,
    staging: &'a ID3D11Texture2D,
    mapped: D3D11_MAPPED_SUBRESOURCE,
}

impl<'a> Mapped<'a> {
    fn map(device: &'a Device, staging: &'a ID3D11Texture2D) -> Result<Self, Error> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();

        unsafe {
            device
                .context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .context("mapping a staging texture")?;
        Ok(Self {
            device,
            staging,
            mapped,
        })
    }

    fn rows(&self, size: (u32, u32)) -> Vec<u8> {
        let row = size.0 as usize * 4;
        let pitch = self.mapped.RowPitch as usize;
        let mut out = Vec::with_capacity(row * size.1 as usize);
        for y in 0..size.1 as usize {
            let start = unsafe { self.mapped.pData.cast::<u8>().add(y * pitch) };

            out.extend_from_slice(unsafe { std::slice::from_raw_parts(start, row) });
        }
        out
    }
}

impl Drop for Mapped<'_> {
    fn drop(&mut self) {
        unsafe { self.device.context.Unmap(self.staging, 0) };
    }
}

fn read_back(
    device: &Device,
    texture: &ID3D11Texture2D,
    format: DXGI_FORMAT,
    size: (u32, u32),
) -> Result<Vec<u8>, Error> {
    let (width, height) = crate::d3d::texture_size(texture);
    if size.0 > width || size.1 > height {
        return Err(Error::Empty("a texture as large as its picture"));
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
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
    .context("creating a staging texture")?;
    let staging: ID3D11Texture2D = present(staging, "the staging texture")?;

    unsafe { device.context.CopyResource(&staging, texture) };
    let mapped = Mapped::map(device, &staging)?;
    if (mapped.mapped.RowPitch as usize) < size.0 as usize * 4 {
        return Err(Error::Empty("a staging texture as wide as its picture"));
    }
    Ok(mapped.rows(size))
}

fn psnr(squares: f64, count: usize) -> f64 {
    let mse = squares / count.max(1) as f64;
    if mse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / mse).log10()
}
