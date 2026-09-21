mod chat;
mod choosing;
pub mod link;
pub mod message;
mod outbox;
mod room;
mod servers;
mod stalls;
mod unfurls;

use std::{
    sync::{mpsc::Receiver, Arc, Mutex},
    time::{Duration, Instant},
};

use whisperia_proto::{
    control::{
        Avatar, ChatText, ClientMsg, JoinError, Name, Password, RejectReason, RoomLimit, RoomName,
        RoomRules, RoomSummary, VoiceState,
    },
    PeerId,
};

pub use self::chat::{About, Chat, ChatLine, Line, Said};
pub use self::choosing::{Choosing, Offered};
pub use self::message::{Message, Piece, Url};
pub use self::room::{Ending, InRoom, Sharing};
pub use self::servers::{Entry, Occupancy, Status};
use self::{
    link::{Link, Look},
    outbox::Outbox,
    servers::Lookout,
    stalls::Stalls,
    unfurls::Unfurls,
};
use crate::{
    address::{NotAnAddress, ServerAddress, Target},
    audio::{AudioBackend, DeviceList},
    config::{
        clamp_db, clamp_mic_db, Appearance, Config, HotkeyAction, Hotkeys, Paths, RecentServer,
    },
    desktop::{
        keys::{self, Combination},
        DesktopCommand, DesktopEvent, DesktopLink, DesktopShare, DesktopStatus, Presence,
    },
    discovery::{Discovery, Search},
    dsp::{cue::Cue, denoise::DenoiseMode},
    engine::{AudioTrouble, Controls, Engine},
    instance::Instance,
    net::{self, ConnectError, Connected, Me, Session, SessionEvent, Welcome},
    preview::{LinkPreview, Previews},
    screen::{ScreenBackend, SourceKind},
    trust::{Fingerprint, TrustStore},
    wake::{self, Waker, WakingSender},
};

pub const LIVE: Duration = Duration::from_millis(50);
const NOTICE_TIME: Duration = Duration::from_millis(3500);
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

pub struct Options {
    pub paths: Paths,
    pub config: Config,
    pub backend: Arc<dyn AudioBackend>,
    pub screen: Arc<dyn ScreenBackend>,
    pub previews: Arc<dyn Previews>,
    pub connect: Option<String>,
    pub desktop: Option<DesktopLink>,
    pub instance: Option<Instance>,
    pub waker: Waker,
    pub search: Search,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    ShowWindow,
    OpenSettings,
    LookAtRoom(RoomName),
    Open(Url),
    Copy(String),
    Quit,
}

pub struct Retry {
    pub(crate) attempt: u32,
    pub(crate) reason: String,
    muted: bool,
    deafened: bool,
    room: Option<Resume>,
}

struct Resume {
    room: RoomName,
    password: Option<Password>,
    rules: RoomRules,
    chat: Chat,
}

impl Resume {
    fn after_drop(room: Option<&mut InRoom>, not_back_yet: Option<Resume>) -> Option<Resume> {
        let Some(room) = room else {
            return not_back_yet;
        };
        Some(Resume {
            room: room.name.clone(),
            password: room.password.clone(),
            rules: RoomRules {
                password: room.password.clone(),
                limit: room.limit,
            },
            chat: std::mem::take(&mut room.chat),
        })
    }
}

pub struct Joining {
    pub(crate) room: RoomName,
    password: Option<Password>,
    rules: Option<RoomRules>,
}

pub struct LockedOut {
    pub(crate) room: RoomName,
    pub(crate) why: Refusal,
}

