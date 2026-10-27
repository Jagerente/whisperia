use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use quinn::crypto::rustls::QuicClientConfig;
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::CryptoProvider,
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use tokio::sync::mpsc;
use whisperia_proto::{
    control::{
        encode_frame, Avatar, ChatText, ClientMsg, FrameDecoder, JoinError, Name, PeerInfo,
        RejectReason, RoomName, RoomState, RoomSummary, ServerMsg, ShareInfo, VoiceState,
        MAX_PEERS, MAX_ROOMS,
    },
    peek::Glance,
    video::{MediaHello, PictureHeader, HEADER_LEN, MAX_PICTURE},
    PeerId, ALPN, ALPN_MEDIA, ALPN_PEEK, PROTOCOL_VERSION, TLS_SERVER_NAME,
};

use crate::{
    address::{Route, ServerAddress, Target},
    trust::{Fingerprint, TrustStore, Verdict},
    wake::Waker,
};

const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(2500);
const STAGGER: Duration = Duration::from_millis(150);
const MAX_ADDRESSES: usize = 8;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const AUDIO_QUEUE: usize = 8;
const PICTURE_QUEUE: usize = 4;
const MEDIA_RETRY: Duration = Duration::from_secs(1);
const MAX_GLANCE: usize = 256;
const NO_APPLICATION_PROTOCOL: u8 = 120;
const KEEP_ALIVE: Duration = Duration::from_secs(2);
const IDLE_TIMEOUT: Duration = Duration::from_secs(8);
const GOODBYE_DELIVERED_WITHIN: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("cannot resolve \"{0}\"")]
    Resolve(String),
    #[error("server not reachable ({0})")]
    Unreachable(String),
    #[error("the server's certificate changed since the last visit")]
    IdentityChanged { fingerprint: Fingerprint },
    #[error("{0}")]
    Rejected(RejectReason),
    #[error("unexpected answer from the server: {0}")]
    Protocol(String),
    #[error("cannot set up networking: {0}")]
    Setup(String),
}

#[derive(Debug, Clone)]
pub struct Me {
    pub name: Name,
    pub avatar: Avatar,
}

#[derive(Debug, Clone)]
pub struct Welcome {
    pub id: PeerId,
    pub name: Name,
    pub server_name: Name,
    pub rooms: Vec<RoomSummary>,
    pub fingerprint: Fingerprint,
    pub addr: SocketAddr,
}

#[derive(Debug)]
pub enum SessionEvent {
    Rooms(Vec<RoomSummary>),
    Joined {
        room: RoomName,
        peers: Vec<PeerInfo>,
        state: RoomState,
    },
    JoinRefused(JoinError),
    RoomChanged(RoomState),
    PeerJoined(PeerInfo),
    PeerLeft(PeerId),
    PeerKicked(PeerId),
    Kicked,
    PeerState {
        id: PeerId,
        state: VoiceState,
    },
    PeerRenamed {
        id: PeerId,
        name: Name,
    },
    PeerAvatar {
        id: PeerId,
        avatar: Avatar,
    },
    Chat {
        from: PeerId,
        text: ChatText,
    },
    PeerShare {
        id: PeerId,
        share: Option<ShareInfo>,
    },
    Viewers(Vec<PeerId>),
    KeyframeWanted,
    Closed(String),
}

#[derive(Debug, Clone)]
pub struct OutgoingPicture {
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone)]
pub struct IncomingPicture {
    pub seq: u32,
    pub keyframe: bool,
    pub data: Bytes,
}

pub type PictureSink = Box<dyn Fn(IncomingPicture) + Send + Sync>;
pub type DatagramSink = Box<dyn Fn(Bytes, Instant) + Send + Sync>;

type SharedDatagramSink = Arc<Mutex<Option<DatagramSink>>>;
type PictureSinks = Arc<Mutex<HashMap<PeerId, PictureSink>>>;

pub struct Session {
    conn: quinn::Connection,
    endpoint: quinn::Endpoint,
    control: mpsc::UnboundedSender<ClientMsg>,
    audio: mpsc::Sender<Bytes>,
    datagrams: SharedDatagramSink,
    pictures: mpsc::Sender<OutgoingPicture>,
    picture_sinks: PictureSinks,
    media_wanted: Arc<tokio::sync::Notify>,
}

