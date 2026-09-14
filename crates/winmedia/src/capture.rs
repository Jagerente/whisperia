use windows::{
    core::{IInspectable, Interface},
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
            GraphicsCaptureSession,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::{
        Graphics::Direct3D11::ID3D11Texture2D,
        System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess,
    },
};

use crate::{d3d::Device, sources::Source, Context, Error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureEvent {
    Frame,
    Closed,
}

pub(crate) struct CapturedFrame {
    pub texture: ID3D11Texture2D,
    pub texture_size: (u32, u32),
    pub content: (u32, u32),
    _frame: Direct3D11CaptureFrame,
}

pub(crate) struct Capture {
    item: GraphicsCaptureItem,
    device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    size: SizeInt32,
    frame_token: i64,
    closed_token: i64,
}

const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
const POOL_FRAMES: i32 = 2;

impl Capture {
    pub fn start(
        device: &Device,
        source: &Source,
        on_event: impl Fn(CaptureEvent) + Clone + Send + 'static,
    ) -> Result<Self, Error> {
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            return Err(Error::CaptureUnsupported);
        }
        let item = source.item.clone();
        let size = item
            .Size()
            .context("asking for the size of what is shared")?;
        let device = device.winrt()?;
        let pool =
            Direct3D11CaptureFramePool::CreateFreeThreaded(&device, FORMAT, POOL_FRAMES, size)
                .context("creating the capture frame pool")?;
        let session = pool
            .CreateCaptureSession(&item)
            .context("creating the capture session")?;

        let arrived = on_event.clone();
        let frame_token = pool
            .FrameArrived(
                &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |_, _| {
                    arrived(CaptureEvent::Frame);
                    Ok(())
                }),
            )
            .context("watching for pictures")?;
        let closed_token = item
            .Closed(
                &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                    on_event(CaptureEvent::Closed);
                    Ok(())
                }),
            )
            .context("watching what is shared")?;
        session.StartCapture().context("starting the capture")?;

        Ok(Self {
            item,
            device,
            pool,
            session,
            size,
            frame_token,
            closed_token,
        })
    }

    pub fn latest(&mut self) -> Result<Option<CapturedFrame>, Error> {
        let mut newest = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            newest = Some(frame);
        }
        let Some(frame) = newest else {
            return Ok(None);
        };

        let content = frame.ContentSize().context("reading the picture size")?;
        let surface = frame.Surface().context("reading the picture")?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().context("reading the picture")?;

        let texture: ID3D11Texture2D =
            unsafe { access.GetInterface() }.context("reading the picture")?;
        let texture_size = crate::d3d::texture_size(&texture);

        if (content.Width != self.size.Width || content.Height != self.size.Height)
            && content.Width > 0
            && content.Height > 0
        {
            self.size = content;
            self.pool
                .Recreate(&self.device, FORMAT, POOL_FRAMES, content)
                .context("resizing the capture frame pool")?;
        }
        Ok(Some(CapturedFrame {
            content: (
                (content.Width.max(1) as u32).min(texture_size.0),
                (content.Height.max(1) as u32).min(texture_size.1),
            ),
            texture_size,
            texture,
            _frame: frame,
        }))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.frame_token);
        let _ = self.item.RemoveClosed(self.closed_token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