pub enum KeyRemark {
    Refused {
        action: HotkeyAction,
        why: &'static str,
    },
    Taken {
        action: HotkeyAction,
        keys: Combination,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Password,
    WrongPassword,
    Full,
}

enum Attempt {
    Running(Receiver<Result<Connected, ConnectError>>),
    Waiting(Instant),
}

pub struct Connecting {
    pub(crate) target: Target,
    attempt: Attempt,
    pub(crate) retry: Option<Retry>,
}

pub struct Online {
    target: Target,
    pub(crate) session: Session,
    pub(crate) welcome: Welcome,
    pub(crate) name: Name,
    events: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
    pub(crate) rooms: Vec<RoomSummary>,
    pub(crate) joining: Option<Joining>,
    pub(crate) locked_out: Option<LockedOut>,
    resume: Option<Resume>,
    pub(crate) room: Option<InRoom>,
    pub(crate) link: Link,
    pub(crate) muted: bool,
    pub(crate) deafened: bool,
}

pub enum Stage {
    Offline { address: Option<String> },
    Connecting(Box<Connecting>),
    Online(Box<Online>),
}

pub enum Prompt {
    Trust {
        target: Target,
        fingerprint: Fingerprint,
    },
    Password {
        target: Target,
        wrong: bool,
    },
    OtherVersion {
        server: String,
        theirs: Option<u16>,
    },
}

pub struct Refused {
    pub(crate) address: String,
    pub(crate) why: RefusedWhy,
}

#[derive(Debug, thiserror::Error)]
pub enum RefusedWhy {
    #[error(transparent)]
    NotAnAddress(#[from] NotAnAddress),
    #[error("could not connect: {0}")]
    NotConnected(#[from] ConnectError),
}

pub struct Notice {
    pub(crate) text: String,
    pub(crate) is_error: bool,
    until: Instant,
}

pub struct App {
    paths: Paths,
    pub(crate) config: Config,
    backend: Arc<dyn AudioBackend>,
    screens: Arc<dyn ScreenBackend>,
    runtime: tokio::runtime::Runtime,
    waker: Waker,
    trust: Arc<Mutex<TrustStore>>,
    discovery: Option<Discovery>,
    pub(crate) discovery_error: Option<String>,
    lookout: Lookout,
    peeks_to: WakingSender<(ServerAddress, Status)>,
    peeks: Receiver<(ServerAddress, Status)>,
    pub(crate) engine: Engine,
    stalls: Stalls,
    unfurls: Unfurls,
    pub(crate) stage: Stage,
    pub(crate) prompt: Option<Prompt>,
    pub(crate) notice: Option<Notice>,
    pub(crate) refused: Option<Refused>,
    desktop: Option<DesktopLink>,
    tray: bool,
    instance: Option<Instance>,
    shown_status: Option<DesktopStatus>,
    pub(crate) capturing: Option<HotkeyAction>,
    pub(crate) key_remark: Option<KeyRemark>,
    effects: Vec<Effect>,
    changed: bool,
    share_tab: SourceKind,
}

impl App {
    pub fn new(options: Options) -> anyhow::Result<Self> {
        let Options {
            paths,
            config,
            backend,
            screen: screens,
            previews,
            connect,
            desktop,
            instance,
            waker,
            search,
        } = options;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ww-net")
            .enable_all()
            .build()?;
        let (discovery, discovery_error) = match search {
            Search::Lan => {
                let found = waker.clone();
                match Discovery::start(move || found.wake()) {
                    Ok(d) => (Some(d), None),
                    Err(e) => {
                        tracing::warn!("mDNS discovery unavailable: {e}");
                        (None, Some(e.to_string()))
                    }
                }
            }
            #[cfg(feature = "testing")]
            Search::Nowhere => (None, None),
        };
        let controls = Arc::new(Controls::new(
            config.denoise,
            config.mic_gain_db,
            config.volume_db,
            config.agc,
        ));
        let mut engine = Engine::new(backend.clone(), controls, &waker)?;
        engine.use_devices(config.input_device.clone(), config.output_device.clone());
        let trust = Arc::new(Mutex::new(TrustStore::load(paths.trust.clone())));
        let (peeks_to, peeks) = wake::channel(&waker);
        let unfurls = Unfurls::new(previews, &waker);

        let mut app = Self {
            stage: Stage::Offline { address: None },
            paths,
            config,
            backend,
            screens,
            runtime,
            waker,
            trust,
            discovery,
            discovery_error,
            lookout: Lookout::default(),
            peeks_to,
            peeks,
            engine,
            stalls: Stalls::default(),
            unfurls,
            prompt: None,
            notice: None,
            refused: None,
            tray: desktop.is_some(),
            desktop,
            instance,
            shown_status: None,
            capturing: None,
            key_remark: None,
            effects: Vec::new(),
            changed: true,
            share_tab: SourceKind::Screen,
        };
        if let Some(address) = connect {
            app.connect_address(&address);
        }
        Ok(app)
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    pub fn has_tray(&self) -> bool {
        self.tray
    }

    pub fn room(&self) -> Option<&InRoom> {
        match &self.stage {
            Stage::Online(online) => online.room.as_ref(),
            _ => None,
        }
    }

    pub fn my_name(&self) -> &Name {
        match &self.stage {
            Stage::Online(online) => &online.name,
            _ => &self.config.name,
        }
    }

    pub fn audio_troubles(&self) -> impl Iterator<Item = &AudioTrouble> {
        self.engine.troubles()
    }

    pub fn entries(&self) -> Vec<Entry> {
        let mut entries = self.known_servers();
        entries.retain(|entry| !entry.gone());
        entries
    }

    pub fn devices(&self) -> DeviceList {
        self.backend.devices()
    }

    pub fn devices_news(&self) -> u64 {
        self.engine.devices_news()
    }

    fn known_servers(&self) -> Vec<Entry> {
        let found = self
            .discovery
            .as_ref()
            .map(Discovery::servers)
            .unwrap_or_default();
        servers::gather(found, &self.config.recent, &self.lookout)
    }

    fn info(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            is_error: false,
            until: Instant::now() + NOTICE_TIME,
        });
        self.changed = true;
    }

    fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::warn!("{text}");
        self.notice = Some(Notice {
            text,
            is_error: true,
            until: Instant::now() + NOTICE_TIME * 3 / 2,
        });
        self.changed = true;
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    fn cue(&mut self, cue: Cue) {
        if self.config.sounds && self.config.notifications.allows(cue) {
            self.engine.play(cue);
        }
    }

    pub fn play_cue(&mut self, cue: Cue) {
        self.engine.play(cue);
    }

    fn deliver(&mut self, out: Outbox) {
        self.changed |= out.changed;
        out.cues.into_iter().for_each(|cue| self.cue(cue));
        for message in &out.unfurl {
            self.unfurls.ask(&self.runtime, message);
        }
        self.forget_old_previews();
        for said in out.said {
            match said {
                outbox::Said::Info(text) => self.info(text),
                outbox::Said::Error(text) => self.error(text),
            }
        }
    }

    pub fn save_config(&mut self) {
        if let Err(e) = self.config.save(&self.paths.config) {
            tracing::warn!("cannot save settings: {e}");
        }
    }

    pub fn quit(&mut self) {
        self.effects.push(Effect::Quit);
        self.waker.wake();
    }

    pub fn say_goodbye(&mut self) {
        self.save_config();
        self.engine.leave_room();
        let stage = std::mem::replace(&mut self.stage, Stage::Offline { address: None });
        if let Stage::Online(online) = stage {
            let Online { session, room, .. } = *online;
            drop(room);
            self.runtime.block_on(session.say_goodbye());
        }
        self.desktop = None;
    }

    pub fn window_closed(&mut self) {
        if !(self.tray && self.config.close_to_tray) {
            self.quit();
        }
    }

    pub fn next_tick(&self, now: Instant) -> Option<Instant> {
        let live = (self.room().is_some() || self.engine.is_testing()).then_some(now + LIVE);
        let stage = match &self.stage {
            Stage::Offline { .. } => self.lookout.next_due(),
            Stage::Connecting(connecting) => match connecting.attempt {
                Attempt::Waiting(at) => Some(at),
                Attempt::Running(_) => None,
            },
            Stage::Online(_) => None,
        };
        let notice = self.notice.as_ref().map(|notice| notice.until);
        [live, stage, notice, self.engine.next_tick()]
            .into_iter()
            .flatten()
            .min()
    }

    pub fn tick(&mut self, now: Instant) {
        self.waker.woken();
        if self.notice.as_ref().is_some_and(|n| now >= n.until) {
            self.notice = None;
            self.changed = true;
        }
        self.changed |= self.engine.tick(now);
        self.changed |= self.unfurls.heard();
        self.poll_desktop();

        match &self.stage {
            Stage::Offline { .. } => self.look_around(now),
            Stage::Connecting(_) => self.tick_connecting(now),
            Stage::Online(_) => self.tick_online(now),
        }
        self.follow_volumes();
        self.tell_desktop();
    }

    fn look_around(&mut self, now: Instant) {
        for (address, status) in self.peeks.try_iter() {
            self.lookout.heard(now, &address, status);
            self.changed = true;
        }
        let entries = self.known_servers();
        for address in self.lookout.due_now(now, servers::watched(&entries)) {
            let answer = self.peeks_to.clone();
            self.runtime.spawn(async move {
                let status = Status::of_peek(&net::peek(&address).await);
                let _ = answer.send((address, status));
            });
        }
    }

    pub fn connect_address(&mut self, typed: &str) {
        match typed.parse::<ServerAddress>() {
            Ok(address) => {
                let target = match self
                    .config
                    .recent
                    .iter()
                    .find(|recent| recent.address == address)
                {
                    Some(recent) => remembered(recent),
                    None => Target::typed(address, None),
                };
                self.begin_connect(target, None, None);
            }
            Err(e) => {
                self.refused = Some(Refused {
                    address: typed.to_owned(),
                    why: e.into(),
                });
                self.changed = true;
            }
        }
    }

    pub fn connect_entry(&mut self, entry: &Entry) {
        if let Status::OtherVersion(theirs) = entry.status {
            self.prompt = Some(Prompt::OtherVersion {
                server: entry.name.to_string(),
                theirs,
            });
            return;
        }
        match (&entry.found, &entry.recent) {
            (_, Some(recent)) => self.begin_connect(remembered(recent), None, None),
            (Some(server), None) => self.begin_connect(Target::discovered(server), None, None),
            (None, None) => {}
        }
    }

    pub fn close_version_prompt(&mut self) {
        if let Some(Prompt::OtherVersion { .. }) = self.prompt {
            self.prompt = None;
        }
    }

    pub fn forget_recent(&mut self, recent: &RecentServer) {
        self.config.recent.retain(|r| r != recent);
        self.save_config();
    }

    fn begin_connect(&mut self, target: Target, accept: Option<Fingerprint>, retry: Option<Retry>) {
        let (tx, rx) = wake::channel(&self.waker);
        let me = Me {
            name: self.config.name.clone(),
            avatar: self.config.avatar,
        };
        let (trust, waker, job) = (self.trust.clone(), self.waker.clone(), target.clone());
        self.runtime.spawn(async move {
            let _ = tx.send(net::connect(job, me, accept, trust, waker).await);
        });
        self.prompt = None;
        self.refused = None;
        self.stage = Stage::Connecting(Box::new(Connecting {
            target,
            attempt: Attempt::Running(rx),
            retry,
        }));
        self.changed = true;
    }

    pub fn cancel_connect(&mut self) {
        if let Stage::Connecting(connecting) = &self.stage {
            let address = connecting.target.typed_address().map(ToString::to_string);
            self.stage = Stage::Offline { address };
            self.changed = true;
        }
    }

    pub fn disconnect(&mut self) {
        if let Stage::Online(_) = &self.stage {
            self.engine.leave_room();
            self.stage = Stage::Offline { address: None };
            self.info("Disconnected.");
        }
    }

    pub fn answer_trust(&mut self, accept: bool) {
        let Some(Prompt::Trust {
            target,
            fingerprint,
        }) = self.prompt.take()
        else {
            return;
        };
        if accept {
            self.begin_connect(target, Some(fingerprint), None);
        } else {
            self.info("Not connected. The remembered certificate was kept.");
        }
    }

    pub fn answer_password(&mut self, typed: Option<&str>) {
        let Some(Prompt::Password { mut target, .. }) = self.prompt.take() else {
            return;
        };
        let Some(typed) = typed else {
            return self.info("Not connected.");
        };
        match Password::new(typed) {
            Ok(password) => {
                target.password = Some(password);
                self.begin_connect(target, None, None);
            }
            Err(_) => {
                self.prompt = Some(Prompt::Password {
                    target,
                    wrong: true,
                })
            }
        }
    }

    fn tick_connecting(&mut self, now: Instant) {
        let Stage::Connecting(connecting) = &mut self.stage else {
            return;
        };
        let outcome = match &connecting.attempt {
            Attempt::Waiting(at) if now >= *at => {
                let (target, retry) = (connecting.target.clone(), connecting.retry.take());
                return self.begin_connect(target, None, retry);
            }
            Attempt::Waiting(_) => return,
            Attempt::Running(result) => match result.try_recv() {
                Ok(outcome) => outcome,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err(ConnectError::Setup("connection task died".into()))
                }
            },
        };
        self.changed = true;
        let target = connecting.target.clone();
        match outcome {
            Ok(connected) => {
                let retry = connecting.retry.take();
                self.enter_online(target, connected, retry);
            }
            Err(ConnectError::IdentityChanged { fingerprint }) => {
                self.cancel_connect();
                self.prompt = Some(Prompt::Trust {
                    target,
                    fingerprint,
                });
            }
            Err(ConnectError::Rejected(RejectReason::BadPassword)) => {
                let wrong = target.password.is_some();
                self.cancel_connect();
                self.prompt = Some(Prompt::Password { target, wrong });
            }
            Err(ConnectError::Rejected(RejectReason::ProtocolMismatch { server })) => {
                self.cancel_connect();
                self.prompt = Some(Prompt::OtherVersion {
                    server: target.label,
                    theirs: Some(server),
                });
            }
            Err(ConnectError::Rejected(RejectReason::ServerFull)) if connecting.retry.is_none() => {
                self.cancel_connect();
                self.error(format!(
                    "{} is full. Try again when somebody leaves.",
                    target.label
                ));
            }
            Err(e) => match connecting.retry.as_mut() {
                Some(retry) => {
                    retry.attempt += 1;
                    retry.reason = e.to_string();
                    connecting.attempt = Attempt::Waiting(now + RECONNECT_DELAY);
                }
                None => {
                    self.cancel_connect();
                    match target.typed_address() {
                        Some(address) => {
                            tracing::warn!("could not connect to {address}: {e}");
                            self.refused = Some(Refused {
                                address: address.to_string(),
                                why: e.into(),
                            });
                        }
                        None => self.error(format!("Could not connect: {e}")),
                    }
                }
            },
        }
    }

