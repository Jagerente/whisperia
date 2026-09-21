use std::{
    sync::{mpsc::Receiver, Arc, Mutex},
    time::{Duration, Instant},
};

use whisperia_proto::{
    control::{
        Avatar, ClientMsg, Name, Password, PeerInfo, RoomLimit, RoomName, RoomState, ShareInfo,
    },
    PeerId,
};

use super::{
    chat::{About, Chat, Said},
    choosing::Choosing,
    message::Message,
    outbox::Outbox,
};
use crate::{
    config::ShareConfig,
    dsp::cue::Cue,
    engine::Engine,
    net::{OutgoingPicture, Session, SessionEvent},
    screen::{
        KeyframeAsks, PickedSource, PictureGate, ScreenBackend, ShareEnd, ShareHandle, ShareOut,
        ShareSound, SoundOut, SourceKey, SourceKind, SourceNews, Verdict, ViewerEvent,
        ViewerHandle,
    },
    wake::{self, Waker},
};

const ENDING_SHOWN: Duration = Duration::from_millis(2000);
const KEYFRAME_SPACING: Duration = Duration::from_millis(400);
const FIRST_PICTURE_PATIENCE: Duration = Duration::from_millis(1500);

pub struct Ending {
    pub sharer: PeerId,
    pub name: Name,
    pub avatar: Avatar,
    since: Instant,
}

pub struct InRoom {
    pub(crate) name: RoomName,
    pub(crate) peers: Vec<PeerInfo>,
    pub(crate) me: PeerId,
    pub(crate) host: PeerId,
    pub(crate) locked: bool,
    pub(crate) limit: Option<RoomLimit>,
    limit_asked: Option<Option<RoomLimit>>,
    pub(super) password: Option<Password>,
    pub(crate) chat: Chat,
    pub(crate) choosing: Option<Choosing>,
    pub(crate) share: Option<Sharing>,
    pub(super) watching: Option<Watching>,
    pub(crate) ending: Vec<Ending>,
}

enum ShareNews {
    Starved,
    Source(SourceNews),
    Ended(ShareEnd),
}

pub struct Sharing {
    pub(crate) handle: Box<dyn ShareHandle>,
    pub(crate) label: String,
    pub(crate) key: SourceKey,
    pub(crate) info: ShareInfo,
    settings: ShareConfig,
    pub(crate) viewers: Vec<PeerId>,
    keyframes: KeyframeAsks,
    news: Receiver<ShareNews>,
}

impl Sharing {
    fn want_keyframe(&mut self, now: Instant) {
        self.keyframes.want();
        self.ask_keyframe_if_due(now);
    }

    fn ask_keyframe_if_due(&mut self, now: Instant) {
        if self.keyframes.due(now) {
            self.handle.request_keyframe();
        }
    }
}

enum Seen {
    Picture,
    Gap,
}

pub(super) struct Watching {
    pub(super) sharer: PeerId,
    _viewer: Arc<dyn ViewerHandle>,
    events: Receiver<ViewerEvent>,
    seen: Receiver<Seen>,
    shown: bool,
    keyframes: KeyframeAsks,
    since: Instant,
}

impl InRoom {
    pub(super) fn new(
        name: RoomName,
        peers: Vec<PeerInfo>,
        state: RoomState,
        me: PeerId,
        password: Option<Password>,
        mut chat: Chat,
    ) -> Self {
        chat.belong_to(&peers, me);
        Self {
            name,
            peers,
            me,
            host: state.host,
            locked: state.locked,
            limit: state.limit,
            limit_asked: None,
            password,
            chat,
            choosing: None,
            share: None,
            watching: None,
            ending: Vec::new(),
        }
    }

    pub fn is_host(&self) -> bool {
        self.host == self.me
    }

    pub fn shown_limit(&self) -> Option<RoomLimit> {
        self.limit_asked.unwrap_or(self.limit)
    }

    pub(super) fn ask_limit(&mut self, limit: Option<RoomLimit>) -> bool {
        if self.shown_limit() == limit {
            return false;
        }
        self.limit_asked = Some(limit);
        true
    }

    pub fn peer(&self, id: PeerId) -> Option<&PeerInfo> {
        self.peers.iter().find(|p| p.id == id)
    }

    pub fn name_of(&self, id: PeerId) -> String {
        self.peer(id)
            .map_or_else(|| format!("peer {id}"), |p| p.name.to_string())
    }

    pub fn avatar_of(&self, id: PeerId) -> Avatar {
        self.peer(id).map_or(Avatar::None, |p| p.avatar)
    }

