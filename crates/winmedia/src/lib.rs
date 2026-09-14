#![cfg(windows)]

mod capture;
mod convert;
mod d3d;
mod decode;
mod encode;
mod mf;
mod share;
mod shared;
mod sources;
mod wgpu_import;

pub use d3d::AdapterLuid;
pub use share::{EndReason, Share, ShareSettings};
pub use shared::{SharedDecoder, SharedEvent, SharedTextures};
pub use sources::{icon_of, list_sources, preview_of, ListedSource, Preview, Source, SourceId};
pub use wgpu_import::{adapter_luid, ImportedTextures};

#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("screen capture is not available on this version of Windows")]
    CaptureUnsupported,
    #[error("the screen or window is not there any more")]
    SourceGone,
    #[error("this video card has no hardware H.264 encoder")]
    NoEncoder,
    #[error("the H.264 encoder \"{name}\" cannot be used: {source}")]
    EncoderUnusable {
        name: String,
        #[source]
        source: Box<Error>,
    },
    #[error("the H.264 decoder offers no NV12 output")]
    NoNv12Output,
    #[error("the H.264 decoder did not decode on the video card")]
    NotOnVideoCard,
    #[error("the window is not drawn with Direct3D 12")]
    NotDirect3D12,
    #[error("{0} came back empty")]
    Empty(&'static str),
    #[error("a picture of {0} bytes is too large")]
    TooLarge(usize),
    #[error("{width}x{height} at {fps} pictures a second and {bitrate} bit/s cannot be encoded")]
    Unencodable {
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
    },
    #[error("the {0} thread stopped while starting")]
    Stillborn(&'static str),
    #[error("{what}: {source}")]
    Api {
        what: &'static str,
        source: windows::core::Error,
    },
    #[error("cannot start a thread: {0}")]
    Thread(#[from] std::io::Error),
}

pub(crate) trait Context<T> {
    fn context(self, what: &'static str) -> Result<T, Error>;
}

impl<T> Context<T> for windows::core::Result<T> {
    fn context(self, what: &'static str) -> Result<T, Error> {
        self.map_err(|source| Error::Api { what, source })
    }
}

pub(crate) fn present<T>(value: Option<T>, what: &'static str) -> Result<T, Error> {
    value.ok_or(Error::Empty(what))
}
