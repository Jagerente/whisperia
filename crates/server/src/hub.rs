use std::collections::{BTreeMap, BTreeSet};

use tokio::sync::mpsc::UnboundedSender;
use whisperia_proto::{
    audio::AudioKind,
    control::{
        Avatar, ChatText, JoinError, Name, Password, PeerInfo, RoomLimit, RoomName, RoomPerson,
        RoomRules, RoomState, RoomSummary, ServerMsg, ShareInfo, VoiceState, MAX_NAME_CHARS,
        MAX_ROOMS,
    },
    PeerId,
};

struct Peer<C> {
    info: PeerInfo,
    asked: Name,
    conn: C,
    control: UnboundedSender<ServerMsg>,
    room: Option<RoomName>,
    entered: u64,
    media_token: u64,
    media: Option<C>,
    watching: BTreeSet<PeerId>,
}

struct Room {
    host: PeerId,
    password: Option<Password>,
    limit: Option<RoomLimit>,
}

impl Room {
    fn state(&self) -> RoomState {
        RoomState {
            host: self.host,
            locked: self.password.is_some(),
            limit: self.limit,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Occupancy {
    pub rooms: usize,
    pub people: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Admitted {
    pub id: PeerId,
    pub name: Name,
    pub rooms: Vec<RoomSummary>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct HubFull;

pub struct Hub<C> {
    peers: BTreeMap<PeerId, Peer<C>>,
    rooms: BTreeMap<RoomName, Room>,
    entries: u64,
    last_id: PeerId,
    capacity: usize,
    announced: Occupancy,
}

impl<C> Hub<C> {
    pub fn new(capacity: usize) -> Self {
        Self {
            peers: BTreeMap::new(),
            rooms: BTreeMap::new(),
            entries: 0,
            last_id: 0,
            capacity,
            announced: Occupancy::default(),
        }
    }

    pub fn connect(
        &mut self,
        name: &Name,
        avatar: Avatar,
        conn: C,
        control: UnboundedSender<ServerMsg>,
        media_token: u64,
    ) -> Result<Admitted, HubFull> {
        if self.peers.len() >= self.capacity {
            return Err(HubFull);
        }
        let id = self.free_id();
        let asked = name.clone();
        let name = self.unique_name(name, None);
        let info = PeerInfo {
            id,
            name: name.clone(),
            avatar,
            state: VoiceState::default(),
            share: None,
        };
        self.peers.insert(
            id,
            Peer {
                info,
                asked,
                conn,
                control,
                room: None,
                entered: 0,
                media_token,
                media: None,
                watching: BTreeSet::new(),
            },
        );
        Ok(Admitted {
            id,
            name,
            rooms: self.rooms(),
        })
    }

    pub fn attach_media(&mut self, id: PeerId, token: u64, media: C) -> bool {
        let watched: Vec<PeerId> = match self.peers.get_mut(&id) {
            Some(peer) if peer.media_token == token => {
                peer.media = Some(media);
                peer.watching.iter().copied().collect()
            }
            _ => return false,
        };
        watched
            .into_iter()
            .for_each(|sharer| self.keyframe_wanted(sharer));
        true
    }

    pub fn detach_media(&mut self, id: PeerId, is_same: impl Fn(&C) -> bool) {
        if let Some(peer) = self.peers.get_mut(&id) {
            if peer.media.as_ref().is_some_and(is_same) {
                peer.media = None;
            }
        }
    }

    pub fn share_start(&mut self, id: PeerId, info: ShareInfo) {
        let Some(peer) = self.peers.get_mut(&id) else {
            return;
        };
        let Some(room) = peer.room.clone() else {
            return;
        };
        if peer.info.share == Some(info) {
            return;
        }
        peer.info.share = Some(info);
        let msg = ServerMsg::PeerShare {
            id,
            share: Some(info),
        };
        self.tell_room(&room, Some(id), &msg);
    }

    pub fn share_stop(&mut self, id: PeerId) {
        let Some(peer) = self.peers.get_mut(&id) else {
            return;
        };
        if peer.info.share.take().is_none() {
            return;
        }
        let room = peer.room.clone();
        for other in self.peers.values_mut() {
            other.watching.remove(&id);
        }
        if let Some(room) = room {
            self.tell_room(&room, Some(id), &ServerMsg::PeerShare { id, share: None });
        }
    }

    pub fn watch(&mut self, id: PeerId, sharer: PeerId) -> bool {
        let showing = self
            .peers
            .get(&sharer)
            .filter(|s| s.info.share.is_some())
            .and_then(|s| s.room.clone());
        let Some(peer) = self.peers.get_mut(&id) else {
            return false;
        };
        if id == sharer || showing.is_none() || peer.room != showing {
            return false;
        }
        if peer.watching.insert(sharer) {
            self.tell_viewers(sharer);
        }
        self.keyframe_wanted(sharer);
        true
    }

    pub fn unwatch(&mut self, id: PeerId, sharer: PeerId) {
        if self
            .peers
            .get_mut(&id)
            .is_some_and(|p| p.watching.remove(&sharer))
        {
            self.tell_viewers(sharer);
        }
    }

    pub fn need_keyframe(&self, id: PeerId, sharer: PeerId) {
        if self
            .peers
            .get(&id)
            .is_some_and(|p| p.watching.contains(&sharer))
        {
            self.keyframe_wanted(sharer);
        }
    }

    pub fn keyframe_wanted(&self, sharer: PeerId) {
        if let Some(peer) = self.peers.get(&sharer) {
            let _ = peer.control.send(ServerMsg::KeyframeWanted);
        }
    }

    pub fn picture_targets(&self, sharer: PeerId) -> Vec<(PeerId, C)>
    where
        C: Clone,
    {
        if self
            .peers
            .get(&sharer)
            .is_none_or(|s| s.info.share.is_none())
        {
            return Vec::new();
        }
        self.peers
            .iter()
            .filter(|(_, p)| p.watching.contains(&sharer))
            .filter_map(|(id, p)| Some((*id, p.media.clone()?)))
            .collect()
    }

    fn viewers(&self, sharer: PeerId) -> Vec<PeerId> {
        self.peers
            .iter()
            .filter(|(_, p)| p.watching.contains(&sharer))
            .map(|(id, _)| *id)
            .collect()
    }

    fn tell_viewers(&self, sharer: PeerId) {
        if let Some(peer) = self.peers.get(&sharer) {
            let _ = peer.control.send(ServerMsg::Viewers(self.viewers(sharer)));
        }
    }

    pub fn join(
        &mut self,
        id: PeerId,
        room: &RoomName,
        password: Option<&Password>,
        rules: Option<RoomRules>,
    ) -> Result<(Vec<PeerInfo>, RoomState), JoinError> {
        let Some(peer) = self.peers.get(&id) else {
            return Err(JoinError::NotConnected);
        };
        if peer.room.as_ref() == Some(room) {
            if let Some(open) = self.rooms.get(room) {
                return Ok((self.others_in(room, id), open.state()));
            }
        }
        match self.rooms.get(room) {
            Some(open) => {
                if open.password.is_some() && open.password.as_ref() != password {
                    return Err(JoinError::Password);
                }
                let inside = self.members(room).filter(|p| p.info.id != id).count();
                if open
                    .limit
                    .is_some_and(|limit| inside >= usize::from(limit.get()))
                {
                    return Err(JoinError::Full);
                }
            }
            None if self.rooms.len() >= MAX_ROOMS => return Err(JoinError::TooManyRooms),
            None => {}
        }
        self.step_out(id, ServerMsg::PeerLeft(id));

        let rules = rules.unwrap_or_default();
        let state = self
            .rooms
            .entry(room.clone())
            .or_insert_with(|| Room {
                host: id,
                password: rules.password,
                limit: rules.limit,
            })
            .state();
        let others = self.others_in(room, id);
        self.entries += 1;
        let info = match self.peers.get_mut(&id) {
            Some(peer) => {
                peer.room = Some(room.clone());
                peer.entered = self.entries;
                peer.info.clone()
            }
            None => return Err(JoinError::NotConnected),
        };
        self.tell_room(room, Some(id), &ServerMsg::PeerJoined(info));
        self.tell_everyone(&ServerMsg::Rooms(self.rooms()));
        Ok((others, state))
    }

    pub fn set_room_password(&mut self, id: PeerId, password: Option<Password>) {
        self.change_room(id, |room| room.password = password);
    }

    pub fn set_room_limit(&mut self, id: PeerId, limit: Option<RoomLimit>) {
        self.change_room(id, |room| room.limit = limit);
    }

    pub fn make_host(&mut self, id: PeerId, heir: PeerId) {
        let same_room = self.peers.get(&heir).and_then(|p| p.room.as_ref())
            == self.peers.get(&id).and_then(|p| p.room.as_ref());
        if id != heir && same_room {
            self.change_room(id, |room| room.host = heir);
        }
    }

    pub fn kick(&mut self, id: PeerId, target: PeerId) -> bool {
        let room = self.peers.get(&id).and_then(|p| p.room.as_ref());
        let hosts = room
            .and_then(|room| self.rooms.get(room))
            .is_some_and(|room| room.host == id);
        let same_room = self.peers.get(&target).and_then(|p| p.room.as_ref()) == room;
        if id == target || !hosts || !same_room {
            return false;
        }
        if let Some(peer) = self.peers.get(&target) {
            let _ = peer.control.send(ServerMsg::Kicked);
        }
        self.step_out(target, ServerMsg::PeerKicked(target));
        self.tell_everyone(&ServerMsg::Rooms(self.rooms()));
        true
    }

    fn change_room(&mut self, id: PeerId, change: impl FnOnce(&mut Room)) {
        let Some(name) = self.peers.get(&id).and_then(|p| p.room.clone()) else {
            return;
        };
        let Some(room) = self.rooms.get_mut(&name).filter(|room| room.host == id) else {
            return;
        };
        change(room);
        let state = room.state();
        self.tell_room(&name, None, &ServerMsg::RoomChanged(state));
        self.tell_everyone(&ServerMsg::Rooms(self.rooms()));
    }

    pub fn leave(&mut self, id: PeerId) {
        if self.step_out(id, ServerMsg::PeerLeft(id)) {
            self.tell_everyone(&ServerMsg::Rooms(self.rooms()));
        }
    }

    pub fn disconnect(&mut self, id: PeerId) -> Option<Name> {
        let left_a_room = self.step_out(id, ServerMsg::PeerLeft(id));
        let peer = self.peers.remove(&id)?;
        if left_a_room {
            self.tell_everyone(&ServerMsg::Rooms(self.rooms()));
        }
        self.give_back(&peer.info.name);
        Some(peer.info.name)
    }

    fn give_back(&mut self, freed: &Name) {
        let waiting = self
            .peers
            .iter()
            .find(|(_, p)| p.asked == *freed && p.info.name != *freed)
            .map(|(id, _)| *id);
        if let Some(id) = waiting {
            self.rename(id, freed);
        }
    }

    pub fn set_state(&mut self, id: PeerId, state: VoiceState) {
        let Some(peer) = self.peers.get_mut(&id) else {
            return;
        };
        if peer.info.state == state {
            return;
        }
        peer.info.state = state;
        if let Some(room) = peer.room.clone() {
            self.tell_room(&room, Some(id), &ServerMsg::PeerState { id, state });
        }
    }

    pub fn rename(&mut self, id: PeerId, wanted: &Name) {
        let name = self.unique_name(wanted, Some(id));
        let Some(peer) = self.peers.get_mut(&id) else {
            return;
        };
        peer.asked = wanted.clone();
        if peer.info.name == name {
            return;
        }
        let freed = std::mem::replace(&mut peer.info.name, name.clone());
        let renamed = ServerMsg::PeerRenamed { id, name };
        match peer.room.clone() {
            Some(room) => {
                self.tell_room(&room, None, &renamed);
                self.tell_everyone(&ServerMsg::Rooms(self.rooms()));
            }
            None => {
                let _ = peer.control.send(renamed);
            }
        }
        self.give_back(&freed);
    }

    pub fn set_avatar(&mut self, id: PeerId, avatar: Avatar) {
        let Some(peer) = self.peers.get_mut(&id) else {
            return;
        };
        if peer.info.avatar == avatar {
            return;
        }
        peer.info.avatar = avatar;
        if let Some(room) = peer.room.clone() {
            self.tell_room(&room, Some(id), &ServerMsg::PeerAvatar { id, avatar });
            self.tell_everyone(&ServerMsg::Rooms(self.rooms()));
        }
    }

    pub fn chat(&self, from: PeerId, text: ChatText) {
        if let Some(room) = self.peers.get(&from).and_then(|p| p.room.as_ref()) {
            self.tell_room(room, Some(from), &ServerMsg::Chat { from, text });
        }
    }

    pub fn audio_targets(&self, from: PeerId, kind: AudioKind) -> impl Iterator<Item = &C> {
        let sender = self.peers.get(&from);
        let audible = sender.is_some_and(|p| match kind {
            AudioKind::Voice => !p.info.state.muted && p.room.is_some(),
            AudioKind::Share => p.info.share.is_some_and(|share| share.sound),
        });
        let room = sender.and_then(|p| p.room.as_ref());
        self.peers
            .iter()
            .filter(move |(id, p)| {
                audible
                    && **id != from
                    && p.room.as_ref() == room
                    && !p.info.state.deafened
                    && (kind == AudioKind::Voice || p.watching.contains(&from))
            })
            .map(|(_, p)| &p.conn)
    }

    pub fn rooms(&self) -> Vec<RoomSummary> {
        self.rooms
            .iter()
            .map(|(name, room)| {
                let locked = room.password.is_some();
                let people = self
                    .members(name)
                    .filter(|_| !locked)
                    .map(|peer| RoomPerson {
                        name: peer.info.name.clone(),
                        avatar: peer.info.avatar,
                        host: peer.info.id == room.host,
                    })
                    .collect();
                RoomSummary {
                    name: name.clone(),
                    people,
                    locked,
                    limit: room.limit.filter(|_| !locked),
                }
            })
            .collect()
    }

    pub fn occupancy(&self) -> Occupancy {
        Occupancy {
            rooms: self.rooms.len(),
            people: self.peers.len(),
        }
    }

    pub fn occupancy_change(&mut self) -> Option<Occupancy> {
        let now = self.occupancy();
        (now != self.announced).then(|| {
            self.announced = now;
            now
        })
    }

    fn step_out(&mut self, id: PeerId, told: ServerMsg) -> bool {
        let Some(peer) = self.peers.get_mut(&id) else {
            return false;
        };
        let Some(room) = peer.room.take() else {
            return false;
        };
        peer.info.share = None;
        let watched = std::mem::take(&mut peer.watching);
        for other in self.peers.values_mut() {
            other.watching.remove(&id);
        }

        self.tell_room(&room, None, &told);
        watched
            .into_iter()
            .for_each(|sharer| self.tell_viewers(sharer));

        let heir = self
            .members(&room)
            .min_by_key(|p| p.entered)
            .map(|p| p.info.id);
        match (heir, self.rooms.get_mut(&room)) {
            (None, _) => {
                self.rooms.remove(&room);
            }
            (Some(heir), Some(open)) if open.host == id => {
                open.host = heir;
                let state = open.state();
                self.tell_room(&room, None, &ServerMsg::RoomChanged(state));
            }
            _ => {}
        }
        true
    }

    fn members<'a>(&'a self, room: &'a RoomName) -> impl Iterator<Item = &'a Peer<C>> + 'a {
        self.peers
            .values()
            .filter(move |p| p.room.as_ref() == Some(room))
    }

    fn others_in(&self, room: &RoomName, id: PeerId) -> Vec<PeerInfo> {
        self.members(room)
            .filter(|p| p.info.id != id)
            .map(|p| p.info.clone())
            .collect()
    }

    fn tell_room(&self, room: &RoomName, except: Option<PeerId>, msg: &ServerMsg) {
        for peer in self.members(room) {
            if Some(peer.info.id) != except {
                let _ = peer.control.send(msg.clone());
            }
        }
    }

    fn tell_everyone(&self, msg: &ServerMsg) {
        for peer in self.peers.values() {
            let _ = peer.control.send(msg.clone());
        }
    }

    fn free_id(&mut self) -> PeerId {
        loop {
            self.last_id = self.last_id.checked_add(1).unwrap_or(1);
            if !self.peers.contains_key(&self.last_id) {
                return self.last_id;
            }
        }
    }

    fn unique_name(&self, wanted: &Name, except: Option<PeerId>) -> Name {
        let taken = |n: &str| {
            self.peers
                .iter()
                .any(|(id, p)| Some(*id) != except && p.info.name == n)
        };
        if !taken(wanted) {
            return wanted.clone();
        }
        (2..)
            .filter_map(|i| {
                let number = format!(" ({i})");
                let room_left = MAX_NAME_CHARS.saturating_sub(number.chars().count());
                let base: String = wanted.chars().take(room_left).collect();
                Name::new(&format!("{}{number}", base.trim_end())).ok()
            })
            .find(|n| !taken(n))
            .unwrap_or_else(|| wanted.clone())
    }
}

#[cfg(test)]
mod tests;
