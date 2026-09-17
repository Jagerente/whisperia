use std::sync::{Arc, Mutex};

use whisperia_proto::PeerId;
use whisperia_winmedia as winmedia;

use super::{
    policy,
    sound::{AppSound, SoundTarget},
    ListedSource, Preview, ScreenBackend, ScreenError, ShareEnd, ShareHandle, ShareOut, SourceKey,
    SourceKind, SourceNews, ViewerEvents, ViewerHandle,
};
use crate::{config::ShareConfig, net::OutgoingPicture};

pub struct WinScreen;

fn source_id(key: SourceKey) -> winmedia::SourceId {
    let handle = key.handle as isize;
    match key.kind {
        SourceKind::Screen => winmedia::SourceId::Screen { handle },
        SourceKind::Window => winmedia::SourceId::Window {
            handle,
            process: key.process,
        },
    }
}

fn key_of(id: winmedia::SourceId) -> SourceKey {
    match id {
        winmedia::SourceId::Screen { handle } => SourceKey {
            kind: SourceKind::Screen,
            handle: handle as u64,
            process: 0,
        },
        winmedia::SourceId::Window { handle, process } => SourceKey {
            kind: SourceKind::Window,
            handle: handle as u64,
            process,
        },
    }
}

fn picture(preview: winmedia::Preview) -> Preview {
    Preview {
        width: preview.width,
        height: preview.height,
        rgba: preview.rgba,
    }
}

fn settings(stream: policy::Stream) -> Result<winmedia::ShareSettings, ScreenError> {
    Ok(winmedia::ShareSettings::new(
        stream.size,
        stream.fps,
        stream.bitrate,
    )?)
}

struct Share {
    _watch: Option<winmedia::WindowWatch>,
    share: winmedia::Share,
    wanted: Mutex<policy::Wanted>,
    sound: Option<AppSound>,
}

impl Share {
    fn change(&self, change: impl FnOnce(&mut policy::Wanted) -> Option<policy::Stream>) {
        let mut wanted = self.wanted.lock().unwrap_or_else(|p| p.into_inner());
        let Some(stream) = change(&mut wanted) else {
            return;
        };
        match settings(stream) {
            Ok(settings) => self.share.reconfigure(settings),
            Err(e) => tracing::warn!("the share keeps its size and quality: {e}"),
        }
    }
}

impl ShareHandle for Share {
    fn request_keyframe(&self) {
        self.share.request_keyframe();
    }

    fn reconfigure(&self, config: &ShareConfig) {
        self.change(|wanted| wanted.reconfigure(config));
    }

    fn follow(&self, source: (u32, u32)) {
        self.change(|wanted| wanted.follow(source));
    }

    fn stream(&self) -> policy::Stream {
        self.wanted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stream()
    }

    fn has_sound(&self) -> bool {
        self.sound.is_some()
    }

    fn details(&self) -> String {
        self.share.encoder().to_owned()
    }
}

impl ScreenBackend for WinScreen {
    fn list(&self) -> Result<Vec<ListedSource>, ScreenError> {
        let mut screens = 0;
        Ok(winmedia::list_sources()
            .into_iter()
            .map(|source| match source {
                winmedia::ListedSource::Screen { id, size } => {
                    screens += 1;
                    ListedSource {
                        key: key_of(id),
                        label: policy::screen_label(screens - 1),
                        detail: policy::screen_detail(size),
                        minimized: false,
                    }
                }
                winmedia::ListedSource::Window {
                    id,
                    title,
                    program,
                    minimized,
                } => ListedSource {
                    key: key_of(id),
                    label: title,
                    detail: program.unwrap_or_default(),
                    minimized,
                },
            })
            .collect())
    }

    fn preview(&self, key: SourceKey) -> Option<Preview> {
        winmedia::preview_of(source_id(key)).map(picture)
    }

    fn icon(&self, key: SourceKey) -> Option<Preview> {
        winmedia::icon_of(source_id(key)).map(picture)
    }

    fn share(
        &self,
        source: SourceKey,
        config: &ShareConfig,
        out: ShareOut,
    ) -> Result<Box<dyn ShareHandle>, ScreenError> {
        let id = source_id(source);
        let source = winmedia::Source::open(id)?;
        let wanted = policy::Wanted::new(source.size()?, config, winmedia::ShareSettings::LARGEST);
        let target = match source.process_id() {
            Some(process) => SoundTarget::Process(process),
            None => SoundTarget::EverythingBut(std::process::id()),
        };
        let ShareOut {
            mut pictures,
            sound,
            ended,
            mut news,
        } = out;
        let share = winmedia::Share::start(
            source,
            settings(wanted.stream())?,
            Box::new(move |frame| {
                pictures(OutgoingPicture {
                    data: frame.data,
                    keyframe: frame.keyframe,
                    width: frame.width.min(u32::from(u16::MAX)) as u16,
                    height: frame.height.min(u32::from(u16::MAX)) as u16,
                })
            }),
            Box::new(move |reason| {
                ended(match reason {
                    winmedia::EndReason::SourceClosed => ShareEnd::SourceClosed,
                    winmedia::EndReason::Failed(e) => ShareEnd::Failed(e.into()),
                })
            }),
        )?;
        let sound = sound.and_then(|sound| {
            AppSound::start(target, sound)
                .map_err(|e| tracing::warn!("sharing without sound: {e}"))
                .ok()
        });
        let watch = winmedia::WindowWatch::start(
            id,
            Box::new(move |heard| {
                news(match heard {
                    winmedia::WindowNews::Resized(size) => SourceNews::Resized(size),
                    winmedia::WindowNews::Minimized => SourceNews::Minimized,
                    winmedia::WindowNews::Restored => SourceNews::Restored,
                })
            }),
        )
        .map_err(|e| tracing::warn!("the share does not follow the window: {e}"))
        .ok()
        .flatten();
        Ok(Box::new(Share {
            _watch: watch,
            share,
            wanted: Mutex::new(wanted),
            sound,
        }))
    }

    fn view(&self, _: PeerId, _: ViewerEvents) -> Result<Arc<dyn ViewerHandle>, ScreenError> {
        Err(ScreenError::NeedsDirect3D12)
    }
}
