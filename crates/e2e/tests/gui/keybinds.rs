use std::sync::mpsc::Receiver;

use whisperia::{
    config::Hotkeys,
    desktop::{
        keys::{self, Combination},
        DesktopCommand, DesktopEvent, DesktopLink,
    },
    wake::{Waker, WakingSender},
};
use whisperia_e2e::gui::*;

const MUTE: &str = "Key for Mute / Unmute";
const DEAFEN: &str = "Key for Deafen / Undeafen";
const SHARE: &str = "Key for Share my screen / Stop sharing";

struct Keybinds {
    panda: Window,
    desktop: WakingSender<DesktopEvent>,
    commands: Receiver<DesktopCommand>,
}

fn bound(text: &str) -> Option<Combination> {
    Some(keys::parse(text).unwrap())
}

fn keybinds() -> Keybinds {
    let (link, desktop, commands) = DesktopLink::channels(&Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();
    press(&mut panda, "Settings");
    press(&mut panda, "Keybinds");
    wait_for_value(&mut panda, MUTE, "Ctrl+Alt+M");
    Keybinds {
        panda,
        desktop,
        commands,
    }
}

fn pressed(text: &str) -> DesktopEvent {
    DesktopEvent::KeyCaptured(keys::parse(text).unwrap())
}

#[test]
fn the_keybinds_tab_lists_every_key() {
    let Keybinds {
        mut panda,
        desktop: _desktop,
        commands: _commands,
    } = keybinds();
    assert_eq!(panda.value_of(SHARE).as_deref(), Some("Ctrl+Alt+S"));
    assert_eq!(panda.value_of(DEAFEN).as_deref(), Some("Ctrl+Alt+D"));
    assert!(panda.has("Reset all keys"));
    assert!(panda.has_text("Keybinds"), "{}", panda.screen());
    assert!(!panda.has("Noise suppression: Off"));
    picture(&panda, "21-keybinds");

    panda.resize(720.0, 520.0);
    wait_until(&mut panda, "the keys in a small window", |ui| {
        ui.has("Remove the key for Share my screen / Stop sharing")
    });
    assert!(panda.has("Reset all keys"));
    let (remove, _) = middle_of(&panda, "Remove the key for Share my screen / Stop sharing");
    let (close, _) = middle_of(&panda, "Close");
    assert!(
        remove <= close,
        "the rows fit the small sheet without scrolling sideways: {remove} and {close}"
    );
    picture(&panda, "25-settings-small");
}

#[test]
fn a_key_is_captured_and_needs_ctrl_or_alt() {
    let Keybinds {
        mut panda,
        desktop,
        commands,
    } = keybinds();
    press(&mut panda, MUTE);
    wait_for_value(&mut panda, MUTE, "Press the keys...");
    told(&commands, DesktopCommand::CaptureKey(true));
    picture(&panda, "22-asking-for-a-key");

    desktop
        .send(DesktopEvent::KeyCaptured(Combination {
            vk: 0x4D,
            modifiers: keys::SHIFT,
        }))
        .unwrap();
    let refused = "Add Ctrl or Alt: a plain key would stop working in every other program";
    wait_for_text(&mut panda, refused);
    assert!(panda.notice().is_none(), "said in the row, not in a toast");
    let (_, said) = middle_of(&panda, refused);
    let (_, row) = middle_of(&panda, MUTE);
    assert!(
        said > row && said - row < 50.0,
        "said right under the row asking: {said} and {row}"
    );
    told(&commands, DesktopCommand::CaptureKey(true));
    assert_eq!(panda.value_of(MUTE).as_deref(), Some("Press the keys..."));
    picture(&panda, "22b-a-key-refused");

    desktop.send(pressed("Ctrl+Shift+F9")).unwrap();
    wait_for_value(&mut panda, MUTE, "Ctrl+Shift+F9");
    told(
        &commands,
        DesktopCommand::Hotkeys(Hotkeys {
            mute: bound("Ctrl+Shift+F9"),
            ..Hotkeys::default()
        }),
    );

    press(&mut panda, DEAFEN);
    wait_for(&mut panda, "Esc leaves it as it is");
    desktop.send(DesktopEvent::KeyCaptureCancelled).unwrap();
    wait_for_value(&mut panda, DEAFEN, "Ctrl+Alt+D");
}

#[test]
fn keys_are_reset_removed_and_kept() {
    let Keybinds {
        mut panda,
        desktop,
        commands,
    } = keybinds();
    press(&mut panda, MUTE);
    desktop.send(pressed("Ctrl+Shift+F9")).unwrap();
    wait_for_value(&mut panda, MUTE, "Ctrl+Shift+F9");

    hover(&mut panda, "Back to the usual key for Mute / Unmute");
    wait_for_text(&mut panda, "Reset");
    panda.mouse_leaves();
    press(&mut panda, "Back to the usual key for Mute / Unmute");
    wait_for_value(&mut panda, MUTE, "Ctrl+Alt+M");
    told(&commands, DesktopCommand::Hotkeys(Hotkeys::default()));

    press(&mut panda, MUTE);
    desktop.send(pressed("Ctrl+Alt+D")).unwrap();
    wait_for_value(&mut panda, MUTE, "Ctrl+Alt+D");
    assert_eq!(panda.value_of(DEAFEN).as_deref(), Some("Not set"));
    wait_for_text(
        &mut panda,
        "Ctrl+Alt+D was the key for \"Deafen / Undeafen\", which has none now",
    );
    assert!(panda.notice().is_none(), "said in the row, not in a toast");
    picture(&panda, "22c-a-key-taken");

    press(&mut panda, "Back to the usual key for Deafen / Undeafen");
    wait_for_value(&mut panda, DEAFEN, "Ctrl+Alt+D");
    assert_eq!(panda.value_of(MUTE).as_deref(), Some("Not set"));

    hover(&mut panda, "Remove the key for Deafen / Undeafen");
    wait_for_text(&mut panda, "Unbind");
    panda.mouse_leaves();
    press(&mut panda, "Remove the key for Deafen / Undeafen");
    wait_for_value(&mut panda, DEAFEN, "Not set");
    told(
        &commands,
        DesktopCommand::Hotkeys(Hotkeys {
            mute: None,
            deafen: None,
            ..Hotkeys::default()
        }),
    );
    let saved = panda.saved_settings();
    assert_eq!(saved.hotkeys.deafen, None);
    assert_eq!(saved.hotkeys.share, bound("Ctrl+Alt+S"));

    press(&mut panda, "Reset all keys");
    wait_for_value(&mut panda, MUTE, "Ctrl+Alt+M");
    told(&commands, DesktopCommand::Hotkeys(Hotkeys::default()));
}

#[test]
fn leaving_the_tab_stops_the_capture() {
    let Keybinds {
        mut panda,
        commands,
        desktop: _desktop,
    } = keybinds();
    press(&mut panda, MUTE);
    told(&commands, DesktopCommand::CaptureKey(true));
    press(&mut panda, "General");
    told(&commands, DesktopCommand::CaptureKey(false));
    press(&mut panda, "Keybinds");
    wait_for_value(&mut panda, MUTE, "Ctrl+Alt+M");
}