    fn enter_online(
        &mut self,
        target: Target,
        (session, welcome, events): Connected,
        retry: Option<Retry>,
    ) {
        if let Some(address) = target.typed_address() {
            self.config.touch_recent(RecentServer {
                name: welcome.server_name.clone(),
                address: address.clone(),
                id: Some(welcome.fingerprint),
            });
            self.save_config();
        }
        if welcome.name != self.config.name {
            tracing::info!(
                "{} calls us \"{}\": \"{}\" is taken there, it comes back once that one leaves",
                welcome.server_name,
                welcome.name,
                self.config.name
            );
        }
        let received = self.engine.received_sink();
        session.set_datagram_sink(Some(Box::new(received)));
        let (muted, deafened, resume) = match retry {
            Some(r) => (r.muted, r.deafened, r.room),
            None => (false, false, None),
        };
        let mut online = Online {
            target,
            rooms: welcome.rooms.clone(),
            name: welcome.name.clone(),
            session,
            welcome,
            events,
            joining: None,
            locked_out: None,
            resume: None,
            room: None,
            link: Link::default(),
            muted,
            deafened,
        };
        online.publish_state(self.engine.controls(), self.engine.is_testing());
        if let Some(resume) = resume {
            online.join(Joining {
                room: resume.room.clone(),
                password: resume.password.clone(),
                rules: Some(resume.rules.clone()),
            });
            online.resume = Some(resume);
        }
        self.stage = Stage::Online(Box::new(online));
    }