impl Session {
    pub fn send(&self, msg: ClientMsg) {
        let _ = self.control.send(msg);
    }

    pub fn audio_sender(&self) -> mpsc::Sender<Bytes> {
        self.audio.clone()
    }

    pub fn picture_sender(&self) -> mpsc::Sender<OutgoingPicture> {
        self.pictures.clone()
    }

    pub fn open_media(&self) {
        self.media_wanted.notify_one();
    }

    pub fn set_picture_sink(&self, sharer: PeerId, sink: Option<PictureSink>) {
        let mut sinks = self.picture_sinks.lock().unwrap_or_else(|p| p.into_inner());
        match sink {
            Some(sink) => sinks.insert(sharer, sink),
            None => sinks.remove(&sharer),
        };
    }

    pub fn set_datagram_sink(&self, sink: Option<DatagramSink>) {
        *self.datagrams.lock().unwrap_or_else(|p| p.into_inner()) = sink;
    }

    pub fn rtt(&self) -> Duration {
        self.conn.rtt()
    }

    pub fn sent_and_lost(&self) -> (u64, u64) {
        let path = self.conn.stats().path;
        (path.sent_packets, path.lost_packets)
    }

    pub async fn say_goodbye(self) {
        let endpoint = self.endpoint.clone();
        drop(self);
        let _ = tokio::time::timeout(GOODBYE_DELIVERED_WITHIN, endpoint.wait_idle()).await;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.conn.close(0u32.into(), b"bye");
    }
}

pub async fn resolve(address: &ServerAddress) -> Result<Vec<SocketAddr>, ConnectError> {
    let unresolved = || ConnectError::Resolve(address.host().to_owned());
    let mut addrs: Vec<SocketAddr> = tokio::net::lookup_host((address.host(), address.port()))
        .await
        .map_err(|_| unresolved())?
        .collect();
    addrs.sort_by_key(|a| a.is_ipv6());
    if addrs.is_empty() {
        return Err(unresolved());
    }
    Ok(addrs)
}

#[derive(Debug, thiserror::Error)]
pub enum PeekError {
    #[error("{0}")]
    Silent(String),
    #[error("{0}")]
    Stranger(String),
}

pub async fn peek(address: &ServerAddress) -> Result<Glance, PeekError> {
    let silent = |e: &dyn std::fmt::Display| PeekError::Silent(e.to_string());
    let addrs = resolve(address).await.map_err(|e| silent(&e))?;
    let mut endpoint =
        quinn::Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0))).map_err(|e| silent(&e))?;
    endpoint.set_default_client_config(client_config(ALPN_PEEK).map_err(|e| silent(&e))?);
    let (conn, _) = first_reachable(&endpoint, &addrs)
        .await
        .map_err(|miss| match miss {
            Miss::Silent(said) => PeekError::Silent(said),
            Miss::Refused(said) => PeekError::Stranger(said),
        })?;
    let stranger = |e: &dyn std::fmt::Display| PeekError::Stranger(e.to_string());
    let look = async {
        let mut recv = conn.accept_uni().await.map_err(|e| silent(&e))?;
        let bytes = recv.read_to_end(MAX_GLANCE).await.map_err(|e| match e {
            quinn::ReadToEndError::Read(quinn::ReadError::ConnectionLost(lost)) => silent(&lost),
            other => stranger(&other),
        })?;
        let mut decoder = FrameDecoder::new();
        decoder.push(&bytes);
        decoder
            .next_message::<Glance>()
            .map_err(|e| stranger(&e))?
            .ok_or_else(|| stranger(&"the answer was cut short"))
    };
    let glance = tokio::time::timeout(HANDSHAKE_TIMEOUT, look).await;
    conn.close(0u32.into(), b"seen");
    glance.map_err(|_| stranger(&"no answer to a peek"))?
}

pub type Connected = (Session, Welcome, mpsc::UnboundedReceiver<SessionEvent>);

