use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use whisperia_proto::control::{Avatar, Name};

use crate::{
    address::ServerAddress,
    desktop::keys::{self, Combination},
    dsp::{cue::Cue, denoise::DenoiseMode},
    files,
    trust::Fingerprint,
};

const MAX_RECENT: usize = 8;

const MAX_PEOPLE: usize = 32;
pub const GAIN_RANGE_DB: std::ops::RangeInclusive<f32> = -30.0..=20.0;
pub const MIC_GAIN_RANGE_DB: std::ops::RangeInclusive<f32> = -30.0..=30.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub name: Name,
    pub avatar: Avatar,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub denoise: DenoiseMode,
    pub mic_gain_db: f32,
    pub volume_db: f32,
    pub agc: bool,
    pub sounds: bool,
    pub notifications: Notifications,
    pub name_confirmed: bool,

    pub close_to_tray: bool,
    pub appearance: Appearance,
    pub hotkeys: Hotkeys,
    pub share: ShareConfig,
    pub recent: Vec<RecentServer>,

    pub people: Vec<PersonVolume>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    #[serde(deserialize_with = "keys_or_none")]
    pub mute: Option<Combination>,
    #[serde(deserialize_with = "keys_or_none")]
    pub deafen: Option<Combination>,
    #[serde(deserialize_with = "keys_or_none")]
    pub share: Option<Combination>,
}

fn keys_or_none<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Combination>, D::Error> {
    let written = Option::<String>::deserialize(deserializer)?;
    Ok(written.and_then(|text| keys::parse(&text).ok()))
}

impl Default for Hotkeys {
    fn default() -> Self {
        let with_ctrl_alt = |letter: u8| Combination {
            vk: u16::from(letter),
            modifiers: keys::CTRL | keys::ALT,
        };
        Self {
            mute: Some(with_ctrl_alt(b'M')),
            deafen: Some(with_ctrl_alt(b'D')),
            share: Some(with_ctrl_alt(b'S')),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Mute,
    Deafen,
    Share,
}

impl HotkeyAction {
    pub const ALL: [Self; 3] = [Self::Mute, Self::Deafen, Self::Share];

    pub fn name(self) -> &'static str {
        match self {
            Self::Mute => "Mute / Unmute",
            Self::Deafen => "Deafen / Undeafen",
            Self::Share => "Share my screen / Stop sharing",
        }
    }
}

impl Hotkeys {
    pub fn get(&self, action: HotkeyAction) -> Option<Combination> {
        match action {
            HotkeyAction::Mute => self.mute,
            HotkeyAction::Deafen => self.deafen,
            HotkeyAction::Share => self.share,
        }
    }

    pub fn set(&mut self, action: HotkeyAction, keys: Option<Combination>) {
        match action {
            HotkeyAction::Mute => self.mute = keys,
            HotkeyAction::Deafen => self.deafen = keys,
            HotkeyAction::Share => self.share = keys,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub join: bool,
    pub leave: bool,
    pub message: bool,
    pub mute: bool,
    pub unmute: bool,
    pub deafen: bool,
    pub undeafen: bool,
    pub share_start: bool,
    pub share_stop: bool,
    pub connected: bool,
    pub lost: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            join: true,
            leave: true,
            message: true,
            mute: true,
            unmute: true,
            deafen: true,
            undeafen: true,
            share_start: true,
            share_stop: true,
            connected: true,
            lost: true,
        }
    }
}

impl Notifications {
    fn switch(&mut self, cue: Cue) -> &mut bool {
        match cue {
            Cue::Join => &mut self.join,
            Cue::Leave => &mut self.leave,
            Cue::Message => &mut self.message,
            Cue::Mute => &mut self.mute,
            Cue::Unmute => &mut self.unmute,
            Cue::Deafen => &mut self.deafen,
            Cue::Undeafen => &mut self.undeafen,
            Cue::ShareStart => &mut self.share_start,
            Cue::ShareStop => &mut self.share_stop,
            Cue::Connected => &mut self.connected,
            Cue::Lost => &mut self.lost,
        }
    }

    pub fn allows(mut self, cue: Cue) -> bool {
        *self.switch(cue)
    }

