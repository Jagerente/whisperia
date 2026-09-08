use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub use crate::text::{
    ChatText, Empty, Name, Password, RoomLimit, RoomName, MAX_CHAT_CHARS, MAX_NAME_CHARS,
    MAX_PASSWORD_CHARS,
};
use crate::PeerId;

pub const MAX_FRAME: usize = 16 * 1024;
pub const MAX_PEERS: usize = 64;
pub const MAX_ROOMS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello {
        protocol: u16,
        name: Name,
        avatar: Avatar,
        password: Option<Password>,
    },
    Join {
        room: RoomName,
        password: Option<Password>,
        rules: Option<RoomRules>,
    },
    Leave,
    State(VoiceState),
    Rename(Name),
    SetAvatar(Avatar),
    Chat(ChatText),
    ShareStart(ShareInfo),
    ShareStop,
    Watch(PeerId),
    Unwatch(PeerId),
    NeedKeyframe(PeerId),
    SetRoomPassword(Option<Password>),
    SetRoomLimit(Option<RoomLimit>),
    MakeHost(PeerId),
    Kick(PeerId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        id: PeerId,
        name: Name,
        server_name: Name,
        rooms: Vec<RoomSummary>,
        media_token: u64,
    },
    Rejected(RejectReason),
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
    PeerKicked(PeerId),
    Kicked,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceState {
    pub muted: bool,
    pub deafened: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareInfo {
    pub sound: bool,
    pub minimized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: PeerId,
    pub name: Name,
    pub avatar: Avatar,
    pub state: VoiceState,
    pub share: Option<ShareInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomRules {
    pub password: Option<Password>,
    pub limit: Option<RoomLimit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomState {
    pub host: PeerId,
    pub locked: bool,
    pub limit: Option<RoomLimit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSummary {
    pub name: RoomName,
    pub people: Vec<RoomPerson>,
    pub locked: bool,
    pub limit: Option<RoomLimit>,
}

impl RoomSummary {
    pub fn is_full(&self) -> bool {
        self.limit
            .is_some_and(|limit| self.people.len() >= usize::from(limit.get()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomPerson {
    pub name: Name,
    pub avatar: Avatar,
    pub host: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Avatar {
    #[default]
    None,
    Fox,
    Cat,
    Dog,
    Bear,
    Panda,
    Koala,
    Tiger,
    Lion,
    Monkey,
    Hamster,
    Rabbit,
    Mouse,
    Cow,
    Pig,
    Frog,
    Chick,
    Penguin,
    Owl,
    Duck,
    Wolf,
    Raccoon,
    Hedgehog,
    Unicorn,
    Octopus,
    Whale,
    Shark,
    Turtle,
    Butterfly,
    Bee,
}

impl Avatar {
    pub const ALL: [Self; 30] = [
        Self::None,
        Self::Fox,
        Self::Cat,
        Self::Dog,
        Self::Bear,
        Self::Panda,
        Self::Koala,
        Self::Tiger,
        Self::Lion,
        Self::Monkey,
        Self::Hamster,
        Self::Rabbit,
        Self::Mouse,
        Self::Cow,
        Self::Pig,
        Self::Frog,
        Self::Chick,
        Self::Penguin,
        Self::Owl,
        Self::Duck,
        Self::Wolf,
        Self::Raccoon,
        Self::Hedgehog,
        Self::Unicorn,
        Self::Octopus,
        Self::Whale,
        Self::Shark,
        Self::Turtle,
        Self::Butterfly,
        Self::Bee,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fox => "fox",
            Self::Cat => "cat",
            Self::Dog => "dog",
            Self::Bear => "bear",
            Self::Panda => "panda",
            Self::Koala => "koala",
            Self::Tiger => "tiger",
            Self::Lion => "lion",
            Self::Monkey => "monkey",
            Self::Hamster => "hamster",
            Self::Rabbit => "rabbit",
            Self::Mouse => "mouse",
            Self::Cow => "cow",
            Self::Pig => "pig",
            Self::Frog => "frog",
            Self::Chick => "chick",
            Self::Penguin => "penguin",
            Self::Owl => "owl",
            Self::Duck => "duck",
            Self::Wolf => "wolf",
            Self::Raccoon => "raccoon",
            Self::Hedgehog => "hedgehog",
            Self::Unicorn => "unicorn",
            Self::Octopus => "octopus",
            Self::Whale => "whale",
            Self::Shark => "shark",
            Self::Turtle => "turtle",
            Self::Butterfly => "butterfly",
            Self::Bee => "bee",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|animal| animal.key() == key)
            .unwrap_or(Self::None)
    }
}

impl From<String> for Avatar {
    fn from(key: String) -> Self {
        Self::from_key(&key)
    }
}

impl From<Avatar> for String {
    fn from(avatar: Avatar) -> Self {
        avatar.key().to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinError {
    NotConnected,
    TooManyRooms,
    Password,
    Full,
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => f.write_str("not connected to the server"),
            Self::TooManyRooms => write!(f, "the server already has {MAX_ROOMS} rooms"),
            Self::Password => f.write_str("the room asks for another password"),
            Self::Full => f.write_str("the room is full"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    ProtocolMismatch { server: u16 },
    BadPassword,
    ServerFull,
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProtocolMismatch { server } => write!(
                f,
                "version mismatch: server speaks protocol {server}, this client {} - update the older one",
                crate::PROTOCOL_VERSION
            ),
            Self::BadPassword => f.write_str("wrong or missing password"),
            Self::ServerFull => f.write_str("server is full"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("control message of {0} bytes exceeds the limit")]
    TooLarge(usize),
    #[error("malformed control message: {0}")]
    Malformed(#[from] postcard::Error),
}

pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = postcard::to_stdvec(msg)?;
    if body.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(2 + body.len());
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn next_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>, FrameError> {
        if self.buf.len() < 2 {
            return Ok(None);
        }
        let len = u16::from_be_bytes([self.buf[0], self.buf[1]]) as usize;
        if len > MAX_FRAME {
            return Err(FrameError::TooLarge(len));
        }
        if self.buf.len() < 2 + len {
            return Ok(None);
        }
        let msg = postcard::from_bytes(&self.buf[2..2 + len]);
        self.buf.drain(..2 + len);
        Ok(Some(msg?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> Name {
        Name::new(text).unwrap()
    }

    #[test]
    fn frames_survive_arbitrary_chunking() {
        let a = ClientMsg::Hello {
            protocol: 1,
            name: name("Panda"),
            avatar: Avatar::Panda,
            password: None,
        };
        let b = ClientMsg::State(VoiceState {
            muted: true,
            deafened: false,
        });
        let mut bytes = encode_frame(&a).unwrap();
        bytes.extend(encode_frame(&b).unwrap());

        let mut dec = FrameDecoder::new();
        let mut got = Vec::new();
        for chunk in bytes.chunks(3) {
            dec.push(chunk);
            while let Some(m) = dec.next_message::<ClientMsg>().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(got, vec![a, b]);
    }

    #[test]
    fn oversized_and_malformed_frames_are_errors() {
        let mut dec = FrameDecoder::new();
        dec.push(&u16::MAX.to_be_bytes());
        assert!(matches!(
            dec.next_message::<ClientMsg>(),
            Err(FrameError::TooLarge(_))
        ));

        let mut dec = FrameDecoder::new();
        dec.push(&[0, 1, 0xFF]);
        assert!(matches!(
            dec.next_message::<ClientMsg>(),
            Err(FrameError::Malformed(_))
        ));

        let huge = ClientMsg::Chat(ChatText::new(&"x".repeat(MAX_CHAT_CHARS)).unwrap());
        assert!(encode_frame(&huge).is_ok());
    }

    #[test]
    fn a_message_with_an_empty_name_is_malformed() {
        let mut body = postcard::to_stdvec(&ClientMsg::Rename(name("x"))).unwrap();
        let at = body.len() - 1;
        body[at] = b'\n';
        let mut frame = (body.len() as u16).to_be_bytes().to_vec();
        frame.extend(body);
        let mut dec = FrameDecoder::new();
        dec.push(&frame);
        assert!(matches!(
            dec.next_message::<ClientMsg>(),
            Err(FrameError::Malformed(_))
        ));
    }

    #[test]
    fn every_animal_has_its_own_key() {
        let mut keys: Vec<&str> = Avatar::ALL.iter().map(|a| a.key()).collect();
        assert_eq!(Avatar::ALL[0], Avatar::None);
        for animal in Avatar::ALL {
            assert_eq!(Avatar::from_key(animal.key()), animal);
        }
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), Avatar::ALL.len());
    }

    #[test]
    fn an_animal_nobody_knows_is_no_animal() {
        assert_eq!(Avatar::from_key("axolotl"), Avatar::None);
        assert_eq!(Avatar::from_key(""), Avatar::None);
        assert_eq!(Avatar::from_key(&"x".repeat(9000)), Avatar::None);
        assert_eq!(Avatar::default(), Avatar::None);
        assert_eq!(
            serde_json::from_str::<Avatar>("\"newt\"").unwrap(),
            Avatar::None
        );
    }

    #[test]
    fn an_animal_travels_by_its_key() {
        let peer = PeerInfo {
            id: 3,
            name: name("Panda"),
            avatar: Avatar::Panda,
            state: VoiceState::default(),
            share: None,
        };
        let bytes = postcard::to_stdvec(&peer).unwrap();
        assert_eq!(postcard::from_bytes::<PeerInfo>(&bytes).unwrap(), peer);
        assert_eq!(
            serde_json::to_string(&Avatar::Hedgehog).unwrap(),
            "\"hedgehog\""
        );
    }

    #[test]
    fn a_room_is_full_once_its_people_reach_the_limit() {
        let person = |text: &str| RoomPerson {
            name: name(text),
            avatar: Avatar::None,
            host: false,
        };
        let mut room = RoomSummary {
            name: RoomName::new("General").unwrap(),
            people: vec![person("Panda"), person("Fox")],
            locked: false,
            limit: None,
        };
        assert!(!room.is_full());
        room.limit = Some(RoomLimit::new(3));
        assert!(!room.is_full());
        room.limit = Some(RoomLimit::new(2));
        assert!(room.is_full());
        room.people.push(person("Raccoon"));
        assert!(room.is_full(), "a limit lowered under the people inside");
    }
}