    pub fn enter_room(&mut self, name: &str, typed: Option<&str>) {
        let Ok(room) = RoomName::new(name) else {
            return;
        };
        if let Stage::Online(online) = &mut self.stage {
            online.join(Joining {
                room,
                password: typed.and_then(|typed| Password::new(typed).ok()),
                rules: None,
            });
        }
    }

    pub fn open_room(&mut self, name: &str, typed: Option<&str>, limit: Option<RoomLimit>) {
        let Ok(room) = RoomName::new(name) else {
            return self.info("Type a name for the new room.");
        };
        let password = typed.and_then(|typed| Password::new(typed).ok());
        if let Stage::Online(online) = &mut self.stage {
            online.join(Joining {
                room,
                password: password.clone(),
                rules: Some(RoomRules { password, limit }),
            });
        }
    }

    fn hosted_room(&mut self) -> Option<(&Session, &mut InRoom)> {
        match &mut self.stage {
            Stage::Online(online) => {
                let room = online.room.as_mut().filter(|room| room.is_host())?;
                Some((&online.session, room))
            }
            _ => None,
        }
    }

    pub fn set_room_password(&mut self, typed: Option<&str>) {
        let password = match typed {
            Some(typed) => match Password::new(typed) {
                Ok(password) => Some(password),
                Err(_) => return self.info("Type a password, or remove it."),
            },
            None => None,
        };
        let Some((session, room)) = self.hosted_room() else {
            return;
        };
        let changed = room.locked && password.is_some();
        session.send(ClientMsg::SetRoomPassword(password.clone()));
        room.password = password;
        if changed {
            room.note(
                About::Locked,
                "The room asks for a new password now".to_owned(),
            );
        }
        self.changed = true;
    }

    pub fn set_room_limit(&mut self, limit: Option<RoomLimit>) {
        if let Some((session, room)) = self.hosted_room() {
            if room.ask_limit(limit) {
                session.send(ClientMsg::SetRoomLimit(limit));
            }
        }
    }

    pub fn make_host(&mut self, heir: PeerId) {
        if let Some((session, room)) = self.hosted_room() {
            if room.peer(heir).is_some() {
                session.send(ClientMsg::MakeHost(heir));
            }
        }
    }

    pub fn kick(&mut self, target: PeerId) {
        if let Some((session, room)) = self.hosted_room() {
            if room.peer(target).is_some() {
                session.send(ClientMsg::Kick(target));
            }
        }
    }

    pub fn leave_room(&mut self) {
        let Stage::Online(online) = &mut self.stage else {
            return;
        };
        let Some(room) = &mut online.room else {
            return;
        };
        room.forget_screens(&online.session);
        online.session.send(ClientMsg::Leave);
        online.room = None;
        self.engine.leave_room();
        self.info("Left the room.");
    }

    pub fn send_chat(&mut self, typed: &str) {
        let Ok(text) = ChatText::new(typed) else {
            return;
        };
        let avatar = self.config.avatar;
        let Stage::Online(online) = &mut self.stage else {
            return;
        };
        let Some(room) = &mut online.room else {
            return;
        };
        online.session.send(ClientMsg::Chat(text.clone()));
        let message = Message::from(&text);
        self.unfurls.ask(&self.runtime, &message);
        room.say(Said {
            from: Some(room.me),
            who: online.name.clone(),
            avatar,
            message,
            mine: true,
        });
        self.forget_old_previews();
    }

