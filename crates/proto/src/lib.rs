pub mod audio;
pub mod control;
mod text;
pub mod video;

pub const PROTOCOL_VERSION: u16 = 1;

pub const ALPN: &[u8] = b"whisperia/1";
pub const ALPN_MEDIA: &[u8] = b"whisperia-media/1";
pub const ALPN_PEEK: &[u8] = b"whisperia-peek/1";

pub const TLS_SERVER_NAME: &str = "whisperia.local";

pub const DEFAULT_PORT: u16 = 47820;

pub const MDNS_SERVICE_TYPE: &str = "_whisperia._udp.local.";
pub const MDNS_TXT_NAME: &str = "name";

pub const MDNS_TXT_ADDRS: &str = "addrs";
pub const MDNS_TXT_ROOMS: &str = "rooms";
pub const MDNS_TXT_PEERS: &str = "peers";
pub const MDNS_TXT_PROTOCOL: &str = "proto";
pub const MDNS_TXT_ID: &str = "id";

pub const SAMPLE_RATE: u32 = 48_000;
pub const FRAME_MS: u32 = 10;
pub const FRAME_SAMPLES: usize = (SAMPLE_RATE / 1000 * FRAME_MS) as usize;

pub type PeerId = u16;