pub async fn connect(
    target: Target,
    me: Me,
    accept_changed: Option<Fingerprint>,
    trust: Arc<Mutex<TrustStore>>,
    waker: Waker,
) -> Result<Connected, ConnectError> {
    let addrs = match &target.route {
        Route::Discovered(addrs) => addrs.clone(),
        Route::Typed(address) => resolve(address).await?,
    };
    let setup = |e: &dyn std::fmt::Display| ConnectError::Setup(e.to_string());
    let mut endpoint =
        quinn::Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0))).map_err(|e| setup(&e))?;
    endpoint.set_default_client_config(client_config(ALPN).map_err(|e| setup(&e))?);
    let media_config = client_config(ALPN_MEDIA).map_err(|e| setup(&e))?;

    let (conn, addr) = first_reachable(&endpoint, &addrs)
        .await
        .map_err(|miss| ConnectError::Unreachable(miss.said()))?;

    let fingerprint = peer_fingerprint(&conn)
        .ok_or_else(|| ConnectError::Protocol("server presented no certificate".into()))?;
    let verdict = trust
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .check(&target.trust_key, &fingerprint);
    let unexpected = target
        .expected
        .is_some_and(|expected| expected != fingerprint);
    if (verdict == Verdict::Changed || unexpected) && accept_changed != Some(fingerprint) {
        conn.close(0u32.into(), b"certificate changed");
        return Err(ConnectError::IdentityChanged { fingerprint });
    }

    let hello = ClientMsg::Hello {
        protocol: PROTOCOL_VERSION,
        name: me.name,
        avatar: me.avatar,
        password: target.password.clone(),
    };
    let handshake = async {
        let (mut send, mut recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
        let frame = encode_frame(&hello).map_err(|e| e.to_string())?;
        send.write_all(&frame).await.map_err(|e| e.to_string())?;
        let mut decoder = FrameDecoder::new();
        let answer = read_msg(&mut recv, &mut decoder).await?;
        Ok::<_, String>((send, recv, decoder, answer))
    };
    let (send, recv, decoder, answer) = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
        .await
        .map_err(|_| ConnectError::Protocol("server did not answer the hello".into()))?
        .map_err(ConnectError::Protocol)?;

    let (id, name, server_name, mut rooms, media_token) = match answer {
        ServerMsg::Welcome {
            id,
            name,
            server_name,
            rooms,
            media_token,
        } => (id, name, server_name, rooms, media_token),
        ServerMsg::Rejected(reason) => return Err(ConnectError::Rejected(reason)),
        other => {
            return Err(ConnectError::Protocol(format!(
                "{other:?} instead of a welcome"
            )))
        }
    };
    rooms.truncate(MAX_ROOMS);

    if let Err(e) = trust
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remember(&target.trust_key, &fingerprint)
    {
        tracing::warn!("cannot save the server's fingerprint: {e}");
    }

    let (events_tx, events) = mpsc::unbounded_channel();
    let (control, control_rx) = mpsc::unbounded_channel();
    let (audio, audio_rx) = mpsc::channel(AUDIO_QUEUE);
    let datagrams: SharedDatagramSink = Arc::default();
    let (pictures, pictures_rx) = mpsc::channel(PICTURE_QUEUE);
    let picture_sinks: PictureSinks = Arc::default();
    let media_wanted = Arc::new(tokio::sync::Notify::new());

    tokio::spawn(run_media(
        MediaSetup {
            endpoint: endpoint.clone(),
            config: media_config,
            addr,
            fingerprint,
            hello: MediaHello {
                protocol: PROTOCOL_VERSION,
                peer: id,
                token: media_token,
            },
            control: conn.clone(),
        },
        media_wanted.clone(),
        pictures_rx,
        picture_sinks.clone(),
    ));
    tokio::spawn(run(
        Pipes {
            conn: conn.clone(),
            send,
            recv,
            decoder,
            control: control_rx,
            audio: audio_rx,
            datagrams: datagrams.clone(),
        },
        events_tx,
        waker,
    ));

    let welcome = Welcome {
        id,
        name,
        server_name,
        rooms,
        fingerprint,
        addr,
    };
    Ok((
        Session {
            conn,
            endpoint,
            control,
            audio,
            datagrams,
            pictures,
            picture_sinks,
            media_wanted,
        },
        welcome,
        events,
    ))
}

struct MediaSetup {
    endpoint: quinn::Endpoint,
    config: quinn::ClientConfig,
    addr: SocketAddr,
    fingerprint: Fingerprint,
    hello: MediaHello,
    control: quinn::Connection,
}