    pub fn preview_of(&self, url: &Url) -> Option<&Arc<LinkPreview>> {
        self.unfurls.of(url)
    }

    pub fn hide_preview(&mut self, line: u64, url: &str) {
        let (Some(url), Stage::Online(online)) = (Url::parse(url), &mut self.stage) else {
            return;
        };
        if let Some(room) = &mut online.room {
            room.chat.hide_preview(line, url);
            self.changed = true;
        }
    }

    fn forget_old_previews(&mut self) {
        if !self.unfurls.too_many() {
            return;
        }
        let wanted = match &self.stage {
            Stage::Online(online) => online
                .room
                .iter()
                .flat_map(|room| room.chat.links())
                .collect(),
            _ => Default::default(),
        };
        self.unfurls.keep_only(&wanted);
    }

    pub fn open_link(&mut self, url: &str) {
        match Url::parse(url) {
            Some(url) => self.effects.push(Effect::Open(url)),
            None => tracing::warn!("not a web address to open: {url}"),
        }
        self.waker.wake();
    }

    pub fn copy(&mut self, text: &str) {
        self.effects.push(Effect::Copy(text.to_owned()));
        self.waker.wake();
    }

    fn tick_online(&mut self, now: Instant) {
        let Stage::Online(online) = &mut self.stage else {
            return;
        };
        let (heard, missed) = self
            .engine
            .telemetry()
            .peers()
            .iter()
            .fold((0, 0), |(heard, missed), peer| {
                (heard + peer.jitter.received, missed + peer.jitter.lost)
            });
        let look = online.look(heard, missed);
        online.link.take(now, look);
        if online.room.is_some() {
            if let Some(skips) = self.stalls.take(now, self.engine.losses()) {
                tracing::warn!(
                    "sound card: microphone lost {} samples, speakers ran dry {} times in the last {} s",
                    skips.microphone_samples,
                    skips.speaker_underruns,
                    stalls::WINDOW.as_secs()
                );
            }
        }

        let mut closed = None;
        let mut out = Outbox::default();
        loop {
            let event = match online.events.try_recv() {
                Ok(event) => event,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    closed.get_or_insert_with(|| "connection closed".into());
                    break;
                }
            };
            out.changed();
            match event {
                SessionEvent::Rooms(rooms) => online.rooms = rooms,
                SessionEvent::Joined { room, peers, state } => {
                    let joining = online.joining.take();
                    online.locked_out = None;
                    let resumed = online.resume.take().filter(|resume| resume.room == room);
                    let resuming = resumed.is_some();
                    let chat = resumed.map(|resume| resume.chat).unwrap_or_default();
                    let me = online.welcome.id;
                    let (password, asked_rules) = match joining {
                        Some(joining) if joining.room == room => (
                            joining.password,
                            joining
                                .rules
                                .filter(|rules| !resuming && *rules != RoomRules::default()),
                        ),
                        _ => (None, None),
                    };
                    if asked_rules.is_some() && state.host != me {
                        out.info(format!(
                            "\"{room}\" was already open, so its own password and limit stay."
                        ));
                    }
                    let password = password.filter(|_| state.locked);
                    online.room = Some(InRoom::new(room, peers, state, me, password, chat));
                    self.engine.enter_room(online.session.audio_sender());
                    out.cue(Cue::Connected);
                }
                SessionEvent::JoinRefused(reason) => {
                    online.resume = None;
                    let Some(joining) = online.joining.take() else {
                        continue;
                    };
                    let room = joining.room;
                    match reason {
                        JoinError::Password => {
                            let why = match joining.password {
                                Some(_) => Refusal::WrongPassword,
                                None => Refusal::Password,
                            };
                            self.effects.push(Effect::LookAtRoom(room.clone()));
                            online.locked_out = Some(LockedOut { room, why });
                        }
                        JoinError::Full => {
                            self.effects.push(Effect::LookAtRoom(room.clone()));
                            online.locked_out = Some(LockedOut {
                                room,
                                why: Refusal::Full,
                            });
                        }
                        reason => out.error(format!("Cannot join: {reason}")),
                    }
                }
                SessionEvent::Closed(reason) => closed = Some(reason),
                SessionEvent::Kicked => {
                    if let Some(mut room) = online.room.take() {
                        room.forget_screens(&online.session);
                        self.engine.leave_room();
                        out.error(format!(
                            "{} kicked you from \"{}\".",
                            room.name_of(room.host),
                            room.name
                        ));
                        out.cue(Cue::Leave);
                    }
                }
                SessionEvent::PeerRenamed { id, name } if id == online.welcome.id => {
                    online.name = name.clone();
                    if let Some(room) = &mut online.room {
                        room.note(About::Renamed, format!("You are now {name}"));
                    }
                }
                other => {
                    if let Some(room) = &mut online.room {
                        room.on_event(other, &online.session, &self.engine, &mut out);
                    }
                }
            }
        }
        if let (Some(room), None) = (&mut online.room, &closed) {
            room.tick_screens(now, &online.session, &mut out);
        }
        if let Some(reason) = closed {
            let target = Target {
                label: online.welcome.server_name.to_string(),
                ..online.target.clone()
            };
            let retry = Retry {
                attempt: 1,
                reason,
                muted: online.muted,
                deafened: online.deafened,
                room: Resume::after_drop(online.room.as_mut(), online.resume.take()),
            };
            self.engine.leave_room();
            self.begin_connect(target, None, Some(retry));
            self.cue(Cue::Lost);
        }
        self.deliver(out);
    }

