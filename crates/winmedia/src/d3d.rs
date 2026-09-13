use windows::{
    core::Interface,
    Graphics::DirectX::Direct3D11::IDirect3DDevice,
    Win32::{
        Foundation::{HMODULE, LUID},
        Graphics::{
            Direct3D::{
                D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_11_1,
            },
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11Device5, ID3D11DeviceContext,
                ID3D11DeviceContext4, ID3D11Fence, ID3D11Multithread, ID3D11Texture2D,
                ID3D11VideoContext1, ID3D11VideoDevice, D3D11_BIND_FLAG,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                D3D11_FENCE_FLAG_SHARED, D3D11_RESOURCE_MISC_FLAG, D3D11_SDK_VERSION,
                D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
            },
            Dxgi::{
                Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC},
                CreateDXGIFactory1, IDXGIAdapter, IDXGIDevice, IDXGIFactory4,
            },
        },
        System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice,
    },
};

use crate::{present, Context, Error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterLuid {
    pub(crate) low: u32,
    pub(crate) high: i32,
}

impl AdapterLuid {
    pub(crate) fn of(luid: LUID) -> Self {
        Self {
            low: luid.LowPart,
            high: luid.HighPart,
        }
    }

    pub(crate) fn luid(self) -> LUID {
        LUID {
            LowPart: self.low,
            HighPart: self.high,
        }
    }
}

pub(crate) struct Device {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub video: ID3D11VideoDevice,
    pub video_context: ID3D11VideoContext1,
}

impl Device {
    pub fn new() -> Result<Self, Error> {
        Self::create(None)
    }

    pub fn on_adapter(adapter: AdapterLuid) -> Result<Self, Error> {
        let factory: IDXGIFactory4 =
            unsafe { CreateDXGIFactory1() }.context("finding the video card")?;
        let adapter: IDXGIAdapter = unsafe { factory.EnumAdapterByLuid(adapter.luid()) }
            .context("finding the video card to share pictures with")?;
        Self::create(Some(&adapter))
    }

    fn create(adapter: Option<&IDXGIAdapter>) -> Result<Self, Error> {
        let mut device = None;
        let mut context = None;
        let driver = match adapter {
            Some(_) => D3D_DRIVER_TYPE_UNKNOWN,
            None => D3D_DRIVER_TYPE_HARDWARE,
        };

        unsafe {
            D3D11CreateDevice(
                adapter,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .context("creating the Direct3D device")?;
        let device: ID3D11Device = present(device, "the Direct3D device")?;
        let context: ID3D11DeviceContext = present(context, "the Direct3D context")?;

        let multithread: ID3D11Multithread = context.cast().context("ID3D11Multithread")?;

        let _ = unsafe { multithread.SetMultithreadProtected(true) };

        Ok(Self {
            video: device.cast().context("ID3D11VideoDevice")?,
            video_context: context.cast().context("ID3D11VideoContext1")?,
            device,
            context,
        })
    }

    pub fn adapter(&self) -> Result<AdapterLuid, Error> {
        let dxgi: IDXGIDevice = self.device.cast().context("IDXGIDevice")?;

        let adapter = unsafe { dxgi.GetAdapter() }.context("asking for the video card")?;

        let desc = unsafe { adapter.GetDesc() }.context("describing the video card")?;
        Ok(AdapterLuid::of(desc.AdapterLuid))
    }

    pub fn winrt(&self) -> Result<IDirect3DDevice, Error> {
        let dxgi: IDXGIDevice = self.device.cast().context("IDXGIDevice")?;

        unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .and_then(|inspectable| inspectable.cast())
            .context("wrapping the Direct3D device for WinRT")
    }

    pub fn context4(&self) -> Result<ID3D11DeviceContext4, Error> {
        self.context.cast().context("ID3D11DeviceContext4")
    }

    pub fn shared_fence(&self) -> Result<ID3D11Fence, Error> {
        let device: ID3D11Device5 = self.device.cast().context("ID3D11Device5")?;
        let mut fence: Option<ID3D11Fence> = None;

        unsafe { device.CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence) }
            .context("creating a fence")?;
        present(fence, "the fence")
    }

    pub fn texture(
        &self,
        size: (u32, u32),
        format: DXGI_FORMAT,
        bind: D3D11_BIND_FLAG,
        misc: D3D11_RESOURCE_MISC_FLAG,
    ) -> Result<ID3D11Texture2D, Error> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.0,
            Height: size.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: misc.0 as u32,
        };
        let mut texture = None;

        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut texture)) }
            .context("creating a texture")?;
        present(texture, "the texture")
    }
}

pub(crate) fn texture_size(texture: &ID3D11Texture2D) -> (u32, u32) {
    let mut desc = D3D11_TEXTURE2D_DESC::default();

    unsafe { texture.GetDesc(&mut desc) };
    (desc.Width, desc.Height)
}