    pub fn toggle(&mut self, cue: Cue) {
        let switch = self.switch(cue);
        *switch = !*switch;
    }
}

pub const SHARE_FPS_RANGE: std::ops::RangeInclusive<u32> = 5..=120;
pub const SHARE_RESOLUTIONS: [u32; 4] = [720, 1080, 1440, 0];
pub const SHARE_FRAME_RATES: [u32; 3] = [15, 30, 60];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareConfig {
    pub fps: u32,
    pub max_height: u32,
}

impl Default for ShareConfig {
    fn default() -> Self {
        Self {
            fps: 60,
            max_height: 1440,
        }
    }
}

impl ShareConfig {
    pub fn resolution_name(max_height: u32) -> String {
        match max_height {
            0 => "Original".to_owned(),
            lines => format!("{lines}p"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentServer {
    pub name: Name,
    pub address: ServerAddress,
    #[serde(default)]
    pub id: Option<Fingerprint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonVolume {
    pub name: String,
    #[serde(default)]
    pub volume_db: f32,
    #[serde(default)]
    pub screen_db: f32,
    #[serde(default)]
    pub screen_muted: bool,
}

impl PersonVolume {
    fn is_usual(&self) -> bool {
        self.volume_db == 0.0 && self.screen_db == 0.0 && !self.screen_muted
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: default_name(),
            avatar: Avatar::None,
            input_device: None,
            output_device: None,
            denoise: DenoiseMode::default(),
            mic_gain_db: 0.0,
            volume_db: 0.0,
            agc: true,
            sounds: true,
            notifications: Notifications::default(),
            name_confirmed: false,
            close_to_tray: true,
            appearance: Appearance::System,
            hotkeys: Hotkeys::default(),
            share: ShareConfig::default(),
            recent: Vec::new(),
            people: Vec::new(),
        }
    }
}

fn default_name() -> Name {
    [std::env::var("USERNAME").ok(), std::env::var("USER").ok()]
        .into_iter()
        .flatten()
        .find_map(|raw| Name::new(&raw).ok())
        .unwrap_or_else(Name::guest)
}

pub fn clamp_db(db: f32) -> f32 {
    clamp_into(db, &GAIN_RANGE_DB)
}

pub fn clamp_mic_db(db: f32) -> f32 {
    clamp_into(db, &MIC_GAIN_RANGE_DB)
}

fn clamp_into(db: f32, range: &std::ops::RangeInclusive<f32>) -> f32 {
    if db.is_finite() {
        db.clamp(*range.start(), *range.end())
    } else {
        0.0
    }
}

impl Config {
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let (mut config, problem): (Self, _) = files::load_json(path);
        config.sanitize();
        (config, problem)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        files::save_json(path, self)
    }

    fn sanitize(&mut self) {
        self.mic_gain_db = clamp_mic_db(self.mic_gain_db);
        self.volume_db = clamp_db(self.volume_db);
        self.recent.truncate(MAX_RECENT);
        self.people.iter_mut().for_each(|p| {
            p.volume_db = clamp_db(p.volume_db);
            p.screen_db = clamp_db(p.screen_db);
        });
        self.people.retain(|p| !p.is_usual());
        self.people.truncate(MAX_PEOPLE);
        self.share.fps = self
            .share
            .fps
            .clamp(*SHARE_FPS_RANGE.start(), *SHARE_FPS_RANGE.end());
    }

    fn person(&self, name: &str) -> PersonVolume {
        self.people
            .iter()
            .find(|p| p.name == name)
            .cloned()
            .unwrap_or_else(|| PersonVolume {
                name: name.to_owned(),
                volume_db: 0.0,
                screen_db: 0.0,
                screen_muted: false,
            })
    }

    pub fn person_volume(&self, name: &str) -> f32 {
        self.person(name).volume_db
    }

    pub fn person_screen_volume(&self, name: &str) -> f32 {
        self.person(name).screen_db
    }

    pub fn person_screen_muted(&self, name: &str) -> bool {
        self.person(name).screen_muted
    }

    pub fn person_screen_volume_heard(&self, name: &str) -> f32 {
        let person = self.person(name);
        match person.screen_muted {
            true => *GAIN_RANGE_DB.start(),
            false => person.screen_db,
        }
    }

    pub fn set_person_volume(&mut self, name: &str, volume_db: f32) {
        let person = self.person(name);
        self.remember(PersonVolume {
            volume_db: clamp_db(volume_db),
            ..person
        });
    }

    pub fn set_person_screen_volume(&mut self, name: &str, screen_db: f32) {
        let person = self.person(name);
        self.remember(PersonVolume {
            screen_db: clamp_db(screen_db),
            ..person
        });
    }

    pub fn set_person_screen_muted(&mut self, name: &str, screen_muted: bool) {
        let person = self.person(name);
        self.remember(PersonVolume {
            screen_muted,
            ..person
        });
    }

    fn remember(&mut self, person: PersonVolume) {
        self.people.retain(|p| p.name != person.name);
        if person.is_usual() {
            return;
        }
        self.people.push(person);
        if self.people.len() > MAX_PEOPLE {
            self.people.remove(0);
        }
    }

    pub fn touch_recent(&mut self, server: RecentServer) {
        self.recent
            .retain(|r| r.address != server.address && (server.id.is_none() || r.id != server.id));
        self.recent.insert(0, server);
        self.recent.truncate(MAX_RECENT);
    }
}

pub struct Paths {
    pub config: PathBuf,
    pub trust: PathBuf,
    pub log: PathBuf,
}

impl Paths {
    pub fn resolve(override_dir: Option<PathBuf>) -> Option<Self> {
        let (settings, logs) = match override_dir {
            Some(dir) => (dir.clone(), dir),
            None => {
                let project = directories::ProjectDirs::from("", "", "whisperia")?;
                (
                    project.config_dir().to_path_buf(),
                    project.data_local_dir().to_path_buf(),
                )
            }
        };
        Some(Self {
            config: settings.join("settings.json"),
            trust: settings.join("known_servers.json"),
            log: logs.join("client.log"),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn what_a_file_does_not_say_is_the_default() {
        let path = std::env::temp_dir().join(format!("ww-partial-{}.json", std::process::id()));
        fs::write(&path, r#"{"name": "Panda"}"#).unwrap();
        let (config, warning) = Config::load(&path);
        let _ = fs::remove_file(path);
        assert_eq!(warning, None);
        assert_eq!(
            config,
            Config {
                name: Name::new("Panda").unwrap(),
                ..Config::default()
            }
        );
    }

    #[test]
    fn a_person_is_remembered_only_while_they_differ_from_everybody_else() {
        let mut config = Config::default();
        assert_eq!(config.person_volume("Raccoon"), 0.0);

        config.set_person_volume("Raccoon", -4.0);
        config.set_person_volume("Fox", 3.0);
        assert_eq!(config.person_volume("Raccoon"), -4.0);
        assert_eq!(config.person_volume("Fox"), 3.0);
        assert_eq!(config.people.len(), 2);

        config.set_person_volume("Raccoon", -6.0);
        assert_eq!(config.person_volume("Raccoon"), -6.0);
        assert_eq!(config.people.len(), 2);

        config.set_person_volume("Raccoon", -99.0);
        assert_eq!(config.person_volume("Raccoon"), *GAIN_RANGE_DB.start());
        config.set_person_volume("Raccoon", 0.0);
        assert_eq!(config.person_volume("Raccoon"), 0.0);
        assert_eq!(config.people.len(), 1);

        config.set_person_screen_volume("Fox", -10.0);
        assert_eq!(config.person_volume("Fox"), 3.0);
        assert_eq!(config.person_screen_volume("Fox"), -10.0);
        config.set_person_volume("Fox", 0.0);
        assert_eq!(config.person_screen_volume("Fox"), -10.0);
        config.set_person_screen_volume("Fox", 0.0);
        assert!(config.people.is_empty());
        config.set_person_volume("Fox", 3.0);

        config.people = (0..MAX_PEOPLE + 5)
            .map(|i| PersonVolume {
                name: format!("p{i}"),
                volume_db: 1.0,
                screen_db: 0.0,
                screen_muted: false,
            })
            .collect();
        config.sanitize();
        assert_eq!(config.people.len(), MAX_PEOPLE);

        (0..5).for_each(|i| config.set_person_volume(&format!("late{i}"), 2.0));
        assert_eq!(config.people.len(), MAX_PEOPLE);
        assert_eq!(config.person_volume("late4"), 2.0);
    }

    #[test]
    fn a_key_taken_off_or_not_understood_is_none() {
        let hotkeys: Hotkeys =
            serde_json::from_str(r#"{"mute": "", "deafen": "Hyper+D", "share": "Ctrl+Alt+S"}"#)
                .unwrap();
        assert_eq!(hotkeys.mute, None);
        assert_eq!(hotkeys.deafen, None);
        assert_eq!(hotkeys.share, Hotkeys::default().share);
        let written = serde_json::to_string(&Hotkeys::default()).unwrap();
        assert!(written.contains("\"Ctrl+Alt+M\""), "{written}");
    }

    #[test]
    fn a_muted_screen_keeps_its_volume_for_when_it_is_heard_again() {
        let mut config = Config::default();
        config.set_person_screen_volume("Fox", -6.0);
        config.set_person_screen_muted("Fox", true);
        assert_eq!(
            config.person_screen_volume_heard("Fox"),
            *GAIN_RANGE_DB.start()
        );
        assert_eq!(config.person_screen_volume("Fox"), -6.0);
        config.set_person_screen_muted("Fox", false);
        assert_eq!(config.person_screen_volume_heard("Fox"), -6.0);

        config.set_person_screen_volume("Raccoon", 0.0);
        config.set_person_screen_muted("Raccoon", true);
        assert!(
            config.people.iter().any(|p| p.name == "Raccoon"),
            "a mute alone is worth remembering"
        );
    }

    #[test]
    fn every_sound_has_a_switch_of_its_own() {
        let path = std::env::temp_dir().join(format!("ww-sounds-{}.json", std::process::id()));
        fs::write(&path, r#"{"notifications": {"message": false}}"#).unwrap();
        let (mut config, warning) = Config::load(&path);
        let _ = fs::remove_file(path);
        assert_eq!(warning, None);
        assert!(!config.notifications.allows(Cue::Message));
        assert!(config.notifications.allows(Cue::Join));

        config.notifications.toggle(Cue::Lost);
        config.notifications.toggle(Cue::Message);
        assert!(!config.notifications.allows(Cue::Lost));
        assert!(config.notifications.allows(Cue::Connected));
        assert!(config.notifications.allows(Cue::Message));
    }

    fn recent(name: &str, address: &str, id: Option<u8>) -> RecentServer {
        RecentServer {
            name: Name::new(name).unwrap(),
            address: address.parse().unwrap(),
            id: id.map(|byte| Fingerprint::from([byte; 32])),
        }
    }

    #[test]
    fn what_is_saved_is_read_back() {
        let dir = std::env::temp_dir().join(format!("ww-config-{}", std::process::id()));
        let path = dir.join("settings.json");
        let mut config = Config {
            name: Name::new("Panda").unwrap(),
            avatar: Avatar::Panda,
            appearance: Appearance::Dark,
            volume_db: -6.0,
            ..Config::default()
        };
        config.set_person_volume("Raccoon", -4.0);
        config.touch_recent(recent("DESKTOP-6TNC", "192.168.1.5", Some(1)));
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path), (config, None));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_server_visited_again_moves_to_the_top_once() {
        let mut config = Config::default();
        config.touch_recent(recent("DESKTOP-6TNC", "192.168.1.5", Some(1)));
        config.touch_recent(recent("DESKTOP-AV83", "192.168.1.9:5000", Some(2)));
        config.touch_recent(recent("DESKTOP-6TNC", "192.168.1.5:47820", Some(1)));
        assert_eq!(config.recent.len(), 2);
        assert_eq!(config.recent[0].name, "DESKTOP-6TNC");
        config.touch_recent(recent("DESKTOP-AV83", "10.0.0.9", Some(2)));
        assert_eq!(config.recent.len(), 2, "the same server at a new address");
        assert_eq!(config.recent[0].address.to_string(), "10.0.0.9");
    }

    #[test]
    fn values_out_of_range_are_brought_back_when_read() {
        let dir = std::env::temp_dir().join(format!("ww-config-range-{}", std::process::id()));
        let path = dir.join("settings.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            &path,
            r#"{"name": "\u001b[2J", "mic_gain_db": 900.0, "avatar": "newt"}"#,
        )
        .unwrap();
        let (loaded, warning) = Config::load(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.mic_gain_db, *MIC_GAIN_RANGE_DB.end());
        assert_eq!(loaded.name, "[2J");
        assert_eq!(loaded.avatar, Avatar::None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_volume_that_is_not_a_number_is_no_change_and_the_file_stays_readable() {
        let dir = std::env::temp_dir().join(format!("ww-config-nan-{}", std::process::id()));
        let path = dir.join("settings.json");
        let mut config = Config::default();
        config.set_person_volume("Raccoon", f32::NAN);
        config.set_person_screen_volume("Fox", f32::INFINITY);
        assert!(config.people.is_empty());
        assert_eq!(clamp_db(f32::NAN), 0.0);
        config.volume_db = clamp_db(f32::NEG_INFINITY);
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path), (config, None));
        let _ = fs::remove_dir_all(dir);
    }
}