    fn follow_volumes(&mut self) {
        let Some(room) = self.room() else {
            return;
        };
        let config = &self.config;
        let voice: Vec<(PeerId, f32)> = room
            .peers
            .iter()
            .map(|p| (p.id, config.person_volume(&p.name)))
            .collect();
        let share: Vec<(PeerId, f32)> = room
            .peers
            .iter()
            .map(|p| (p.id, config.person_screen_volume_heard(&p.name)))
            .collect();
        self.engine.controls().set_volumes(&voice, &share);
    }

    fn poll_desktop(&mut self) {
        if self.instance.as_ref().is_some_and(Instance::called) {
            self.effects.push(Effect::ShowWindow);
        }
        let events: Vec<DesktopEvent> = match &self.desktop {
            Some(desktop) => desktop.events.try_iter().collect(),
            None => return,
        };
        for event in events {
            self.changed = true;
            match event {
                DesktopEvent::ToggleMute => self.toggle_mute(),
                DesktopEvent::ToggleDeafen => self.toggle_deafen(),
                DesktopEvent::ToggleShare => self.toggle_share(),
                DesktopEvent::ToggleWatch(sharer) => self.toggle_watch(sharer),
                DesktopEvent::ShowWindow => self.effects.push(Effect::ShowWindow),
                DesktopEvent::OpenSettings => {
                    self.effects.push(Effect::ShowWindow);
                    self.effects.push(Effect::OpenSettings);
                }
                DesktopEvent::Quit => self.effects.push(Effect::Quit),
                DesktopEvent::KeyCaptured(combination) => self.hotkey_captured(combination),
                DesktopEvent::KeyCaptureCancelled => {
                    self.capturing = None;
                    self.key_remark = None;
                }
                DesktopEvent::Problem(text) => self.error(text),
                DesktopEvent::NoTray => self.tray = false,
            }
        }
    }

    fn tell_desktop(&mut self) {
        let status = self.desktop_status();
        if self.shown_status.as_ref() == Some(&status) {
            return;
        }
        if let Some(desktop) = &self.desktop {
            (desktop.tell)(DesktopCommand::Status(status.clone()));
        }
        self.shown_status = Some(status);
    }

    fn desktop_status(&self) -> DesktopStatus {
        let room = self.room();
        let shares = room.map_or_else(Vec::new, |room| {
            room.peers
                .iter()
                .filter(|peer| peer.share.is_some())
                .map(|peer| DesktopShare {
                    sharer: peer.id,
                    name: peer.name.to_string(),
                    watched: room.is_watching(peer.id),
                })
                .collect()
        });
        let returning = matches!(&self.stage, Stage::Connecting(c) if c.retry.is_some());
        let testing = self.engine.is_testing();
        let presence = match self.voice_kept() {
            Some(_) if testing && room.is_some() => Presence::Deafened,
            Some((_, true)) if room.is_some() || returning => Presence::Deafened,
            Some((true, false)) if room.is_some() || returning => Presence::Muted,
            Some((false, false)) if room.is_some() => Presence::Live,
            _ => Presence::Away,
        };
        DesktopStatus {
            presence,
            sharing: room.is_some_and(|room| room.share.is_some()),
            shares,
        }
    }

    pub fn voice_kept(&self) -> Option<(bool, bool)> {
        match &self.stage {
            Stage::Online(online) => Some((online.muted, online.deafened)),
            Stage::Connecting(connecting) => {
                connecting.retry.as_ref().map(|r| (r.muted, r.deafened))
            }
            Stage::Offline { .. } => None,
        }
    }

    fn voice_mut(&mut self) -> Option<(&mut bool, &mut bool)> {
        match &mut self.stage {
            Stage::Online(online) => Some((&mut online.muted, &mut online.deafened)),
            Stage::Connecting(connecting) => connecting
                .retry
                .as_mut()
                .map(|r| (&mut r.muted, &mut r.deafened)),
            Stage::Offline { .. } => None,
        }
    }

    fn voice_changed(&mut self, cue: Cue) {
        self.publish_state();
        self.changed = true;
        self.cue(cue);
    }

    pub fn toggle_mute(&mut self) {
        if self.room().is_some() && self.engine.is_testing() {
            return self.stop_mic_test();
        }
        let Some((muted, deafened)) = self.voice_mut() else {
            return;
        };
        if *deafened {
            *deafened = false;
            *muted = false;
        } else {
            *muted = !*muted;
        }
        let cue = if *muted { Cue::Mute } else { Cue::Unmute };
        self.voice_changed(cue);
    }

    pub fn toggle_deafen(&mut self) {
        if self.room().is_some() && self.engine.is_testing() {
            return self.stop_mic_test();
        }
        let Some((_, deafened)) = self.voice_mut() else {
            return;
        };
        *deafened = !*deafened;
        let cue = if *deafened {
            Cue::Deafen
        } else {
            Cue::Undeafen
        };
        self.voice_changed(cue);
    }

    pub fn toggle_share(&mut self) {
        let (screens, waker) = (self.screens.clone(), self.waker.clone());
        let Stage::Online(online) = &mut self.stage else {
            return;
        };
        let Some(room) = &mut online.room else {
            return self.info("Enter a room first: a screen is shared with a room.");
        };
        let mut out = Outbox::default();
        if room.share.is_some() {
            room.stop_share(&online.session, &mut out);
        } else if room.choosing.take().is_none() {
            room.begin_choosing(screens, self.share_tab, None, &waker);
            self.effects.push(Effect::ShowWindow);
        }
        self.deliver(out);
    }

    pub fn change_share(&mut self) {
        let (screens, waker) = (self.screens.clone(), self.waker.clone());
        let Stage::Online(online) = &mut self.stage else {
            return;
        };
        let Some(room) = &mut online.room else {
            return;
        };
        let Some(shared) = room.share.as_ref().map(|share| share.key) else {
            return;
        };
        if room.choosing.is_none() {
            room.begin_choosing(screens, shared.kind, Some(shared), &waker);
        }
    }

    fn choosing(&mut self) -> Option<&mut Choosing> {
        match &mut self.stage {
            Stage::Online(online) => online.room.as_mut()?.choosing.as_mut(),
            _ => None,
        }
    }

