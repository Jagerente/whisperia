use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use bytes::Bytes;
use slint::wgpu_30::wgpu;
use whisperia_proto::PeerId;
use whisperia_winmedia::{
    AdapterLuid, ImportedTextures, SharedDecoder, SharedEvent, SharedTextures,
};

use crate::screen::{
    ListedSource, Preview, ScreenBackend, ScreenError, ShareHandle, ShareOut, SourceKey,
    ViewerEvent, ViewerEvents, ViewerHandle,
};

pub(super) type OnPicture = Box<dyn Fn(PeerId, slint::Image)>;

struct Share {
    generation: u64,
    imported: ImportedTextures,
    images: Vec<slint::Image>,
}

struct Shown {
    device: wgpu::Device,
    shares: HashMap<PeerId, Share>,
    on_picture: OnPicture,
}

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

pub(super) fn show_with(device: wgpu::Device, on_picture: OnPicture) {
    SHOWN.set(Some(Shown {
        device,
        shares: HashMap::new(),
        on_picture,
    }));
}

pub(super) fn current(sharer: PeerId) -> Option<slint::Image> {
    SHOWN.with_borrow(|shown| {
        let share = shown.as_ref()?.shares.get(&sharer)?;
        share.images.get(share.imported.held()?).cloned()
    })
}

fn on_window_thread(job: impl FnOnce(&mut Shown) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        SHOWN.with_borrow_mut(|shown| {
            if let Some(shown) = shown {
                job(shown);
            }
        });
    });
}

fn textures_arrived(
    shown: &mut Shown,
    sharer: PeerId,
    generation: u64,
    shared: SharedTextures,
) -> Result<(), ScreenError> {
    let imported = ImportedTextures::open(&shown.device, shared)?;
    let images = imported
        .textures()
        .iter()
        .map(|texture| slint::Image::try_from(texture.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    shown.shares.insert(
        sharer,
        Share {
            generation,
            imported,
            images,
        },
    );
    Ok(())
}

fn picture_arrived(shown: &mut Shown, sharer: PeerId, generation: u64) -> Result<(), ScreenError> {
    let Some(share) = shown
        .shares
        .get_mut(&sharer)
        .filter(|share| share.generation == generation)
    else {
        return Ok(());
    };
    let taken = share.imported.take();
    let index = match taken {
        Ok(Some(index)) => index,
        Ok(None) => return Ok(()),
        Err(e) => {
            shown.shares.remove(&sharer);
            return Err(e.into());
        }
    };
    if let Some(image) = share.images.get(index) {
        (shown.on_picture)(sharer, image.clone());
    }
    Ok(())
}

fn forget(shown: &mut Shown, sharer: PeerId, generation: u64) {
    if shown
        .shares
        .get(&sharer)
        .is_some_and(|share| share.generation == generation)
    {
        shown.shares.remove(&sharer);
    }
}

struct Viewer {
    sharer: PeerId,
    generation: u64,
    decoder: SharedDecoder,
}

impl ViewerHandle for Viewer {
    fn push(&self, data: Bytes, keyframe: bool) {
        self.decoder.push(data, keyframe);
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.decoder.stop();
        let (sharer, generation) = (self.sharer, self.generation);
        on_window_thread(move |shown| forget(shown, sharer, generation));
    }
}

pub(super) struct InWindow {
    pub inner: Arc<dyn ScreenBackend>,
    pub adapter_luid: AdapterLuid,
}

impl ScreenBackend for InWindow {
    fn list(&self) -> Result<Vec<ListedSource>, ScreenError> {
        self.inner.list()
    }

    fn preview(&self, key: SourceKey) -> Option<Preview> {
        self.inner.preview(key)
    }

    fn icon(&self, key: SourceKey) -> Option<Preview> {
        self.inner.icon(key)
    }

    fn share(
        &self,
        source: SourceKey,
        settings: &crate::config::ShareConfig,
        out: ShareOut,
    ) -> Result<Box<dyn ShareHandle>, ScreenError> {
        self.inner.share(source, settings, out)
    }

    fn view(
        &self,
        sharer: PeerId,
        events: ViewerEvents,
    ) -> Result<Arc<dyn ViewerHandle>, ScreenError> {
        static GENERATIONS: AtomicU64 = AtomicU64::new(0);
        let generation = GENERATIONS.fetch_add(1, Ordering::Relaxed);
        let decoder = SharedDecoder::open(
            self.adapter_luid,
            Box::new(move |event| match event {
                SharedEvent::Textures(shared) => {
                    let events = events.clone();
                    on_window_thread(move |shown| {
                        if let Err(e) = textures_arrived(shown, sharer, generation, shared) {
                            let _ = events.send(ViewerEvent::Failed(e));
                        }
                    });
                }
                SharedEvent::Picture => {
                    let events = events.clone();
                    on_window_thread(move |shown| {
                        if let Err(e) = picture_arrived(shown, sharer, generation) {
                            let _ = events.send(ViewerEvent::Failed(e));
                        }
                    });
                }
                SharedEvent::NeedKeyframe => {
                    let _ = events.send(ViewerEvent::NeedKeyframe);
                }
                SharedEvent::Failed(e) => {
                    let _ = events.send(ViewerEvent::Failed(e.into()));
                }
            }),
        )?;
        Ok(Arc::new(Viewer {
            sharer,
            generation,
            decoder,
        }))
    }
}
