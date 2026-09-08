use std::fmt;

use serde::{Deserialize, Serialize};

pub const MAX_NAME_CHARS: usize = 24;
pub const MAX_CHAT_CHARS: usize = 400;
pub const MAX_PASSWORD_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{0} is empty")]
pub struct Empty(&'static str);

macro_rules! text {
    ($type:ident, $what:literal, $clean:ident) => {
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $type(String);

        impl $type {
            pub fn new(raw: &str) -> Result<Self, Empty> {
                $clean(raw).map(Self).ok_or(Empty($what))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $type {
            type Error = Empty;

            fn try_from(raw: String) -> Result<Self, Empty> {
                Self::new(&raw)
            }
        }

        impl TryFrom<&str> for $type {
            type Error = Empty;

            fn try_from(raw: &str) -> Result<Self, Empty> {
                Self::new(raw)
            }
        }

        impl From<$type> for String {
            fn from(text: $type) -> String {
                text.0
            }
        }

        impl std::ops::Deref for $type {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl PartialEq<str> for $type {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $type {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $type {
            fn eq(&self, other: &String) -> bool {
                self.0 == *other
            }
        }
    };
}

text!(Name, "a name", clean_name);
text!(RoomName, "a room name", clean_name);
text!(ChatText, "a message", clean_chat);
text!(Password, "a password", clean_password);

impl Name {
    pub fn guest() -> Self {
        Self("guest".to_owned())
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for RoomName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RoomName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for ChatText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ChatText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(..)")
    }
}

impl From<Name> for RoomName {
    fn from(name: Name) -> Self {
        Self(name.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub struct RoomLimit(u8);

impl RoomLimit {
    pub const FEWEST: u8 = 2;
    pub const MOST: u8 = 25;

    pub fn new(people: u8) -> Self {
        Self(people.clamp(Self::FEWEST, Self::MOST))
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl From<u8> for RoomLimit {
    fn from(people: u8) -> Self {
        Self::new(people)
    }
}

impl From<RoomLimit> for u8 {
    fn from(limit: RoomLimit) -> u8 {
        limit.0
    }
}

impl fmt::Display for RoomLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

fn printable(c: char) -> char {
    if c.is_control() {
        ' '
    } else {
        c
    }
}

fn clean_name(raw: &str) -> Option<String> {
    let visible: String = raw
        .chars()
        .filter(|c| !is_invisible(*c))
        .map(printable)
        .collect();
    let collapsed = visible.split_whitespace().collect::<Vec<_>>().join(" ");
    let bounded: String = collapsed.chars().take(MAX_NAME_CHARS).collect();
    let bounded = bounded.trim_end();
    (!bounded.is_empty()).then(|| bounded.to_owned())
}

fn clean_chat(raw: &str) -> Option<String> {
    let visible: String = raw
        .chars()
        .filter(|c| !is_invisible(*c))
        .map(printable)
        .collect();
    let bounded: String = visible.trim_start().chars().take(MAX_CHAT_CHARS).collect();
    let bounded = bounded.trim_end();
    (!bounded.is_empty()).then(|| bounded.to_owned())
}

fn clean_password(raw: &str) -> Option<String> {
    let bounded: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_PASSWORD_CHARS)
        .collect();
    (!bounded.trim().is_empty()).then_some(bounded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_line_of_visible_text() {
        assert_eq!(Name::new("  Panda \x1b[31m  B\n").unwrap(), "Panda [31m B");
        assert_eq!(Name::new("Pan\u{202E}da\u{200B}").unwrap(), "Panda");
        assert_eq!(Name::new("\x07\n  \u{200B}"), Err(Empty("a name")));
    }

    #[test]
    fn a_name_is_cut_to_its_length() {
        let long = Name::new(&"x".repeat(100)).unwrap();
        assert_eq!(long.chars().count(), MAX_NAME_CHARS);
        let spaced = format!("{} y", "x".repeat(MAX_NAME_CHARS - 1));
        assert_eq!(Name::new(&spaced).unwrap(), "x".repeat(MAX_NAME_CHARS - 1));
    }

    #[test]
    fn a_name_from_the_wire_is_cleaned_or_refused() {
        let clean = postcard::to_stdvec(&" Panda ").unwrap();
        assert_eq!(postcard::from_bytes::<Name>(&clean).unwrap(), "Panda");
        let empty = postcard::to_stdvec(&"\n").unwrap();
        assert!(postcard::from_bytes::<Name>(&empty).is_err());
    }

    #[test]
    fn a_message_keeps_its_length_for_what_is_said() {
        assert_eq!(ChatText::new(" hi\x1b[2J\n").unwrap(), "hi [2J");
        assert!(ChatText::new("\n\t ").is_err());
        let padded = format!("{}{}", " ".repeat(300), "я".repeat(1000));
        assert_eq!(
            ChatText::new(&padded).unwrap().chars().count(),
            MAX_CHAT_CHARS
        );
    }

    #[test]
    fn a_password_keeps_its_spaces_and_hides_itself() {
        assert!(Password::new("  ").is_err());
        assert!(Password::new("\n").is_err());
        assert_eq!(Password::new(" open sesame ").unwrap(), " open sesame ");
        assert_eq!(
            Password::new(&"я".repeat(500)).unwrap().chars().count(),
            MAX_PASSWORD_CHARS
        );
        assert_eq!(
            format!("{:?}", Password::new("owl").unwrap()),
            "Password(..)"
        );
    }

    #[test]
    fn a_room_limit_stays_between_two_and_twenty_five() {
        assert_eq!(RoomLimit::new(0).get(), RoomLimit::FEWEST);
        assert_eq!(RoomLimit::new(7).get(), 7);
        assert_eq!(RoomLimit::new(255).get(), RoomLimit::MOST);
        let wire = postcard::to_stdvec(&200u8).unwrap();
        assert_eq!(
            postcard::from_bytes::<RoomLimit>(&wire).unwrap().get(),
            RoomLimit::MOST
        );
    }
}
