use windows::{
    core::Interface,
    Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12Resource},
};

use crate::{
    d3d::AdapterLuid,
    present,
    shared::{ReaderQueue, SharedHandle, SharedTextures},
    Context, Error,
};

pub fn adapter_luid(device: &wgpu::Device) -> Result<AdapterLuid, Error> {
    let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.ok_or(Error::NotDirect3D12)?;

    let luid = unsafe { hal.raw_device().GetAdapterLuid() };
    Ok(AdapterLuid::of(luid))
}

struct Fences {
    queue: ID3D12CommandQueue,
    written: ID3D12Fence,
    read: ID3D12Fence,
}

impl ReaderQueue for Fences {
    fn signal_read(&self, value: u64) -> Result<(), Error> {
        unsafe { self.queue.Signal(&self.read, value) }
            .context("telling the decoder a picture is done")
    }

    fn wait_written(&self, value: u64) -> Result<(), Error> {
        unsafe { self.queue.Wait(&self.written, value) }.context("waiting for the picture")
    }
}

pub struct ImportedTextures {
    textures: Vec<wgpu::Texture>,
    shared: SharedTextures,
    fences: Fences,
}

fn open<T: Interface>(
    device: &ID3D12Device,
    handle: &SharedHandle,
    what: &'static str,
) -> Result<T, Error> {
    let mut opened: Option<T> = None;

    unsafe { device.OpenSharedHandle(handle.raw(), &mut opened) }.context(what)?;
    present(opened, what)
}

impl ImportedTextures {
    pub fn open(device: &wgpu::Device, shared: SharedTextures) -> Result<Self, Error> {
        let (width, height) = shared.size();
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let desc = wgpu::TextureDescriptor {
            label: Some("shared picture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        };

        let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.ok_or(Error::NotDirect3D12)?;
        let raw = hal.raw_device();
        let mut textures = Vec::with_capacity(shared.textures.len());
        for handle in &shared.textures {
            let resource: ID3D12Resource = open(raw, handle, "opening a shared texture")?;

            let texture = unsafe {
                wgpu::hal::dx12::Device::texture_from_raw(
                    resource,
                    desc.format,
                    desc.dimension,
                    size,
                    desc.mip_level_count,
                    desc.sample_count,
                )
            };

            textures.push(unsafe {
                device.create_texture_from_hal::<wgpu::hal::api::Dx12>(
                    texture,
                    &desc,
                    wgpu::TextureUses::UNINITIALIZED,
                )
            });
        }
        let fences = Fences {
            written: open(raw, &shared.written, "opening the decoder's fence")?,
            read: open(raw, &shared.read, "opening the window's fence")?,
            queue: hal.raw_queue().clone(),
        };
        Ok(Self {
            textures,
            shared,
            fences,
        })
    }

    pub fn textures(&self) -> &[wgpu::Texture] {
        &self.textures
    }

    pub fn take(&mut self) -> Result<Option<usize>, Error> {
        self.shared.take(&self.fences)
    }

    pub fn held(&self) -> Option<usize> {
        self.shared.held()
    }
}
