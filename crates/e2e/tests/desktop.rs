#![cfg(windows)]

use std::time::Duration;

use whisperia::{
    config::Hotkeys,
    desktop::{self, keys, DesktopCommand, DesktopEvent, DesktopShare, DesktopStatus, Presence},
    wake::Waker,
};

fn keys(text: &str) -> Option<keys::Combination> {
    Some(keys::parse(text).expect("a test key"))
}

fn share(sharer: u16, name: &str, watched: bool) -> DesktopShare {
    DesktopShare {
        sharer,
        name: name.into(),
        watched,
    }
}

#[test]
#[ignore = "needs an interactive desktop session"]
fn tray_icon_appears_and_hotkeys_register() {
    let hotkeys = Hotkeys {
        mute: keys("Ctrl+Alt+Shift+F11"),
        deafen: keys("Ctrl+Alt+Shift+F12"),
        share: keys("Ctrl+Alt+Shift+F10"),
    };
    let link = desktop::start(&hotkeys, &Waker::default()).expect("desktop integration starts");
    for presence in [Presence::Live, Presence::Muted, Presence::Deafened] {
        (link.tell)(DesktopCommand::Status(DesktopStatus {
            presence,
            sharing: false,
            shares: Vec::new(),
        }));
        std::thread::sleep(Duration::from_millis(300));
    }
    for shares in [
        vec![share(3, "Raccoon", false)],
        vec![share(3, "Raccoon", true), share(4, "Fox", false)],
        Vec::new(),
    ] {
        (link.tell)(DesktopCommand::Status(DesktopStatus {
            presence: Presence::Live,
            sharing: !shares.is_empty(),
            shares,
        }));
        std::thread::sleep(Duration::from_millis(300));
    }
    let problems: Vec<DesktopEvent> = link.events.try_iter().collect();
    assert_eq!(problems, [], "icon or hotkeys failed");

    let press_keys = std::env::var_os("WW_PRESS_KEYS").is_some();
    if press_keys {
        println!("waiting for Ctrl+Alt+Shift+F11...");
        let event = link.events.recv_timeout(Duration::from_secs(20));
        assert_eq!(event, Ok(DesktopEvent::ToggleMute));
    }

    (link.tell)(DesktopCommand::Hotkeys(Hotkeys {
        mute: keys("Ctrl+Alt+Shift+F9"),
        deafen: None,
        share: None,
    }));
    std::thread::sleep(Duration::from_millis(300));
    let problems: Vec<DesktopEvent> = link.events.try_iter().collect();
    assert_eq!(problems, [], "the new keys were not taken");
    if press_keys {
        println!("waiting for Ctrl+Alt+Shift+F9...");
        let event = link.events.recv_timeout(Duration::from_secs(20));
        assert_eq!(event, Ok(DesktopEvent::ToggleMute));

        (link.tell)(DesktopCommand::CaptureKey(true));
        std::thread::sleep(Duration::from_millis(300));
        println!("waiting for F16...");
        let event = link.events.recv_timeout(Duration::from_secs(20));
        assert_eq!(event, Ok(DesktopEvent::KeyCaptureCancelled));
    }
}
