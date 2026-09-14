use std::{
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        mpsc::{Receiver, Sender},
        Arc,
    },
    thread::JoinHandle,
};

use windows::{
    core::Interface,
    Win32::{
        Foundation::{CloseHandle, GENERIC_ALL, HANDLE},
        Graphics::{
            Direct3D11::{
                ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
                D3D11_BIND_SHADER_RESOURCE, D3D11_RESOURCE_MISC_SHARED,
                D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
            },
            Dxgi::{Common::DXGI_FORMAT_R8G8B8A8_UNORM, IDXGIResource1},
        },
    },
};

use crate::{
    convert::{rect, Converter, Direction},
    d3d::{AdapterLuid, Device},
    decode::{Decoded, DecodedFrame, Decoder},
    mf::ComThread,
    Context, Error,
};

pub(crate) const TEXTURES: usize = 3;
const READER_STARTS_WITH: usize = 2;
const WRITER_STARTS_WITH: usize = 0;
const MIDDLE_STARTS_AS: u8 = 1;
const FRESH: u8 = 0x80;

#[derive(Debug)]
pub(crate) struct SharedHandle(HANDLE);

unsafe impl Send for SharedHandle {}

unsafe impl Sync for SharedHandle {}

impl SharedHandle {
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for SharedHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

#[derive(Debug)]
struct Slots {
    middle: AtomicU8,
    written: [AtomicU64; TEXTURES],
    released: [AtomicU64; TEXTURES],
}

impl Slots {
    fn new() -> Self {
        Self {
            middle: AtomicU8::new(MIDDLE_STARTS_AS),
            written: Default::default(),
            released: Default::default(),
        }
    }

    fn publish(&self, back: usize, written: u64) -> (usize, u64) {
        self.written[back].store(written, Ordering::Release);
        let next = usize::from(self.middle.swap(back as u8 | FRESH, Ordering::AcqRel) & !FRESH);
        (next, self.released[next].load(Ordering::Acquire))
    }

    fn has_new(&self) -> bool {
        self.middle.load(Ordering::Acquire) & FRESH != 0
    }

    fn take(&self, holding: usize, released: u64) -> (usize, u64) {
        self.released[holding].store(released, Ordering::Release);
        let index = usize::from(self.middle.swap(holding as u8, Ordering::AcqRel) & !FRESH);
        (index, self.written[index].load(Ordering::Acquire))
    }
}

pub(crate) trait ReaderQueue {
    fn signal_read(&self, value: u64) -> Result<(), Error>;

    fn wait_written(&self, value: u64) -> Result<(), Error>;
}

#[derive(Debug)]
pub struct SharedTextures {
    pub(crate) textures: [SharedHandle; TEXTURES],
    pub(crate) written: SharedHandle,
    pub(crate) read: SharedHandle,
    size: (u32, u32),
    slots: Arc<Slots>,
    holding: usize,
    taken: bool,
    read_value: u64,
}

impl SharedTextures {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn held(&self) -> Option<usize> {
        self.taken.then_some(self.holding)
    }

