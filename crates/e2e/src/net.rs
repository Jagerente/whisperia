use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use tokio::sync::mpsc::UnboundedReceiver;
use whisperia::{
    address::{ServerAddress, Target},
    audio::fake::FakeBackend,
    dsp::denoise::DenoiseMode,
    engine::{Controls, Engine},
    net::{
        self, ConnectError, IncomingPicture, Me, OutgoingPicture, Session, SessionEvent, Welcome,
    },
    screen::{PictureGate, ShareSound, Verdict},
    trust::{Fingerprint, TrustStore},
    wake::Waker,
};
use whisperia_proto::{
    control::{
        ChatText, ClientMsg, Name, Password, PeerInfo, RoomName, RoomSummary, ShareInfo, VoiceState,
    },
    PeerId,
};

pub const EVENT_PATIENCE: Duration = Duration::from_secs(10);

pub fn target(addr: SocketAddr) -> Target {
    Target::typed(ServerAddress::from(addr), None)
}

pub fn with_password(mut target: Target, typed: &str) -> Target {
    target.password = Password::new(typed).ok();
    target
}

pub fn me(name: &str) -> Me {
    Me {
        name: Name::new(name).expect("a test person has a name"),
        avatar: crate::people::animal_of(name),
    }
}

pub fn trust() -> Arc<Mutex<TrustStore>> {
    Arc::new(Mutex::new(TrustStore::in_memory()))
}

pub type Events = UnboundedReceiver<SessionEvent>;

pub async fn connect(addr: SocketAddr, name: &str) -> (Session, Welcome, Events) {
    try_connect(target(addr), name)
        .await
        .unwrap_or_else(|e| panic!("{name} cannot connect to {addr}: {e}"))
}

pub async fn try_connect(
    target: Target,
    name: &str,
) -> Result<(Session, Welcome, Events), ConnectError> {
    connect_trusting(target, name, None, trust()).await
}

pub async fn connect_trusting(
    target: Target,
    name: &str,
    accept_changed: Option<Fingerprint>,
    trust: Arc<Mutex<TrustStore>>,
) -> Result<(Session, Welcome, Events), ConnectError> {
    net::connect(target, me(name), accept_changed, trust, Waker::default()).await
}

pub fn enter(session: &Session, room: &str) {
    session.send(ClientMsg::Join {
        room: RoomName::new(room).expect("a test room has a name"),
        password: None,
        rules: None,
    });
}

pub fn say(session: &Session, text: &str) {
    if let Ok(text) = ChatText::new(text) {
        session.send(ClientMsg::Chat(text));
    }
}

pub fn rename(session: &Session, name: &str) {
    session.send(ClientMsg::Rename(Name::new(name).expect("a new name")));
}

pub fn send_state(session: &Session, state: VoiceState) {
    session.send(ClientMsg::State(state));
}

pub fn share_start(session: &Session, sound: bool) {
    session.open_media();
    session.send(ClientMsg::ShareStart(ShareInfo {
        sound,
        minimized: false,
    }));
}

pub fn share_stop(session: &Session) {
    session.send(ClientMsg::ShareStop);
}

pub fn watch_only(session: &Session, sharer: PeerId) {
    session.open_media();
    session.send(ClientMsg::Watch(sharer));
}

pub fn unwatch(session: &Session, sharer: PeerId) {
    session.set_picture_sink(sharer, None);
    session.send(ClientMsg::Unwatch(sharer));
}

pub async fn next_event_where(
    events: &mut Events,
    what: &str,
    mut wanted: impl FnMut(&SessionEvent) -> bool,
) -> SessionEvent {
    let waiting = async {
        loop {
            match events.recv().await {
                Some(event) if wanted(&event) => return event,
                Some(_) => {}
                None => panic!("the session ended while waiting for {what}"),
            }
        }
    };
    tokio::time::timeout(EVENT_PATIENCE, waiting)
        .await
        .unwrap_or_else(|_| panic!("{what} never came"))
}

pub fn names(room: &RoomSummary) -> Vec<&str> {
    room.people.iter().map(|p| p.name.as_str()).collect()
}

pub struct Member {
    pub id: PeerId,
    pub session: Session,
    pub welcome: Welcome,
    pub events: Events,
    pub peers: Vec<PeerInfo>,
}