    pub fn is_watching(&self, sharer: PeerId) -> bool {
        self.watched() == Some(sharer)
    }

    pub fn watched(&self) -> Option<PeerId> {
        self.watching.as_ref().map(|w| w.sharer)
    }

    fn changed(&mut self, state: RoomState) {
        if self.limit_asked == Some(state.limit) || state.host != self.me {
            self.limit_asked = None;
        }
        if state.host != self.host {
            self.host = state.host;
            let who = match state.host == self.me {
                true => "You are".to_owned(),
                false => format!("{} is", self.name_of(state.host)),
            };
            self.note(About::Host, format!("{who} the host now"));
        }
        if state.locked != self.locked {
            self.locked = state.locked;
            match state.locked {
                true => self.note(About::Locked, "The room asks for a password now".into()),
                false => self.note(
                    About::Unlocked,
                    "The room no longer asks for a password".into(),
                ),
            }
        }
        if !state.locked {
            self.password = None;
        }
        if state.limit != self.limit {
            self.limit = state.limit;
            self.note(
                About::Size,
                match state.limit {
                    Some(most) => format!("At most {most} people can be in the room now"),
                    None => "Any number of people can be in the room now".to_owned(),
                },
            );
        }
    }

    fn share_over(&mut self, sharer: PeerId, keep_the_place: bool) {
        self.ending.retain(|ending| ending.sharer != sharer);
        if keep_the_place && self.is_watching(sharer) {
            if let Some(peer) = self.peer(sharer) {
                self.ending.push(Ending {
                    sharer,
                    name: peer.name.clone(),
                    avatar: peer.avatar,
                    since: Instant::now(),
                });
            }
        }
    }

    pub(super) fn begin_choosing(
        &mut self,
        screens: Arc<dyn ScreenBackend>,
        tab: SourceKind,
        shared_now: Option<SourceKey>,
        waker: &Waker,
    ) {
        self.choosing = Choosing::start(screens, tab, shared_now, waker);
    }

    pub(super) fn share_chosen(
        &mut self,
        id: i32,
        session: &Session,
        screens: &Arc<dyn ScreenBackend>,
        config: &ShareConfig,
        waker: &Waker,
        out: &mut Outbox,
    ) {
        let Some(mut choosing) = self.choosing.take() else {
            return;
        };
        let Some(source) = choosing.take(id) else {
            self.choosing = Some(choosing);
            return;
        };
        match &mut self.share {
            None => self.start_share(source, session, screens, config, waker, out),
            Some(share) if share.key == source.key => {
                if share.settings != *config {
                    share.handle.reconfigure(config);
                    share.settings = config.clone();
                }
            }
            Some(_) => self.switch_share(source, session, screens, config, waker, out),
        }
    }

    fn start_share(
        &mut self,
        source: PickedSource,
        session: &Session,
        screens: &Arc<dyn ScreenBackend>,
        config: &ShareConfig,
        waker: &Waker,
        out: &mut Outbox,
    ) {
        let Some((handle, info, news)) = self.show(&source, session, screens, config, waker, out)
        else {
            return;
        };
        out.cue(Cue::ShareStart);
        self.share = Some(Sharing {
            handle,
            label: source.label,
            key: source.key,
            info,
            settings: config.clone(),
            viewers: Vec::new(),
            keyframes: KeyframeAsks::new(KEYFRAME_SPACING),
            news,
        });
    }

    fn switch_share(
        &mut self,
        source: PickedSource,
        session: &Session,
        screens: &Arc<dyn ScreenBackend>,
        config: &ShareConfig,
        waker: &Waker,
        out: &mut Outbox,
    ) {
        let Some((handle, info, news)) = self.show(&source, session, screens, config, waker, out)
        else {
            return;
        };
        if let Some(share) = &mut self.share {
            share.handle = handle;
            share.info = info;
            share.news = news;
            share.label = source.label;
            share.key = source.key;
            share.settings = config.clone();
        }
    }