async fn run_media(
    setup: MediaSetup,
    wanted: Arc<tokio::sync::Notify>,
    mut outgoing: mpsc::Receiver<OutgoingPicture>,
    sinks: PictureSinks,
) {
    tokio::select! {
        _ = wanted.notified() => {}
        _ = setup.control.closed() => return,
    }
    let mut seq = 0u32;
    loop {
        let session = async {
            let conn = connect_media(&setup).await?;
            let reason = pump_media(&conn, &mut outgoing, &mut seq, &sinks).await;
            conn.close(0u32.into(), b"bye");
            Err::<(), String>(reason)
        };
        tokio::select! {
            Err(reason) = session => tracing::warn!("media connection: {reason}"),
            _ = setup.control.closed() => return,
        }

        while outgoing.try_recv().is_ok() {}
        tokio::select! {
            _ = tokio::time::sleep(MEDIA_RETRY) => {}
            _ = setup.control.closed() => return,
        }
    }
}

async fn connect_media(setup: &MediaSetup) -> Result<quinn::Connection, String> {
    let connecting = setup
        .endpoint
        .connect_with(setup.config.clone(), setup.addr, TLS_SERVER_NAME)
        .map_err(|e| e.to_string())?;
    let conn = tokio::time::timeout(ATTEMPT_TIMEOUT, connecting)
        .await
        .map_err(|_| "no answer".to_owned())?
        .map_err(|e| e.to_string())?;

    if peer_fingerprint(&conn) != Some(setup.fingerprint) {
        conn.close(0u32.into(), b"certificate changed");
        return Err("the server presented another certificate".into());
    }
    let mut stream = conn.open_uni().await.map_err(|e| e.to_string())?;
    let hello = encode_frame(&setup.hello).map_err(|e| e.to_string())?;
    stream.write_all(&hello).await.map_err(|e| e.to_string())?;
    stream.finish().map_err(|e| e.to_string())?;
    Ok(conn)
}

async fn pump_media(
    conn: &quinn::Connection,
    outgoing: &mut mpsc::Receiver<OutgoingPicture>,
    seq: &mut u32,
    sinks: &PictureSinks,
) -> String {
    let send = async {
        while let Some(picture) = outgoing.recv().await {
            if picture.data.is_empty() || picture.data.len() > MAX_PICTURE {
                continue;
            }
            let header = PictureHeader {
                sharer: 0,
                seq: *seq,
                keyframe: picture.keyframe,
                width: picture.width,
                height: picture.height,
            };
            *seq = seq.wrapping_add(1);
            let mut stream = conn.open_uni().await.map_err(|e| e.to_string())?;
            let sent = async {
                stream.write_all(&header.to_bytes()).await?;
                stream.write_all(&picture.data).await
            };
            sent.await.map_err(|e| e.to_string())?;
            let _ = stream.finish();
        }
        std::future::pending::<Result<(), String>>().await
    };
    let receive = async {
        loop {
            let mut stream = conn.accept_uni().await.map_err(|e| e.to_string())?;
            let Ok(bytes) = stream.read_to_end(HEADER_LEN + MAX_PICTURE).await else {
                continue;
            };
            let bytes = Bytes::from(bytes);
            let Ok((header, data)) = PictureHeader::parse(&bytes) else {
                continue;
            };
            let sinks = sinks.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(sink) = sinks.get(&header.sharer) {
                sink(IncomingPicture {
                    seq: header.seq,
                    keyframe: header.keyframe,
                    data: bytes.slice_ref(data),
                });
            }
        }
    };
    let outcome: Result<(), String> = tokio::select! {
        r = send => r,
        r = receive => r,
    };
    outcome.err().unwrap_or_else(|| "closed".into())
}

#[derive(Debug)]
enum Miss {
    Silent(String),
    Refused(String),
}

impl Miss {
    fn said(self) -> String {
        match self {
            Self::Silent(said) | Self::Refused(said) => said,
        }
    }
}