    pub(crate) fn take(&mut self, queue: &impl ReaderQueue) -> Result<Option<usize>, Error> {
        if !self.slots.has_new() {
            return Ok(None);
        }
        let released = self.read_value + 1;
        queue.signal_read(released)?;
        self.read_value = released;
        let (index, written) = self.slots.take(self.holding, released);
        self.holding = index;
        self.taken = false;
        queue.wait_written(written)?;
        self.taken = true;
        Ok(Some(index))
    }
}

pub enum SharedEvent {
    Textures(SharedTextures),
    Picture,
    NeedKeyframe,
    Failed(Error),
}

type Picture = (Box<dyn AsRef<[u8]> + Send>, bool);

pub struct SharedDecoder {
    input: Option<Sender<Picture>>,
    thread: Option<JoinHandle<()>>,
}

impl SharedDecoder {
    pub fn open(
        adapter: AdapterLuid,
        mut on_event: Box<dyn FnMut(SharedEvent) + Send>,
    ) -> Result<Self, Error> {
        let (input, pictures) = std::sync::mpsc::channel();
        let (ready_tx, ready) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("ww-decoder".into())
            .spawn(move || {
                let _com = ComThread::enter();
                let mut writer = match Writer::new(adapter) {
                    Ok(writer) => writer,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                if let Err(e) = writer.run(&pictures, &mut *on_event) {
                    tracing::warn!("decoder: {e}");
                    on_event(SharedEvent::Failed(e));
                }
            })?;
        ready.recv().map_err(|_| Error::Stillborn("decoder"))??;
        Ok(Self {
            input: Some(input),
            thread: Some(thread),
        })
    }

    pub fn push(&self, data: impl AsRef<[u8]> + Send + 'static, keyframe: bool) {
        if let Some(input) = &self.input {
            let _ = input.send((Box::new(data), keyframe));
        }
    }

    pub fn stop(&mut self) {
        self.input = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for SharedDecoder {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Ring {
    size: (u32, u32),
    slots: Arc<Slots>,
    back: usize,
    back_released: u64,
    written: ID3D11Fence,
    written_value: u64,
    read: ID3D11Fence,
    converter: Converter,
}

struct Writer {
    device: Device,
    context: ID3D11DeviceContext4,
    decoder: Decoder,
    ring: Option<Ring>,
}

impl Writer {
    fn new(adapter: AdapterLuid) -> Result<Self, Error> {
        let device = Device::on_adapter(adapter)?;
        Ok(Self {
            decoder: Decoder::new(&device)?,
            context: device.context4()?,
            device,
            ring: None,
        })
    }

    fn run(
        &mut self,
        pictures: &Receiver<Picture>,
        on_event: &mut dyn FnMut(SharedEvent),
    ) -> Result<(), Error> {
        while let Ok((data, keyframe)) = pictures.recv() {
            match self.decoder.newest((*data).as_ref(), keyframe) {
                Decoded::Picture(picture) => self.show(&picture, on_event)?,
                Decoded::Nothing => {}
                Decoded::NeedKeyframe(e) => {
                    tracing::debug!("decoder: {e}");
                    on_event(SharedEvent::NeedKeyframe);
                }
            }
        }
        Ok(())
    }

    fn show(
        &mut self,
        picture: &DecodedFrame,
        on_event: &mut dyn FnMut(SharedEvent),
    ) -> Result<(), Error> {
        let size = (picture.width, picture.height);
        let ring = match self.ring.take() {
            Some(ring) if ring.size == size => ring,
            _ => {
                let (ring, shared) = self.new_ring(size)?;
                on_event(SharedEvent::Textures(shared));
                ring
            }
        };
        let ring = self.ring.insert(ring);

        unsafe { self.context.Wait(&ring.read, ring.back_released) }
            .context("waiting until the window is done with a picture")?;
        ring.converter.blit(
            &self.device,
            &picture.texture,
            picture.slice,
            rect(size.0, size.1),
            ring.back,
            rect(size.0, size.1),
        )?;
        ring.written_value += 1;

        unsafe { self.context.Signal(&ring.written, ring.written_value) }
            .context("signalling the fence")?;

        unsafe { self.context.Flush() };
        (ring.back, ring.back_released) = ring.slots.publish(ring.back, ring.written_value);
        on_event(SharedEvent::Picture);
        Ok(())
    }

    fn new_ring(&self, size: (u32, u32)) -> Result<(Ring, SharedTextures), Error> {
        let texture = || {
            self.device.texture(
                size,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE,
                D3D11_RESOURCE_MISC_SHARED | D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
            )
        };
        let textures = [texture()?, texture()?, texture()?];
        let handles = [
            share_texture(&textures[0])?,
            share_texture(&textures[1])?,
            share_texture(&textures[2])?,
        ];
        let written = self.device.shared_fence()?;
        let read = self.device.shared_fence()?;
        let slots = Arc::new(Slots::new());
        let shared = SharedTextures {
            textures: handles,
            written: share_fence(&written)?,
            read: share_fence(&read)?,
            size,
            slots: slots.clone(),
            holding: READER_STARTS_WITH,
            taken: false,
            read_value: 0,
        };
        let converter = Converter::new(
            &self.device,
            size,
            size,
            None,
            Direction::YuvToRgb,
            &textures,
        )?;
        let ring = Ring {
            size,
            slots,
            back: WRITER_STARTS_WITH,
            back_released: 0,
            written,
            written_value: 0,
            read,
            converter,
        };
        Ok((ring, shared))
    }
}

fn share_texture(texture: &ID3D11Texture2D) -> Result<SharedHandle, Error> {
    let resource: IDXGIResource1 = texture.cast().context("IDXGIResource1")?;

    unsafe { resource.CreateSharedHandle(None, GENERIC_ALL.0, None) }
        .map(SharedHandle)
        .context("sharing a texture")
}

fn share_fence(fence: &ID3D11Fence) -> Result<SharedHandle, Error> {
    unsafe { fence.CreateSharedHandle(None, GENERIC_ALL.0, None) }
        .map(SharedHandle)
        .context("sharing a fence")
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct Queue {
        signalled: RefCell<Vec<u64>>,
        waited: RefCell<Vec<u64>>,
        broken: bool,
    }

    impl ReaderQueue for Queue {
        fn signal_read(&self, value: u64) -> Result<(), Error> {
            if self.broken {
                return Err(Error::Empty("the queue"));
            }
            self.signalled.borrow_mut().push(value);
            Ok(())
        }

        fn wait_written(&self, value: u64) -> Result<(), Error> {
            self.waited.borrow_mut().push(value);
            Ok(())
        }
    }

    fn reader(slots: &Arc<Slots>) -> SharedTextures {
        let none = || SharedHandle(HANDLE::default());
        SharedTextures {
            textures: [none(), none(), none()],
            written: none(),
            read: none(),
            size: (2, 2),
            slots: slots.clone(),
            holding: READER_STARTS_WITH,
            taken: false,
            read_value: 0,
        }
    }

    #[test]
    fn the_reader_gets_the_newest_picture_and_never_the_texture_being_written() {
        let slots = Arc::new(Slots::new());
        let mut shared = reader(&slots);
        let queue = Queue::default();
        let mut back = WRITER_STARTS_WITH;
        let mut publish = |written: u64| {
            back = slots.publish(back, written).0;
            back
        };

        assert_eq!(shared.take(&queue).unwrap(), None);
        assert_eq!(shared.held(), None);
        publish(1);
        let back = publish(2);
        let taken = shared.take(&queue).unwrap().unwrap();
        assert_ne!(taken, back);
        assert_eq!(shared.held(), Some(taken));
        assert_eq!(*queue.waited.borrow(), [2]);
        assert_eq!(shared.take(&queue).unwrap(), None);
        for written in 3..40 {
            let back = publish(written);
            if written % 3 == 0 {
                let taken = shared.take(&queue).unwrap().unwrap();
                assert_ne!(taken, back);
                assert_eq!(queue.waited.borrow().last(), Some(&written));
            }
        }
    }

    #[test]
    fn the_writer_waits_for_what_the_reader_signalled_when_it_gave_the_texture_back() {
        let slots = Arc::new(Slots::new());
        let mut shared = reader(&slots);
        let queue = Queue::default();
        let (mut back, mut back_released) = (WRITER_STARTS_WITH, 0);
        let mut released_with = [0u64; TEXTURES];
        for written in 1..60 {
            assert!(
                back_released >= released_with[back],
                "texture {back} is written before the window is done with it"
            );
            (back, back_released) = slots.publish(back, written);
            if written % 2 == 0 {
                let before = shared.holding;
                shared.take(&queue).unwrap();
                released_with[before] = *queue.signalled.borrow().last().unwrap();
            }
        }
        assert!(queue.signalled.borrow().windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_failed_signal_keeps_the_picture_that_is_shown() {
        let slots = Arc::new(Slots::new());
        let mut shared = reader(&slots);
        slots.publish(WRITER_STARTS_WITH, 1);
        let broken = Queue {
            broken: true,
            ..Queue::default()
        };
        assert!(shared.take(&broken).is_err());
        assert_eq!(shared.holding, READER_STARTS_WITH);
        assert!(slots.has_new());
        assert!(shared.take(&Queue::default()).unwrap().is_some());
    }
}