    fn show(
        &mut self,
        source: &PickedSource,
        session: &Session,
        screens: &Arc<dyn ScreenBackend>,
        config: &ShareConfig,
        waker: &Waker,
        out: &mut Outbox,
    ) -> Option<(Box<dyn ShareHandle>, ShareInfo, Receiver<ShareNews>)> {
        let (wiring, news) = wire(session, waker);
        let handle = match screens.share(source.key, config, wiring) {
            Ok(handle) => handle,
            Err(e) => {
                out.error(format!("Cannot share the screen: {e}"));
                return None;
            }
        };
        let has_sound = handle.has_sound();
        let info = ShareInfo {
            sound: has_sound,
            minimized: false,
        };
        session.open_media();
        session.send(ClientMsg::ShareStart(info));
        tracing::info!(encoder = %handle.details(), has_sound, "showing the screen");
        self.note(About::Sharing, format!("You are sharing {}", source.label));
        if !has_sound {
            out.info("Sharing without sound: this Windows cannot record it.");
        }
        Some((handle, info, news))
    }

    pub(super) fn stop_share(&mut self, session: &Session, out: &mut Outbox) {
        if self.share.take().is_some() {
            session.send(ClientMsg::ShareStop);
            self.note(
                About::StoppedSharing,
                "You stopped sharing your screen".into(),
            );
            out.cue(Cue::ShareStop);
        }
    }

    pub(super) fn start_watching(
        &mut self,
        sharer: PeerId,
        session: &Session,
        screens: &Arc<dyn ScreenBackend>,
        waker: &Waker,
        out: &mut Outbox,
    ) {
        if self.is_watching(sharer) {
            return;
        }
        self.stop_watching(session);
        let (events_tx, events) = wake::channel(waker);
        let viewer = match screens.view(sharer, events_tx) {
            Ok(viewer) => viewer,
            Err(e) => return out.error(format!("Cannot watch: {e}")),
        };

        let (seen_tx, seen) = wake::channel(waker);
        let gate = Mutex::new((PictureGate::default(), None));
        let window = viewer.clone();
        session.set_picture_sink(
            sharer,
            Some(Box::new(move |picture| {
                let mut gate = gate.lock().unwrap_or_else(|p| p.into_inner());
                let (gate, last) = &mut *gate;
                let verdict = gate.admit(picture.seq, picture.keyframe);
                if verdict == Verdict::Show {
                    window.push(picture.data, picture.keyframe);
                }
                if last.replace(verdict) != Some(verdict) {
                    let _ = seen_tx.send(match verdict {
                        Verdict::Show => Seen::Picture,
                        Verdict::Skip => Seen::Gap,
                    });
                }
            })),
        );
        session.open_media();
        session.send(ClientMsg::Watch(sharer));
        self.watching = Some(Watching {
            sharer,
            _viewer: viewer,
            events,
            seen,
            shown: false,
            keyframes: KeyframeAsks::new(KEYFRAME_SPACING),
            since: Instant::now(),
        });
    }

    pub(super) fn stop_watching(&mut self, session: &Session) {
        if let Some(watching) = self.watching.take() {
            session.set_picture_sink(watching.sharer, None);
            session.send(ClientMsg::Unwatch(watching.sharer));
        }
    }

    pub(super) fn forget_screens(&mut self, session: &Session) {
        if let Some(watching) = self.watching.take() {
            session.set_picture_sink(watching.sharer, None);
        }
        self.share = None;
        self.choosing = None;
        self.ending.clear();
    }

    pub(super) fn tick_screens(&mut self, now: Instant, session: &Session, out: &mut Outbox) {
        match self.choosing.as_mut().map(Choosing::tick) {
            Some(Ok(true)) => out.changed(),
            Some(Err(e)) => {
                self.choosing = None;
                out.error(format!("Cannot share the screen: {e}"));
            }
            Some(Ok(false)) | None => {}
        }

        let before = self.ending.len();
        self.ending
            .retain(|ending| now.duration_since(ending.since) < ENDING_SHOWN);
        if self.ending.len() != before {
            out.changed();
        }
        if let Some(share) = &mut self.share {
            let mut ended = None;
            let before = share.info;
            for news in share.news.try_iter() {
                match news {
                    ShareNews::Starved => share.keyframes.want(),
                    ShareNews::Source(SourceNews::Resized(size)) => {
                        share.handle.follow(size);
                        out.changed();
                    }
                    ShareNews::Source(SourceNews::Minimized) => share.info.minimized = true,
                    ShareNews::Source(SourceNews::Restored) => share.info.minimized = false,
                    ShareNews::Ended(end) => ended = Some(end),
                }
            }
            if share.info != before {
                session.send(ClientMsg::ShareStart(share.info));
                out.changed();
            }
            match ended {
                Some(end) => {
                    out.info(format!("Stopped sharing: {end}."));
                    self.stop_share(session, out);
                }
                None => share.ask_keyframe_if_due(now),
            }
        }

        let Some(watching) = &mut self.watching else {
            return;
        };
        let mut closed = false;
        for event in watching.events.try_iter() {
            match event {
                ViewerEvent::NeedKeyframe => watching.keyframes.want(),
                ViewerEvent::Failed(why) => {
                    out.error(format!("Cannot show what is shared: {why}"));
                    closed = true;
                }
            }
        }
        if closed {
            self.stop_watching(session);
            out.changed();
            return;
        }
        for seen in watching.seen.try_iter() {
            match seen {
                Seen::Picture => watching.shown = true,
                Seen::Gap => watching.keyframes.want(),
            }
        }
        if !watching.shown && now.duration_since(watching.since) > FIRST_PICTURE_PATIENCE {
            watching.since = now;
            watching.keyframes.want();
        }
        if watching.keyframes.due(now) {
            session.send(ClientMsg::NeedKeyframe(watching.sharer));
        }
    }