    pub fn show_share_tab(&mut self, tab: SourceKind) {
        if let Some(choosing) = self.choosing() {
            choosing.show(tab);
        }
    }

    pub fn pick_source(&mut self, id: i32) {
        if let Some(choosing) = self.choosing() {
            choosing.pick(id);
        }
    }

    pub fn share_source(&mut self, id: i32) {
        let (screens, config, waker) = (
            self.screens.clone(),
            self.config.share.clone(),
            self.waker.clone(),
        );
        let Stage::Online(online) = &mut self.stage else {
            return;
        };
        let Some(room) = &mut online.room else {
            return;
        };
        if let Some(choosing) = &room.choosing {
            self.share_tab = choosing.tab();
        }
        let mut out = Outbox::default();
        room.share_chosen(id, &online.session, &screens, &config, &waker, &mut out);
        self.deliver(out);
    }

    pub fn cancel_choosing(&mut self) {
        if let Stage::Online(online) = &mut self.stage {
            if let Some(room) = &mut online.room {
                room.choosing = None;
            }
        }
    }

    pub fn toggle_watch(&mut self, sharer: PeerId) {
        let (screens, waker) = (self.screens.clone(), self.waker.clone());
        let Stage::Online(online) = &mut self.stage else {
            return;
        };
        let Some(room) = &mut online.room else {
            return;
        };
        if room.peer(sharer).is_none_or(|peer| peer.share.is_none()) {
            return;
        }
        let mut out = Outbox::default();
        if room.is_watching(sharer) {
            room.stop_watching(&online.session);
        } else {
            room.start_watching(sharer, &online.session, &screens, &waker, &mut out);
        }
        self.deliver(out);
    }

    pub fn stop_watching(&mut self) {
        if let Stage::Online(online) = &mut self.stage {
            if let Some(room) = &mut online.room {
                room.stop_watching(&online.session);
            }
        }
    }

    pub fn set_volume(&mut self, db: f32) {
        self.config.volume_db = clamp_db(db);
        self.engine.controls().volume_db.set(self.config.volume_db);
    }

    pub fn set_person_volume(&mut self, id: PeerId, db: f32) {
        let Some(name) = self
            .room()
            .and_then(|room| room.peer(id))
            .map(|p| p.name.clone())
        else {
            return;
        };
        self.config.set_person_volume(&name, clamp_db(db));
    }

    pub fn set_screen_volume(&mut self, sharer: PeerId, db: f32) {
        let Some(room) = self.room() else {
            return;
        };
        let Some(peer) = room.peer(sharer) else {
            return;
        };
        if !peer.share.is_some_and(|share| share.sound) {
            return;
        }
        let name = peer.name.clone();
        self.config.set_person_screen_volume(&name, clamp_db(db));
        self.config.set_person_screen_muted(&name, false);
    }

    pub fn toggle_screen_mute(&mut self, sharer: PeerId) {
        let Some(name) = self
            .room()
            .and_then(|room| room.peer(sharer))
            .map(|p| p.name.clone())
        else {
            return;
        };
        let muted = self.config.person_screen_muted(&name);
        self.config.set_person_screen_muted(&name, !muted);
        self.save_config();
    }

    pub fn set_mic_gain(&mut self, db: f32) {
        if self.config.agc {
            return;
        }
        self.config.mic_gain_db = clamp_mic_db(db);
        self.engine
            .controls()
            .mic_gain_db
            .set(self.config.mic_gain_db);
    }

    pub fn set_denoise(&mut self, mode: DenoiseMode) {
        self.config.denoise = mode;
        self.engine.controls().set_denoise(mode);
        self.save_config();
    }

    pub fn toggle_agc(&mut self) {
        self.config.agc = !self.config.agc;
        self.engine
            .controls()
            .agc
            .store(self.config.agc, std::sync::atomic::Ordering::Relaxed);
        self.save_config();
    }

    pub fn set_close_to_tray(&mut self, on: bool) {
        self.config.close_to_tray = on;
        self.save_config();
    }

    pub fn set_appearance(&mut self, appearance: Appearance) {
        self.config.appearance = appearance;
        self.save_config();
    }

    pub fn toggle_sounds(&mut self) {
        self.config.sounds = !self.config.sounds;
        self.save_config();
    }

    pub fn toggle_notification(&mut self, cue: Cue) {
        self.config.notifications.toggle(cue);
        self.save_config();
    }

    pub fn capture_hotkey(&mut self, action: HotkeyAction) {
        if self.capturing == Some(action) {
            return self.cancel_hotkey_capture();
        }
        let Some(desktop) = &self.desktop else {
            return;
        };
        (desktop.tell)(DesktopCommand::CaptureKey(true));
        self.capturing = Some(action);
        self.key_remark = None;
    }

    pub fn cancel_hotkey_capture(&mut self) {
        self.key_remark = None;
        if self.capturing.take().is_some() {
            if let Some(desktop) = &self.desktop {
                (desktop.tell)(DesktopCommand::CaptureKey(false));
            }
        }
    }

    pub fn clear_hotkey(&mut self, action: HotkeyAction) {
        self.cancel_hotkey_capture();
        self.config.hotkeys.set(action, None);
        self.hotkeys_changed();
    }

    pub fn reset_hotkey(&mut self, action: HotkeyAction) {
        self.cancel_hotkey_capture();
        let usual = Hotkeys::default().get(action);
        if let Some(keys) = usual {
            self.take_keys_from_others(action, keys);
        }
        self.config.hotkeys.set(action, usual);
        self.hotkeys_changed();
    }

    pub fn reset_hotkeys(&mut self) {
        self.cancel_hotkey_capture();
        self.config.hotkeys = Hotkeys::default();
        self.hotkeys_changed();
    }