async fn first_reachable(
    endpoint: &quinn::Endpoint,
    addrs: &[SocketAddr],
) -> Result<(quinn::Connection, SocketAddr), Miss> {
    let mut attempts = tokio::task::JoinSet::new();
    for (position, addr) in addrs.iter().copied().take(MAX_ADDRESSES).enumerate() {
        let endpoint = endpoint.clone();
        attempts.spawn(async move {
            tokio::time::sleep(STAGGER * position as u32).await;
            let connecting = endpoint
                .connect(addr, TLS_SERVER_NAME)
                .map_err(|e| Miss::Silent(format!("{addr}: {e}")))?;
            match tokio::time::timeout(ATTEMPT_TIMEOUT, connecting).await {
                Ok(Ok(conn)) => Ok((conn, addr)),
                Ok(Err(quinn::ConnectionError::ConnectionClosed(close)))
                    if close.error_code
                        == quinn::TransportErrorCode::crypto(NO_APPLICATION_PROTOCOL) =>
                {
                    Err(Miss::Refused(format!("{addr}: {close}")))
                }
                Ok(Err(e)) => Err(Miss::Silent(format!("{addr}: {e}"))),
                Err(_) => Err(Miss::Silent(format!("{addr}: no answer"))),
            }
        });
    }

    let (mut refused, mut silent) = (None, None);
    while let Some(finished) = attempts.join_next().await {
        match finished {
            Ok(Ok(winner)) => return Ok(winner),
            Ok(Err(Miss::Refused(said))) => refused = refused.or(Some(said)),
            Ok(Err(Miss::Silent(said))) => silent = silent.or(Some(said)),
            Err(e) => silent = silent.or(Some(e.to_string())),
        }
    }
    Err(match (refused, silent) {
        (Some(said), _) => Miss::Refused(said),
        (None, said) => Miss::Silent(said.unwrap_or_else(|| "no address to try".into())),
    })
}

fn event_of(msg: ServerMsg) -> Result<SessionEvent, String> {
    Ok(match msg {
        ServerMsg::Rooms(mut rooms) => {
            rooms.truncate(MAX_ROOMS);
            SessionEvent::Rooms(rooms)
        }
        ServerMsg::Joined {
            room,
            mut peers,
            state,
        } => {
            peers.truncate(MAX_PEERS);
            SessionEvent::Joined { room, peers, state }
        }
        ServerMsg::JoinRefused(reason) => SessionEvent::JoinRefused(reason),
        ServerMsg::RoomChanged(state) => SessionEvent::RoomChanged(state),
        ServerMsg::PeerJoined(peer) => SessionEvent::PeerJoined(peer),
        ServerMsg::PeerLeft(id) => SessionEvent::PeerLeft(id),
        ServerMsg::PeerKicked(id) => SessionEvent::PeerKicked(id),
        ServerMsg::Kicked => SessionEvent::Kicked,
        ServerMsg::PeerState { id, state } => SessionEvent::PeerState { id, state },
        ServerMsg::PeerAvatar { id, avatar } => SessionEvent::PeerAvatar { id, avatar },
        ServerMsg::PeerRenamed { id, name } => SessionEvent::PeerRenamed { id, name },
        ServerMsg::Chat { from, text } => SessionEvent::Chat { from, text },
        ServerMsg::PeerShare { id, share } => SessionEvent::PeerShare { id, share },
        ServerMsg::Viewers(mut viewers) => {
            viewers.truncate(MAX_PEERS);
            SessionEvent::Viewers(viewers)
        }
        ServerMsg::KeyframeWanted => SessionEvent::KeyframeWanted,
        other @ (ServerMsg::Welcome { .. } | ServerMsg::Rejected(_)) => {
            return Err(format!("unexpected {other:?}"))
        }
    })
}

struct Pipes {
    conn: quinn::Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    decoder: FrameDecoder,
    control: mpsc::UnboundedReceiver<ClientMsg>,
    audio: mpsc::Receiver<Bytes>,
    datagrams: SharedDatagramSink,
}