    pub(super) fn on_event(
        &mut self,
        event: SessionEvent,
        session: &Session,
        engine: &Engine,
        out: &mut Outbox,
    ) {
        match event {
            SessionEvent::PeerJoined(peer) => {
                self.peers.retain(|p| p.id != peer.id);
                self.note(About::Joined, format!("{} joined", peer.name));
                self.peers.push(peer);
                out.cue(Cue::Join);
            }
            SessionEvent::PeerLeft(id) | SessionEvent::PeerKicked(id) => {
                engine.forget(id);
                if let Some(peer) = self.peer(id) {
                    let (about, note) = match (&event, self.is_host()) {
                        (SessionEvent::PeerLeft(_), _) => {
                            (About::Left, format!("{} left", peer.name))
                        }
                        (_, true) => (About::Kicked, format!("You kicked {}", peer.name)),
                        (_, false) => (
                            About::Kicked,
                            format!("{} kicked {}", self.name_of(self.host), peer.name),
                        ),
                    };
                    self.note(about, note);
                    out.cue(Cue::Leave);
                }
                self.share_over(id, true);
                if self.is_watching(id) {
                    self.stop_watching(session);
                }
                self.peers.retain(|p| p.id != id);
            }
            SessionEvent::PeerShare { id, share } => {
                let Some(peer) = self.peers.iter_mut().find(|p| p.id == id) else {
                    return;
                };
                let (name, before) = (peer.name.clone(), peer.share);
                peer.share = share;
                match (before, share) {
                    (None, Some(_)) => {
                        self.share_over(id, false);
                        self.note(About::Sharing, format!("{name} is sharing their screen"));
                        out.cue(Cue::ShareStart);
                    }
                    (Some(_), None) => {
                        engine.forget_share(id);
                        self.note(
                            About::StoppedSharing,
                            format!("{name} stopped sharing their screen"),
                        );
                        out.cue(Cue::ShareStop);
                        self.share_over(id, true);
                        if self.is_watching(id) {
                            self.stop_watching(session);
                        }
                    }
                    _ => {}
                }
            }
            SessionEvent::Viewers(viewers) => {
                if let Some(share) = &mut self.share {
                    share.viewers = viewers;
                }
            }
            SessionEvent::KeyframeWanted => {
                if let Some(share) = &mut self.share {
                    share.want_keyframe(Instant::now());
                }
            }
            SessionEvent::PeerState { id, state } => {
                if let Some(peer) = self.peers.iter_mut().find(|p| p.id == id) {
                    peer.state = state;
                }
            }
            SessionEvent::PeerAvatar { id, avatar } => {
                if let Some(peer) = self.peers.iter_mut().find(|p| p.id == id) {
                    peer.avatar = avatar;
                }
                self.reface(id, avatar);
            }
            SessionEvent::PeerRenamed { id, name } => {
                if let Some(peer) = self.peers.iter_mut().find(|p| p.id == id) {
                    let before = std::mem::replace(&mut peer.name, name.clone());
                    self.note(About::Renamed, format!("{before} is now {name}"));
                }
            }
            SessionEvent::Chat { from, text } => {
                let Some(who) = self.peer(from).map(|p| p.name.clone()) else {
                    return tracing::warn!("a message from {from}, who is not in the room");
                };
                let avatar = self.avatar_of(from);
                let message = Message::from(&text);
                out.unfurl(&message);
                self.say(Said {
                    from: Some(from),
                    who,
                    avatar,
                    message,
                    mine: false,
                });
                out.cue(Cue::Message);
            }
            SessionEvent::RoomChanged(state) => self.changed(state),
            SessionEvent::Rooms(_)
            | SessionEvent::Joined { .. }
            | SessionEvent::JoinRefused(_)
            | SessionEvent::Kicked
            | SessionEvent::Closed(_) => {}
        }
    }

