use std::collections::HashSet;

use chrono::{DateTime, Local};
use whisperia_proto::{
    control::{Avatar, Name, PeerInfo},
    PeerId,
};

use super::message::{Message, Url};

const MAX_LINES: usize = 200;

#[derive(Debug, Clone)]
pub struct ChatLine {
    pub number: u64,
    pub at: DateTime<Local>,
    pub what: Line,
}

#[derive(Debug, Clone)]
pub enum Line {
    Said(Said),
    Note { about: About, text: String },
}

#[derive(Debug, Clone)]
pub struct Said {
    pub from: Option<PeerId>,
    pub who: Name,
    pub avatar: Avatar,
    pub message: Message,
    pub mine: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum About {
    Joined,
    Left,
    Kicked,
    Host,
    Locked,
    Unlocked,
    Size,
    Sharing,
    StoppedSharing,
    Renamed,
}

impl ChatLine {
    pub fn text(&self) -> &str {
        match &self.what {
            Line::Said(said) => said.message.text(),
            Line::Note { text, .. } => text,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Chat {
    lines: Vec<ChatLine>,
    written: u64,
    heard: u64,
    hidden: HashSet<(u64, Url)>,
}

impl Chat {
    pub fn lines(&self) -> &[ChatLine] {
        &self.lines
    }

    pub fn heard(&self) -> u64 {
        self.heard
    }

    pub fn links(&self) -> impl Iterator<Item = &Url> {
        self.lines
            .iter()
            .flat_map(|line| match &line.what {
                Line::Said(said) => Some(said.message.links()),
                Line::Note { .. } => None,
            })
            .flatten()
    }

    pub fn shows_preview(&self, line: u64, url: &Url) -> bool {
        !self.hidden.contains(&(line, url.clone()))
    }

    pub(super) fn hide_preview(&mut self, line: u64, url: Url) {
        if self.lines.iter().any(|shown| shown.number == line) {
            self.hidden.insert((line, url));
        }
    }

    pub(super) fn say(&mut self, said: Said) {
        if !said.mine {
            self.heard += 1;
        }
        self.write(Line::Said(said));
    }

    pub(super) fn note(&mut self, about: About, text: String) {
        self.write(Line::Note { about, text });
    }

    fn write(&mut self, what: Line) {
        self.lines.push(ChatLine {
            number: self.written,
            at: Local::now(),
            what,
        });
        self.written += 1;
        if self.lines.len() > MAX_LINES {
            let gone = self.lines.remove(0).number;
            self.hidden.retain(|(line, _)| *line != gone);
        }
    }

    pub(super) fn reface(&mut self, id: PeerId, avatar: Avatar) {
        for said in self.said_mut() {
            if said.from == Some(id) {
                said.avatar = avatar;
            }
        }
    }

    pub(super) fn belong_to(&mut self, peers: &[PeerInfo], me: PeerId) {
        for said in self.said_mut() {
            said.from = match said.mine {
                true => Some(me),
                false => peers.iter().find(|p| p.name == said.who).map(|p| p.id),
            };
        }
    }

    fn said_mut(&mut self) -> impl Iterator<Item = &mut Said> {
        self.lines
            .iter_mut()
            .filter_map(|line| match &mut line.what {
                Line::Said(said) => Some(said),
                Line::Note { .. } => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use whisperia_proto::control::{ChatText, VoiceState};

    use super::*;

    fn said(from: PeerId, who: &str, avatar: Avatar, text: &str) -> Said {
        Said {
            from: Some(from),
            who: Name::new(who).unwrap(),
            avatar,
            message: Message::from(&ChatText::new(text).unwrap()),
            mine: false,
        }
    }

    fn faces(chat: &Chat) -> Vec<(Option<PeerId>, Avatar)> {
        chat.lines
            .iter()
            .filter_map(|line| match &line.what {
                Line::Said(said) => Some((said.from, said.avatar)),
                Line::Note { .. } => None,
            })
            .collect()
    }

    #[test]
    fn a_new_face_reaches_the_lines_of_its_owner_only() {
        let mut chat = Chat::default();
        chat.say(said(7, "Panda", Avatar::Panda, "hi"));
        chat.say(said(8, "Panda", Avatar::Panda, "hello"));
        chat.reface(7, Avatar::Owl);
        assert_eq!(
            faces(&chat),
            [(Some(7), Avatar::Owl), (Some(8), Avatar::Panda)]
        );
    }

    #[test]
    fn the_chat_keeps_the_last_lines_and_counts_them_all() {
        let mut chat = Chat::default();
        for n in 0..MAX_LINES + 5 {
            chat.note(About::Joined, n.to_string());
        }
        assert_eq!(chat.lines.len(), MAX_LINES);
        assert_eq!((chat.lines[0].text(), chat.lines[0].number), ("5", 5));
        assert_eq!(
            chat.lines.last().map(|line| line.number),
            Some(MAX_LINES as u64 + 4)
        );
    }

    #[test]
    fn only_what_the_others_say_is_heard() {
        let mut chat = Chat::default();
        chat.say(said(7, "Fox", Avatar::Fox, "hi"));
        chat.say(Said {
            mine: true,
            ..said(3, "Panda", Avatar::Panda, "hello")
        });
        chat.note(About::Joined, "Octopus joined".into());
        chat.say(said(7, "Fox", Avatar::Fox, "bye"));
        assert_eq!(chat.heard(), 2);
    }

    #[test]
    fn a_chat_brought_back_after_a_reconnect_belongs_to_the_new_ids() {
        let mut chat = Chat::default();
        chat.say(said(7, "Fox", Avatar::Fox, "hi"));
        chat.say(Said {
            mine: true,
            ..said(3, "Panda", Avatar::Panda, "hello")
        });
        let fox = PeerInfo {
            id: 12,
            name: Name::new("Fox").unwrap(),
            avatar: Avatar::Fox,
            state: VoiceState::default(),
            share: None,
        };
        chat.belong_to(&[fox], 9);
        chat.reface(12, Avatar::Owl);
        assert_eq!(
            faces(&chat),
            [(Some(12), Avatar::Owl), (Some(9), Avatar::Panda)]
        );
    }

    #[test]
    fn a_hidden_preview_is_hidden_under_its_own_message_only() {
        let mut chat = Chat::default();
        chat.say(said(7, "Fox", Avatar::Fox, "https://example.org"));
        chat.say(said(7, "Fox", Avatar::Fox, "again https://example.org"));
        let url = Url::parse("https://example.org").unwrap();
        chat.hide_preview(0, url.clone());
        assert!(!chat.shows_preview(0, &url));
        assert!(chat.shows_preview(1, &url));
    }

    #[test]
    fn a_hidden_preview_is_forgotten_with_its_message() {
        let mut chat = Chat::default();
        chat.say(said(7, "Fox", Avatar::Fox, "https://example.org"));
        chat.hide_preview(0, Url::parse("https://example.org").unwrap());
        for _ in 0..MAX_LINES {
            chat.note(About::Joined, "Octopus joined".into());
        }
        assert!(chat.hidden.is_empty());
    }
}