pub async fn member_of(addr: SocketAddr, name: &str, room: &str) -> Member {
    let (session, welcome, mut events) = connect(addr, name).await;
    enter(&session, room);
    let peers = joined(&mut events, name, room).await;
    Member {
        id: welcome.id,
        session,
        welcome,
        events,
        peers,
    }
}

pub async fn member(addr: SocketAddr, name: &str) -> Member {
    member_of(addr, name, "General").await
}

async fn joined(events: &mut Events, name: &str, room: &str) -> Vec<PeerInfo> {
    match next_event_where(events, "the room to be joined", |event| {
        matches!(
            event,
            SessionEvent::Joined { .. } | SessionEvent::JoinRefused(_)
        )
    })
    .await
    {
        SessionEvent::Joined {
            room: joined,
            peers,
            ..
        } if joined == room => peers,
        other => panic!("{name} could not join {room}: {other:?}"),
    }
}

pub struct Pictures {
    pub keyframe: Arc<AtomicBool>,
    pub sent: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

impl Pictures {
    pub async fn wait_until_sent(&self, at_least: u64) {
        let deadline = Instant::now() + EVENT_PATIENCE;
        while self.sent.load(Ordering::Relaxed) < at_least {
            assert!(Instant::now() < deadline, "the pictures are not going out");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for Pictures {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub const SHARE_TONE: f32 = 660.0;
pub const KEYFRAME_BYTES: usize = 80_000;

pub fn send_pictures_and_sound(session: &Session) -> Pictures {
    let pictures = Pictures {
        keyframe: Arc::new(AtomicBool::new(true)),
        sent: Arc::default(),
        stop: Arc::default(),
    };
    session.open_media();
    let queue = session.picture_sender();
    let mut sound = ShareSound::new(session.audio_sender()).expect("share sound can be encoded");
    let (keyframe, sent, stop) = (
        pictures.keyframe.clone(),
        pictures.sent.clone(),
        pictures.stop.clone(),
    );
    std::thread::spawn(move || {
        let mut number = 0u64;
        let mut phase = 0f32;
        let started = Instant::now();
        let mut delivered = 0usize;
        while !stop.load(Ordering::Relaxed) {
            let key = keyframe.swap(false, Ordering::Relaxed);
            let mut data = vec![0u8; if key { KEYFRAME_BYTES } else { 4_000 }];
            data[..8].copy_from_slice(&number.to_be_bytes());
            number += 1;
            let _ = queue.try_send(OutgoingPicture {
                data,
                keyframe: key,
                width: 1920,
                height: 1080,
            });
            sent.fetch_add(1, Ordering::Relaxed);
            let due = (started.elapsed().as_secs_f64() * 48_000.0) as usize;
            let chunk: Vec<f32> = (delivered..due)
                .flat_map(|_| {
                    phase = (phase + SHARE_TONE * std::f32::consts::TAU / 48_000.0)
                        % std::f32::consts::TAU;
                    [phase.sin() * 0.3; 2]
                })
                .collect();
            delivered = delivered.max(due);
            sound.push(&chunk);
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    pictures
}

pub type Seen = Arc<Mutex<Vec<(bool, u64, usize)>>>;

pub fn watch(session: &Session, sharer: PeerId) -> Seen {
    let seen: Seen = Arc::default();
    let (sink, gate) = (seen.clone(), Mutex::new(PictureGate::default()));
    session.set_picture_sink(
        sharer,
        Some(Box::new(move |picture: IncomingPicture| {
            let admitted = gate
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .admit(picture.seq, picture.keyframe);
            if admitted == Verdict::Show {
                let number = picture
                    .data
                    .get(..8)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map_or(0, u64::from_be_bytes);
                sink.lock().unwrap_or_else(|p| p.into_inner()).push((
                    picture.keyframe,
                    number,
                    picture.data.len(),
                ));
            }
        })),
    );
    session.open_media();
    session.send(ClientMsg::Watch(sharer));
    seen
}

pub fn catch_without_asking(session: &Session, sharer: PeerId) -> Seen {
    let seen: Seen = Arc::default();
    let sink = seen.clone();
    session.set_picture_sink(
        sharer,
        Some(Box::new(move |picture: IncomingPicture| {
            sink.lock().unwrap_or_else(|p| p.into_inner()).push((
                picture.keyframe,
                0,
                picture.data.len(),
            ));
        })),
    );
    seen
}

pub async fn wait_for_pictures(seen: &Seen, at_least: usize) -> Vec<(bool, u64, usize)> {
    let deadline = Instant::now() + EVENT_PATIENCE;
    loop {
        let so_far = seen.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if so_far.len() >= at_least {
            return so_far;
        }
        assert!(Instant::now() < deadline, "only {} pictures", so_far.len());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[derive(Clone, Default)]
pub struct SharerHears {
    pub viewers: Arc<Mutex<Option<Vec<PeerId>>>>,
    pub asked_for_keyframes: Arc<AtomicBool>,
}

impl SharerHears {
    pub fn viewers(&self) -> Option<Vec<PeerId>> {
        self.viewers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub async fn wait_for_viewers(&self, wanted: &[PeerId]) {
        let deadline = Instant::now() + EVENT_PATIENCE;
        while self.viewers().as_deref() != Some(wanted) {
            assert!(
                Instant::now() < deadline,
                "the sharer was never told {wanted:?} watch, only {:?}",
                self.viewers()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

pub fn answer_keyframes(mut events: Events, pictures: &Pictures) -> SharerHears {
    let hears = SharerHears::default();
    let (told, keyframe) = (hears.clone(), pictures.keyframe.clone());
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::KeyframeWanted => {
                    told.asked_for_keyframes.store(true, Ordering::Relaxed);
                    keyframe.store(true, Ordering::Relaxed);
                }
                SessionEvent::Viewers(now) => {
                    *told.viewers.lock().unwrap_or_else(|p| p.into_inner()) = Some(now)
                }
                _ => {}
            }
        }
    });
    hears
}

pub async fn wait_for_occupancy(server: &crate::TestServer, wanted: (usize, usize)) {
    let deadline = Instant::now() + EVENT_PATIENCE;
    while server.occupancy() != wanted {
        assert!(
            Instant::now() < deadline,
            "the server counts {:?}, not {wanted:?}",
            server.occupancy()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub struct Client {
    pub id: PeerId,
    pub session: Session,
    pub welcome: Welcome,
    pub events: Events,
    pub engine: Engine,
    pub controls: Arc<Controls>,
    pub audio: FakeBackend,
    pub peers: Vec<PeerInfo>,
}

pub struct Joining {
    addr: SocketAddr,
    name: String,
    room: String,
    audio: FakeBackend,
    denoise: DenoiseMode,
}

impl Joining {
    pub fn room(mut self, room: &str) -> Self {
        self.room = room.into();
        self
    }

    pub fn audio(mut self, audio: FakeBackend) -> Self {
        self.audio = audio;
        self
    }

    pub fn denoise(mut self, denoise: DenoiseMode) -> Self {
        self.denoise = denoise;
        self
    }

    pub async fn join(self) -> Client {
        let Member {
            id,
            session,
            welcome,
            events,
            peers,
        } = member_of(self.addr, &self.name, &self.room).await;
        let controls = Arc::new(Controls::new(self.denoise, 0.0, 0.0, false));
        let mut engine = Engine::new(
            Arc::new(self.audio.clone()),
            controls.clone(),
            &Waker::default(),
        )
        .unwrap_or_else(|e| panic!("the sound of {} does not start: {e}", self.name));
        session.set_datagram_sink(Some(Box::new(engine.received_sink())));
        engine.enter_room(session.audio_sender());
        if let Some(trouble) = engine.troubles().next() {
            panic!("the sound of {} does not start: {trouble:?}", self.name);
        }
        Client {
            id,
            session,
            welcome,
            events,
            engine,
            controls,
            audio: self.audio,
            peers,
        }
    }
}

pub fn joining(addr: SocketAddr, name: &str) -> Joining {
    Joining {
        addr,
        name: name.into(),
        room: "General".into(),
        audio: FakeBackend::new(48_000, None),
        denoise: DenoiseMode::Off,
    }
}

pub async fn join(addr: SocketAddr, name: &str) -> Client {
    joining(addr, name).join().await
}
