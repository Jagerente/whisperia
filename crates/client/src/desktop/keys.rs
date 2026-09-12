use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

pub const CTRL: u8 = 1;
pub const ALT: u8 = 2;
pub const SHIFT: u8 = 4;
pub const WIN: u8 = 8;

const MODIFIERS: [(u8, &str); 4] = [(CTRL, "Ctrl"), (ALT, "Alt"), (SHIFT, "Shift"), (WIN, "Win")];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Combination {
    pub vk: u16,
    pub modifiers: u8,
}

pub fn parse(text: &str) -> Result<Combination, String> {
    let mut modifiers = 0;
    let mut vk = None;
    for token in text.split('+').map(str::trim) {
        if token.is_empty() {
            return Err("empty part".into());
        }
        if let Some((bit, _)) = MODIFIERS
            .iter()
            .find(|(_, word)| word.eq_ignore_ascii_case(token))
        {
            if modifiers & bit != 0 {
                return Err(format!("{token} twice"));
            }
            modifiers |= bit;
            continue;
        }
        if vk.is_some() {
            return Err("only one key besides the modifiers".into());
        }
        vk = Some(key_code(token).ok_or_else(|| format!("unknown key \"{token}\""))?);
    }
    match vk {
        Some(vk) => Ok(Combination { vk, modifiers }),
        None => Err("no key besides the modifiers".into()),
    }
}

pub fn name(combination: Combination) -> Option<String> {
    let mut text = String::new();
    for (bit, word) in MODIFIERS {
        if combination.modifiers & bit != 0 {
            text.push_str(word);
            text.push('+');
        }
    }
    text.push_str(&key_name(combination.vk)?);
    Some(text)
}

pub fn code_name(vk: u16) -> Option<String> {
    match vk {
        0x41..=0x5A => Some(format!("Key{}", char::from(vk as u8))),
        0x30..=0x39 => Some(format!("Digit{}", char::from(vk as u8))),
        0x25 => Some("ArrowLeft".into()),
        0x26 => Some("ArrowUp".into()),
        0x27 => Some("ArrowRight".into()),
        0x28 => Some("ArrowDown".into()),
        _ => key_name(vk),
    }
}

pub fn leaves_typing_alone(combination: Combination) -> bool {
    const PAUSE: u16 = 0x13;
    const INSERT: u16 = 0x2D;
    const SCROLL_LOCK: u16 = 0x91;
    combination.modifiers & (CTRL | ALT | WIN) != 0
        || matches!(combination.vk, 0x70..=0x87 | PAUSE | INSERT | SCROLL_LOCK)
}

impl Serialize for Combination {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text = name(*self).ok_or_else(|| serde::ser::Error::custom("a key without a name"))?;
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for Combination {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse(&text).map_err(de::Error::custom)
    }
}

const NAMED: [(u16, &str); 36] = [
    (0x08, "Backspace"),
    (0x09, "Tab"),
    (0x0D, "Enter"),
    (0x13, "Pause"),
    (0x14, "CapsLock"),
    (0x1B, "Escape"),
    (0x20, "Space"),
    (0x21, "PageUp"),
    (0x22, "PageDown"),
    (0x23, "End"),
    (0x24, "Home"),
    (0x25, "Left"),
    (0x26, "Up"),
    (0x27, "Right"),
    (0x28, "Down"),
    (0x2C, "PrintScreen"),
    (0x2D, "Insert"),
    (0x2E, "Delete"),
    (0x6A, "NumpadMultiply"),
    (0x6B, "NumpadAdd"),
    (0x6D, "NumpadSubtract"),
    (0x6E, "NumpadDecimal"),
    (0x6F, "NumpadDivide"),
    (0x90, "NumLock"),
    (0x91, "ScrollLock"),
    (0xBA, "Semicolon"),
    (0xBB, "Equal"),
    (0xBC, "Comma"),
    (0xBD, "Minus"),
    (0xBE, "Period"),
    (0xBF, "Slash"),
    (0xC0, "Backquote"),
    (0xDB, "BracketLeft"),
    (0xDC, "Backslash"),
    (0xDD, "BracketRight"),
    (0xDE, "Quote"),
];

fn key_name(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some(char::from(vk as u8).to_string()),
        0x70..=0x87 => Some(format!("F{}", vk - 0x70 + 1)),
        0x60..=0x69 => Some(format!("Numpad{}", vk - 0x60)),
        _ => NAMED
            .iter()
            .find(|(code, _)| *code == vk)
            .map(|(_, name)| (*name).to_owned()),
    }
}