async fn run(pipes: Pipes, events: mpsc::UnboundedSender<SessionEvent>, waker: Waker) {
    let Pipes {
        conn,
        mut send,
        mut recv,
        mut decoder,
        control: mut control_rx,
        audio: mut audio_rx,
        datagrams,
    } = pipes;

    let control_out = async {
        while let Some(msg) = control_rx.recv().await {
            let frame = encode_frame(&msg).map_err(|e| e.to_string())?;
            send.write_all(&frame).await.map_err(|e| e.to_string())?;
        }
        std::future::pending::<Result<(), String>>().await
    };

    let control_in = async {
        loop {
            let event = event_of(read_msg(&mut recv, &mut decoder).await?)?;
            if events.send(event).is_err() {
                return Ok::<(), String>(());
            }
            waker.wake();
        }
    };

    let audio_out = async {
        while let Some(datagram) = audio_rx.recv().await {
            let _ = conn.send_datagram(datagram);
        }
        std::future::pending::<Result<(), String>>().await
    };

    let audio_in = async {
        loop {
            let datagram = match conn.read_datagram().await {
                Ok(datagram) => datagram,
                Err(e) => break Err::<(), String>(e.to_string()),
            };
            let at = Instant::now();
            if let Some(sink) = datagrams.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
                sink(datagram, at);
            }
        }
    };

    let reason = tokio::select! {
        r = control_out => r.err(),
        r = control_in => r.err(),
        r = audio_out => r.err(),
        r = audio_in => r.err(),
        e = conn.closed() => Some(describe_close(&e)),
    };
    let reason = match conn.close_reason() {
        Some(e) => describe_close(&e),
        None => reason.unwrap_or_else(|| "connection closed".into()),
    };
    conn.close(0u32.into(), b"bye");
    let _ = events.send(SessionEvent::Closed(reason));
    waker.wake();
}

fn describe_close(e: &quinn::ConnectionError) -> String {
    match e {
        quinn::ConnectionError::ApplicationClosed(close) => {
            let text = String::from_utf8_lossy(&close.reason);
            if text.is_empty() {
                "server closed the connection".into()
            } else {
                text.into_owned()
            }
        }
        quinn::ConnectionError::TimedOut => "connection lost (timed out)".into(),
        quinn::ConnectionError::LocallyClosed => "disconnected".into(),
        other => other.to_string(),
    }
}

async fn read_msg(
    recv: &mut quinn::RecvStream,
    decoder: &mut FrameDecoder,
) -> Result<ServerMsg, String> {
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(msg) = decoder.next_message().map_err(|e| e.to_string())? {
            return Ok(msg);
        }
        match recv.read(&mut chunk).await.map_err(|e| e.to_string())? {
            Some(n) => decoder.push(&chunk[..n]),
            None => return Err("server closed the control stream".into()),
        }
    }
}

fn peer_fingerprint(conn: &quinn::Connection) -> Option<Fingerprint> {
    let identity = conn.peer_identity()?;
    let certs = identity.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    Some(Fingerprint::of(certs.first()?))
}

fn client_config(alpn: &[u8]) -> Result<quinn::ClientConfig, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![alpn.to_vec()];

    let quic = QuicClientConfig::try_from(tls)
        .map_err(|e| rustls::Error::General(format!("TLS 1.3 unavailable: {e}")))?;
    let mut config = quinn::ClientConfig::new(Arc::new(quic));

    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(KEEP_ALIVE));
    transport.max_idle_timeout(Some(
        IDLE_TIMEOUT
            .try_into()
            .map_err(|e| rustls::Error::General(format!("idle timeout: {e}")))?,
    ));

    transport.datagram_send_buffer_size(16 * 1024);
    transport.datagram_receive_buffer_size(Some(64 * 1024));

    transport.max_concurrent_uni_streams(32u32.into());
    transport.stream_receive_window((MAX_PICTURE as u32).into());
    transport.receive_window((4 * MAX_PICTURE as u32).into());
    transport.send_window(4 * MAX_PICTURE as u64);

    let mut acks = quinn::AckFrequencyConfig::default();
    acks.ack_eliciting_threshold(0u32.into());
    transport.ack_frequency_config(Some(acks));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

#[derive(Debug)]
struct AnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_welcome_or_rejection_in_the_middle_of_a_session_ends_it() {
        assert!(event_of(ServerMsg::Rejected(RejectReason::ServerFull)).is_err());
        assert!(matches!(
            event_of(ServerMsg::KeyframeWanted),
            Ok(SessionEvent::KeyframeWanted)
        ));
    }

    #[test]
    fn lists_from_the_server_are_kept_to_their_bounds() {
        let many = (0..MAX_PEERS as u16 + 10).collect();
        let Ok(SessionEvent::Viewers(viewers)) = event_of(ServerMsg::Viewers(many)) else {
            panic!("viewers");
        };
        assert_eq!(viewers.len(), MAX_PEERS);
    }
}
