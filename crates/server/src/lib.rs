mod advertise;
pub mod cert;
mod conn;
mod hub;
mod media;

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use quinn::crypto::rustls::QuicServerConfig;
use tracing::{info, warn};
use whisperia_proto::{
    control::{Name, Password, MAX_PEERS},
    video::MAX_PICTURE,
    ALPN, ALPN_MEDIA, ALPN_PEEK,
};

use crate::{
    advertise::{Advertiser, Announcer},
    cert::Identity,
    hub::{Hub, Occupancy},
};

const IDLE_TIMEOUT: Duration = Duration::from_secs(8);
const KEEP_ALIVE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub name: Name,
    pub bind: SocketAddr,
    pub password: Option<Password>,
    pub data_dir: PathBuf,
    pub max_peers: usize,
    pub advertise: bool,
    pub advertise_addrs: Vec<Ipv4Addr>,
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error(transparent)]
    Identity(#[from] cert::CertError),
    #[error("TLS setup failed: {0}")]
    Tls(String),
    #[error("cannot listen on {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
}

pub(crate) struct Shared {
    hub: Mutex<Hub<quinn::Connection>>,
    announcer: Option<Announcer>,
    pub name: Name,
    pub password: Option<Password>,
}

impl Shared {
    pub fn change<R>(&self, act: impl FnOnce(&mut Hub<quinn::Connection>) -> R) -> R {
        let mut hub = self.hub.lock().unwrap_or_else(|p| p.into_inner());
        let result = act(&mut hub);
        if let (Some(occupancy), Some(announcer)) = (hub.occupancy_change(), &self.announcer) {
            announcer.announce(occupancy);
        }
        result
    }

    pub fn read<R>(&self, look: impl FnOnce(&Hub<quinn::Connection>) -> R) -> R {
        look(&self.hub.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

pub struct Server {
    endpoint: quinn::Endpoint,
    fingerprint: [u8; 32],
    shared: Arc<Shared>,
    _advertiser: Option<Advertiser>,
}

impl Server {
    pub fn start(config: ServerConfig) -> Result<Self, ServerError> {
        let identity = Identity::load_or_create(&config.data_dir)?;
        let fingerprint = identity.fingerprint();

        let endpoint =
            quinn::Endpoint::server(quic_config(identity)?, config.bind).map_err(|source| {
                ServerError::Bind {
                    addr: config.bind,
                    source,
                }
            })?;
        let port = endpoint
            .local_addr()
            .map(|a| a.port())
            .unwrap_or(config.bind.port());

        let advertiser = if config.advertise {
            match Advertiser::start(&config.name, &fingerprint, port, &config.advertise_addrs) {
                Ok(a) => Some(a),
                Err(e) => {
                    warn!("mDNS announcement failed, clients must enter the address by hand: {e}");
                    None
                }
            }
        } else {
            None
        };
        let shared = Arc::new(Shared {
            hub: Mutex::new(Hub::new(config.max_peers.clamp(1, MAX_PEERS))),
            announcer: advertiser.as_ref().map(Advertiser::announcer),
            name: config.name.clone(),
            password: config.password,
        });

        let accepting = endpoint.clone();
        let serving = shared.clone();
        tokio::spawn(async move {
            while let Some(incoming) = accepting.accept().await {
                tokio::spawn(conn::serve(incoming, serving.clone()));
            }
        });

        info!(name = %config.name, port, "server started");
        Ok(Self {
            endpoint,
            fingerprint,
            shared,
            _advertiser: advertiser,
        })
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.endpoint.local_addr().ok()
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    pub fn occupancy(&self) -> (usize, usize) {
        let Occupancy { rooms, people } = self.shared.read(Hub::occupancy);
        (rooms, people)
    }

    pub async fn shutdown(self) {
        self.endpoint.close(0u32.into(), b"server shutting down");
        let _ = tokio::time::timeout(Duration::from_secs(2), self.endpoint.wait_idle()).await;
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"server shutting down");
    }
}

fn quic_config(identity: Identity) -> Result<quinn::ServerConfig, ServerError> {
    let tls_err = |e: &dyn std::fmt::Display| ServerError::Tls(e.to_string());

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| tls_err(&e))?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert], identity.key)
        .map_err(|e| tls_err(&e))?;
    tls.alpn_protocols = vec![ALPN.to_vec(), ALPN_MEDIA.to_vec(), ALPN_PEEK.to_vec()];

    let mut config = quinn::ServerConfig::with_crypto(Arc::new(
        QuicServerConfig::try_from(tls).map_err(|e| tls_err(&e))?,
    ));
    config.transport_config(Arc::new(transport_config()?));
    Ok(config)
}

fn transport_config() -> Result<quinn::TransportConfig, ServerError> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(KEEP_ALIVE));
    t.max_idle_timeout(Some(
        IDLE_TIMEOUT
            .try_into()
            .map_err(|e| ServerError::Tls(format!("idle timeout: {e}")))?,
    ));

    t.datagram_send_buffer_size(16 * 1024);
    t.datagram_receive_buffer_size(Some(64 * 1024));

    t.max_concurrent_bidi_streams(1u32.into());

    t.max_concurrent_uni_streams(32u32.into());
    t.stream_receive_window((MAX_PICTURE as u32).into());
    t.receive_window((4 * MAX_PICTURE as u32).into());
    t.send_window(4 * MAX_PICTURE as u64);
    Ok(t)
}