fn key_code(name: &str) -> Option<u16> {
    if let [c @ (b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9')] = name.as_bytes() {
        return Some(u16::from(c.to_ascii_uppercase()));
    }
    let lower = name.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
        return (1..=24).contains(&n).then(|| 0x70 + n - 1);
    }
    if let Some(n) = lower
        .strip_prefix("numpad")
        .and_then(|n| n.parse::<u16>().ok())
    {
        return (n <= 9).then(|| 0x60 + n);
    }
    NAMED
        .iter()
        .find(|(_, named)| named.eq_ignore_ascii_case(name))
        .map(|(code, _)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(vk: u16, modifiers: u8) -> Combination {
        Combination { vk, modifiers }
    }

    #[test]
    fn what_is_written_is_what_is_read() {
        assert_eq!(name(keys(0x4D, CTRL | ALT)).as_deref(), Some("Ctrl+Alt+M"));
        assert_eq!(
            name(keys(0x78, SHIFT | WIN)).as_deref(),
            Some("Shift+Win+F9")
        );
        let mut named = 0;
        for vk in 0..=0xFF {
            for modifiers in [0, CTRL | SHIFT, CTRL | ALT | SHIFT | WIN] {
                let combination = keys(vk, modifiers);
                if let Some(text) = name(combination) {
                    assert_eq!(parse(&text), Ok(combination), "{text}");
                    named += 1;
                }
            }
        }
        assert_eq!(named, 3 * (26 + 10 + 24 + 10 + NAMED.len()));
        assert_eq!(name(keys(0x05, 0)), None);
        assert_eq!(name(keys(0xB3, CTRL)), None);
    }

    #[test]
    fn what_is_read_is_written_the_one_way() {
        assert_eq!(
            parse(" ctrl + alt + m ").ok().and_then(name).as_deref(),
            Some("Ctrl+Alt+M")
        );
        assert_eq!(
            parse("win+numpad0").ok().and_then(name).as_deref(),
            Some("Win+Numpad0")
        );
    }

    #[test]
    fn a_key_one_types_with_needs_more_than_shift() {
        let with = |vk, modifiers| leaves_typing_alone(keys(vk, modifiers));
        assert!(with(0x4D, CTRL | ALT));
        assert!(with(0x4D, WIN));
        assert!(!with(0x4D, 0));
        assert!(!with(0x4D, SHIFT));
        assert!(!with(0x20, 0));
        assert!(!with(0x61, 0));
        assert!(with(0x78, 0));
        assert!(with(0x78, SHIFT));
        assert!(with(0x13, 0));
    }

    #[test]
    fn nonsense_is_refused_with_a_reason() {
        assert!(parse("Ctrl+Alt").unwrap_err().contains("no key"));
        assert!(parse("Ctrl+A+B").unwrap_err().contains("one key"));
        assert!(parse("Ctrl++M").unwrap_err().contains("empty"));
        assert!(parse("Ctrl+Ctrl+M").unwrap_err().contains("twice"));
        assert!(parse("Hyper+M").unwrap_err().contains("Hyper"));
        assert!(parse("Control+M").is_err(), "only what the program writes");
        assert!(parse("F25").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn every_named_key_has_a_code_name() {
        assert_eq!(code_name(0x4D).as_deref(), Some("KeyM"));
        assert_eq!(code_name(0x35).as_deref(), Some("Digit5"));
        assert_eq!(code_name(0x25).as_deref(), Some("ArrowLeft"));
        assert_eq!(code_name(0x6B).as_deref(), Some("NumpadAdd"));
        for vk in 0..=0xFF {
            assert_eq!(key_name(vk).is_some(), code_name(vk).is_some(), "{vk:#x}");
        }
    }

    #[test]
    fn keys_are_kept_as_they_are_written() {
        let json = serde_json::to_string(&keys(0x4D, CTRL | ALT)).unwrap();
        assert_eq!(json, "\"Ctrl+Alt+M\"");
        assert_eq!(
            serde_json::from_str::<Combination>(&json).unwrap(),
            keys(0x4D, CTRL | ALT)
        );
        assert!(serde_json::to_string(&keys(0x05, 0)).is_err());
    }
}