    fn hotkey_captured(&mut self, combination: Combination) {
        let Some(action) = self.capturing else {
            return;
        };
        let refusal = match keys::name(combination) {
            None => Some("That key cannot be used here"),
            Some(_) if !keys::leaves_typing_alone(combination) => {
                Some("Add Ctrl or Alt: a plain key would stop working in every other program")
            }
            Some(_) => None,
        };
        if let Some(refusal) = refusal {
            if let Some(desktop) = &self.desktop {
                (desktop.tell)(DesktopCommand::CaptureKey(true));
            }
            self.key_remark = Some(KeyRemark::Refused {
                action,
                why: refusal,
            });
            self.changed = true;
            return;
        }
        self.capturing = None;
        self.key_remark = None;
        self.take_keys_from_others(action, combination);
        self.config.hotkeys.set(action, Some(combination));
        self.hotkeys_changed();
    }

    fn take_keys_from_others(&mut self, action: HotkeyAction, combination: Combination) {
        for other in HotkeyAction::ALL {
            if other != action && self.config.hotkeys.get(other) == Some(combination) {
                self.config.hotkeys.set(other, None);
                self.key_remark = Some(KeyRemark::Taken {
                    action: other,
                    keys: combination,
                });
            }
        }
    }

    fn hotkeys_changed(&mut self) {
        if let Some(desktop) = &self.desktop {
            (desktop.tell)(DesktopCommand::Hotkeys(self.config.hotkeys.clone()));
        }
        self.save_config();
    }

    pub fn set_name(&mut self, name: Name) {
        let known = self.config.name == name;
        if known && self.config.name_confirmed {
            return;
        }
        self.config.name = name.clone();
        self.config.name_confirmed = true;
        self.save_config();
        if known {
            return;
        }
        if let Stage::Online(online) = &self.stage {
            online.session.send(ClientMsg::Rename(name));
        }
    }

    pub fn set_avatar(&mut self, avatar: Avatar) {
        if self.config.avatar == avatar {
            return;
        }
        self.config.avatar = avatar;
        self.save_config();
        if let Stage::Online(online) = &mut self.stage {
            online.session.send(ClientMsg::SetAvatar(avatar));
            if let Some(room) = &mut online.room {
                let me = room.me;
                room.reface(me, avatar);
            }
        }
    }

    pub fn set_stream_quality(&mut self, max_height: u32, fps: u32) {
        if (self.config.share.max_height, self.config.share.fps) == (max_height, fps) {
            return;
        }
        self.config.share.max_height = max_height;
        self.config.share.fps = fps;
        self.save_config();
    }

    pub fn apply_devices(&mut self, input: Option<String>, output: Option<String>) {
        if (&self.config.input_device, &self.config.output_device) == (&input, &output) {
            return;
        }
        (self.config.input_device, self.config.output_device) = (input.clone(), output.clone());
        self.save_config();
        self.engine.use_devices(input, output);
    }

    pub fn start_mic_test(&mut self) {
        self.test_microphone(true);
    }

    pub fn stop_mic_test(&mut self) {
        self.test_microphone(false);
    }

    fn test_microphone(&mut self, on: bool) {
        if self.engine.is_testing() != on {
            self.engine.test_microphone(on);
            self.publish_state();
        }
    }

    fn publish_state(&self) {
        if let Stage::Online(online) = &self.stage {
            online.publish_state(self.engine.controls(), self.engine.is_testing());
        }
    }

    pub fn is_testing_microphone(&self) -> bool {
        self.engine.is_testing()
    }
}

impl Online {
    fn look(&self, heard: u64, missed: u64) -> Look {
        let (sent, lost_sending) = self.session.sent_and_lost();
        Look {
            rtt_ms: self.session.rtt().as_secs_f32() * 1000.0,
            sent,
            lost_sending,
            heard,
            missed,
        }
    }

    fn join(&mut self, joining: Joining) {
        self.session.send(ClientMsg::Join {
            room: joining.room.clone(),
            password: joining.password.clone(),
            rules: joining.rules.clone(),
        });
        self.joining = Some(joining);
    }

    fn publish_state(&self, controls: &Controls, testing: bool) {
        let state = VoiceState {
            muted: self.muted || self.deafened || testing,
            deafened: self.deafened || testing,
        };
        controls
            .muted
            .store(state.muted, std::sync::atomic::Ordering::Relaxed);
        controls
            .deafened
            .store(state.deafened, std::sync::atomic::Ordering::Relaxed);
        self.session.send(ClientMsg::State(state));
    }
}

fn remembered(recent: &RecentServer) -> Target {
    Target {
        label: recent.name.to_string(),
        ..Target::typed(recent.address.clone(), recent.id)
    }
}

#[cfg(test)]
mod tests {
    use whisperia_proto::control::RoomState;

    use super::*;

    fn resume(room: &str, said: &str) -> Resume {
        let mut chat = Chat::default();
        chat.note(About::Joined, said.into());
        Resume {
            room: RoomName::new(room).unwrap(),
            password: None,
            rules: RoomRules::default(),
            chat,
        }
    }

    #[test]
    fn a_drop_before_the_room_is_back_keeps_the_way_back() {
        let back = Resume::after_drop(None, Some(resume("Backroom", "ave 83"))).unwrap();
        assert_eq!(back.room.as_str(), "Backroom");
        assert_eq!(back.chat.lines()[0].text(), "ave 83");
    }

    #[test]
    fn a_drop_in_a_room_goes_back_to_that_room() {
        let mut room = InRoom::new(
            RoomName::new("General").unwrap(),
            Vec::new(),
            RoomState {
                host: 3,
                locked: false,
                limit: None,
            },
            3,
            None,
            resume("General", "hi").chat,
        );
        let back = Resume::after_drop(Some(&mut room), None).unwrap();
        assert_eq!(back.room.as_str(), "General");
        assert_eq!(back.chat.lines()[0].text(), "hi");
    }
}
