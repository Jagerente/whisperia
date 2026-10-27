use std::sync::mpsc::Receiver;

use whisperia_proto::PeerId;

use crate::wake::{self, Waker, WakingSender};

#[cfg(windows)]
pub mod fatal;
#[cfg(windows)]
mod hook;
pub mod keys;
#[cfg(windows)]
mod pump;
#[cfg(windows)]
pub mod session;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopEvent {
    ToggleMute,
    ToggleDeafen,
    ToggleShare,
    ToggleWatch(PeerId),
    ShowWindow,
    OpenSettings,
    Quit,
    KeyCaptured(keys::Combination),
    KeyCaptureCancelled,
    Problem(String),
    NoTray,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Away,
    Live,
    Muted,
    Deafened,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopShare {
    pub sharer: PeerId,
    pub name: String,
    pub watched: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopStatus {
    pub presence: Presence,
    pub sharing: bool,
    pub shares: Vec<DesktopShare>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopCommand {
    Status(DesktopStatus),
    Hotkeys(crate::config::Hotkeys),

    CaptureKey(bool),
}

pub struct DesktopLink {
    pub events: Receiver<DesktopEvent>,
    pub tell: Box<dyn Fn(DesktopCommand) + Send>,

    _guard: Option<Box<dyn std::any::Any + Send>>,
}

impl DesktopLink {
    pub fn channels(waker: &Waker) -> (Self, WakingSender<DesktopEvent>, Receiver<DesktopCommand>) {
        let (events_tx, events) = wake::channel(waker);
        let (commands_tx, commands) = std::sync::mpsc::channel();
        let link = Self {
            events,
            tell: Box::new(move |command| {
                let _ = commands_tx.send(command);
            }),
            _guard: None,
        };
        (link, events_tx, commands)
    }
}

pub fn start(hotkeys: &crate::config::Hotkeys, waker: &Waker) -> Option<DesktopLink> {
    #[cfg(windows)]
    {
        windows::start(hotkeys.clone(), waker)
    }
    #[cfg(not(windows))]
    {
        let _ = (hotkeys, waker);
        None
    }
}