    pub(super) fn say(&mut self, said: Said) {
        self.chat.say(said);
    }

    pub(super) fn note(&mut self, about: About, text: String) {
        self.chat.note(about, text);
    }

    pub(super) fn reface(&mut self, id: PeerId, avatar: Avatar) {
        self.chat.reface(id, avatar);
        for ending in &mut self.ending {
            if ending.sharer == id {
                ending.avatar = avatar;
            }
        }
    }
}

fn wire(session: &Session, waker: &Waker) -> (ShareOut, Receiver<ShareNews>) {
    let (news_tx, news) = wake::channel(waker);
    let queue = session.picture_sender();
    let starved = news_tx.clone();
    let mut broken = false;
    let pictures = Box::new(move |picture: OutgoingPicture| {
        if broken && !picture.keyframe {
            return;
        }
        broken = queue.try_send(picture).is_err();
        if broken {
            let _ = starved.send(ShareNews::Starved);
        }
    });
    let sound: Option<SoundOut> = ShareSound::new(session.audio_sender())
        .map_err(|e| tracing::warn!("share sound: {e}"))
        .ok()
        .map(|mut sound| Box::new(move |samples: &[f32]| sound.push(samples)) as SoundOut);
    let heard = news_tx.clone();
    let wiring = ShareOut {
        pictures,
        sound,
        ended: Box::new(move |end| {
            let _ = news_tx.send(ShareNews::Ended(end));
        }),
        news: Box::new(move |news| {
            let _ = heard.send(ShareNews::Source(news));
        }),
    };
    (wiring, news)
}

#[cfg(test)]
mod tests {
    use whisperia_proto::control::VoiceState;

    use super::{super::chat::ChatLine, *};

    fn notes(room: &InRoom) -> Vec<&str> {
        room.chat.lines().iter().map(ChatLine::text).collect()
    }

    fn state(host: PeerId, locked: bool, limit: Option<u8>) -> RoomState {
        RoomState {
            host,
            locked,
            limit: limit.map(RoomLimit::new),
        }
    }

    fn room(peers: Vec<PeerInfo>, at: RoomState, me: PeerId) -> InRoom {
        InRoom::new(
            RoomName::new("Backroom").unwrap(),
            peers,
            at,
            me,
            Password::new("owl").ok(),
            Chat::default(),
        )
    }

    #[test]
    fn an_asked_limit_is_shown_until_the_server_says_it_back() {
        let mut room = room(Vec::new(), state(3, false, None), 3);
        let two = Some(RoomLimit::new(2));
        assert!(!room.ask_limit(None), "nothing to ask");
        assert!(room.ask_limit(two));
        assert_eq!((room.shown_limit(), room.limit), (two, None));
        assert!(room.ask_limit(None), "changed its mind before the answer");
        room.changed(state(3, false, Some(2)));
        assert_eq!(room.shown_limit(), None, "the first answer is already old");
        room.changed(state(3, false, None));
        assert_eq!((room.shown_limit(), room.limit), (None, None));

        assert!(room.ask_limit(Some(RoomLimit::new(4))));
        room.changed(state(9, false, None));
        assert_eq!(
            room.shown_limit(),
            None,
            "whoever holds the crown now decides"
        );
    }

    #[test]
    fn the_room_says_who_hosts_it_and_what_the_host_changed() {
        let fox = PeerInfo {
            id: 7,
            name: Name::new("Fox").unwrap(),
            avatar: Avatar::Fox,
            state: VoiceState::default(),
            share: None,
        };
        let mut room = room(vec![fox], state(7, true, Some(4)), 3);
        assert!(!room.is_host());

        room.changed(state(7, true, Some(4)));
        assert!(notes(&room).is_empty(), "a new password says nothing");
        room.changed(state(3, true, Some(4)));
        assert!(room.is_host());
        room.changed(state(3, true, None));
        room.changed(state(3, false, None));
        assert_eq!(room.password, None);
        room.changed(state(7, false, Some(2)));
        assert_eq!(
            notes(&room),
            [
                "You are the host now",
                "Any number of people can be in the room now",
                "The room no longer asks for a password",
                "Fox is the host now",
                "At most 2 people can be in the room now",
            ]
        );
    }
}
