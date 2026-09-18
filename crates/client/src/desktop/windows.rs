use std::{
    str::FromStr,
    sync::mpsc::{Receiver, Sender, TryRecvError},
    thread::JoinHandle,
};

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use tray_icon::{
    menu::{Icon as MenuIcon, IconMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use super::{
    hook::KeyboardHook,
    keys,
    pump::{pump_messages, Doorbell},
    DesktopCommand, DesktopEvent, DesktopLink, DesktopShare, DesktopStatus, Presence,
};
use crate::{
    config::{HotkeyAction, Hotkeys},
    wake::{self, Waker, WakingSender},
};

const ICON_SIZE: u32 = 32;
const MENU_OPEN: &str = "open";
const MENU_MUTE: &str = "mute";
const MENU_DEAFEN: &str = "deafen";
const MENU_SHARE: &str = "share";
const MENU_SETTINGS: &str = "settings";
const MENU_QUIT: &str = "quit";
const MENU_WATCH: &str = "watch:";
const WATCH_POSITION: usize = 5;

struct Guard {
    commands: Option<Sender<DesktopCommand>>,
    doorbell: Doorbell,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.commands = None;
        self.doorbell.ring();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn start(hotkeys: Hotkeys, waker: &Waker) -> Option<DesktopLink> {
    let (events_tx, events) = wake::channel(waker);
    let (commands_tx, commands) = std::sync::mpsc::channel();
    let (doorbell_tx, doorbell) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("ww-desktop".into())
        .spawn(move || {
            let _ = doorbell_tx.send(Doorbell::of_this_thread());
            run(hotkeys, events_tx, commands)
        })
        .ok()?;
    let doorbell = doorbell.recv().ok()?;

    let telling = commands_tx.clone();
    Some(DesktopLink {
        events,
        tell: Box::new(move |command| {
            let _ = telling.send(command);
            doorbell.ring();
        }),
        _guard: Some(Box::new(Guard {
            commands: Some(commands_tx),
            doorbell,
            thread: Some(thread),
        })),
    })
}

fn run(hotkeys: Hotkeys, events: WakingSender<DesktopEvent>, commands: Receiver<DesktopCommand>) {
    let mut handler = Handler {
        hotkeys,
        events,
        parts: None,
        shown: None,
    };
    handler.parts = Some(handler.create_parts());
    pump_messages(|| loop {
        match commands.try_recv() {
            Ok(DesktopCommand::Status(status)) => {
                handler.show(&status);
                handler.shown = Some(status);
            }
            Ok(DesktopCommand::Hotkeys(hotkeys)) => handler.rebind(hotkeys),
            Ok(DesktopCommand::CaptureKey(on)) => handler.capture_key(on),
            Err(TryRecvError::Disconnected) => {
                handler.parts = None;
                return false;
            }
            Err(TryRecvError::Empty) => return true,
        }
    });
}

struct Parts {
    tray: Option<TrayIcon>,
    menu: Menu,
    mute: MenuItem,
    deafen: MenuItem,
    share: MenuItem,
    watch: Vec<MenuItem>,
    shown_shares: Vec<DesktopShare>,
    hook: Option<KeyboardHook>,
    _fallback: Option<GlobalHotKeyManager>,
}

struct Handler {
    hotkeys: Hotkeys,
    events: WakingSender<DesktopEvent>,
    parts: Option<Parts>,
    shown: Option<DesktopStatus>,
}

type Wanted = Vec<(keys::Combination, DesktopEvent)>;

impl Handler {
    fn problem(&self, text: String) {
        tracing::warn!("{text}");
        let _ = self.events.send(DesktopEvent::Problem(text));
    }

    fn wanted(&self) -> Wanted {
        HotkeyAction::ALL
            .into_iter()
            .filter_map(|action| {
                let event = match action {
                    HotkeyAction::Mute => DesktopEvent::ToggleMute,
                    HotkeyAction::Deafen => DesktopEvent::ToggleDeafen,
                    HotkeyAction::Share => DesktopEvent::ToggleShare,
                };
                Some((self.hotkeys.get(action)?, event))
            })
            .collect()
    }

    fn rebind(&mut self, hotkeys: Hotkeys) {
        self.hotkeys = hotkeys;
        let wanted = self.wanted();
        let Some(mut parts) = self.parts.take() else {
            return;
        };
        match &parts.hook {
            Some(hook) => hook.rebind(wanted.clone()),
            None => {
                parts._fallback = None;
                parts._fallback = self.register_fallback(&wanted);
            }
        }
        self.parts = Some(parts);

        if let Some(status) = self.shown.clone() {
            self.show(&status);
        }
    }

    fn capture_key(&self, on: bool) {
        match self.parts.as_ref().and_then(|parts| parts.hook.as_ref()) {
            Some(hook) => hook.capture(on),
            None if on => {
                let _ = self.events.send(DesktopEvent::KeyCaptureCancelled);
                self.problem("Keys cannot be set on this PC".into());
            }
            None => {}
        }
    }

    fn create_parts(&self) -> Parts {
        let events = self.events.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(what) = menu_event(&event.id().0) {
                let _ = events.send(what);
            }
        }));

        let mute = MenuItem::with_id(MENU_MUTE, "Mute microphone", false, None);
        let deafen = MenuItem::with_id(MENU_DEAFEN, "Deafen", false, None);
        let share = MenuItem::with_id(MENU_SHARE, "Share my screen...", false, None);
        let settings = MenuItem::with_id(MENU_SETTINGS, "Settings...", true, None);
        let quit = MenuItem::with_id(MENU_QUIT, "Quit Whisperia", true, None);
        let title = IconMenuItem::with_id(MENU_OPEN, "Whisperia", true, menu_icon(), None);
        let menu = Menu::new();
        let filled = menu.append_items(&[
            &title,
            &PredefinedMenuItem::separator(),
            &mute,
            &deafen,
            &share,
            &settings,
            &PredefinedMenuItem::separator(),
            &quit,
        ]);
        let events = self.events.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = events.send(DesktopEvent::ShowWindow);
            }
        }));
        let tray = filled
            .map_err(|e| e.to_string())
            .and_then(|()| {
                TrayIconBuilder::new()
                    .with_menu(Box::new(menu.clone()))
                    .with_menu_on_left_click(false)
                    .with_tooltip("Whisperia")
                    .with_icon(icon(Presence::Away, false).ok_or("icon")?)
                    .build()
                    .map_err(|e| e.to_string())
            })
            .map_err(|e| {
                tracing::warn!("no tray icon: {e}");
                let _ = self.events.send(DesktopEvent::NoTray);
            })
            .ok();

        let wanted = self.wanted();
        let hook = KeyboardHook::install(wanted.clone(), self.events.clone())
            .map_err(|e| tracing::warn!("{e}; falling back to RegisterHotKey"))
            .ok();
        let manager = match hook {
            Some(_) => None,
            None => self.register_fallback(&wanted),
        };

        Parts {
            tray,
            menu,
            mute,
            deafen,
            share,
            watch: Vec::new(),
            shown_shares: Vec::new(),
            hook,
            _fallback: manager,
        }
    }

    fn register_fallback(&self, wanted: &Wanted) -> Option<GlobalHotKeyManager> {
        if wanted.is_empty() {
            return None;
        }
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| self.problem(format!("Global hotkeys are unavailable: {e}")))
            .ok()?;
        let mut registered = Vec::new();
        for (combination, event) in wanted {
            let text = keys::name(*combination).unwrap_or_default();
            let Some(hotkey) = hotkey_of(*combination) else {
                self.problem(format!("Hotkey {text} cannot be used on this PC"));
                continue;
            };
            match manager.register(hotkey) {
                Ok(()) => registered.push((hotkey.id(), event.clone())),
                Err(_) => self.problem(format!(
                    "Hotkey {text} is taken by another program - pick another one"
                )),
            }
        }
        let events = self.events.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state() != HotKeyState::Pressed {
                return;
            }
            if let Some((_, what)) = registered.iter().find(|(id, _)| *id == event.id()) {
                let _ = events.send(what.clone());
            }
        }));
        Some(manager)
    }

    fn show(&mut self, status: &DesktopStatus) {
        let Some(parts) = &mut self.parts else {
            return;
        };
        let in_room = status.presence != Presence::Away;
        let with_key = |label: &str, key: Option<keys::Combination>| match key.and_then(keys::name)
        {
            Some(key) => format!("{label}\t{key}"),
            None => label.to_owned(),
        };
        parts.mute.set_enabled(in_room);
        parts.deafen.set_enabled(in_room);
        parts.mute.set_text(with_key(
            match status.presence {
                Presence::Muted | Presence::Deafened => "Unmute microphone",
                _ => "Mute microphone",
            },
            self.hotkeys.mute,
        ));
        parts.deafen.set_text(with_key(
            match status.presence {
                Presence::Deafened => "Undeafen",
                _ => "Deafen",
            },
            self.hotkeys.deafen,
        ));
        parts.share.set_enabled(in_room);
        parts.share.set_text(with_key(
            if status.sharing {
                "Stop sharing my screen"
            } else {
                "Share my screen..."
            },
            self.hotkeys.share,
        ));
        if parts.shown_shares != status.shares {
            for item in parts.watch.drain(..) {
                let _ = parts.menu.remove(&item);
            }
            for share in &status.shares {
                let text = if share.watched {
                    format!("Stop watching {}", share.name)
                } else {
                    format!("Watch {}'s screen", share.name)
                };
                let id = format!("{MENU_WATCH}{}", share.sharer);
                let item = MenuItem::with_id(id, text, true, None);
                let at = WATCH_POSITION + parts.watch.len();
                if parts.menu.insert(&item, at).is_ok() {
                    parts.watch.push(item);
                }
            }
            parts.shown_shares = status.shares.clone();
        }
        if let Some(tray) = &parts.tray {
            let _ = tray.set_icon(icon(status.presence, status.sharing));
        }
    }
}

fn menu_event(id: &str) -> Option<DesktopEvent> {
    Some(match id {
        MENU_OPEN => DesktopEvent::ShowWindow,
        MENU_MUTE => DesktopEvent::ToggleMute,
        MENU_DEAFEN => DesktopEvent::ToggleDeafen,
        MENU_SHARE => DesktopEvent::ToggleShare,
        MENU_SETTINGS => DesktopEvent::OpenSettings,
        MENU_QUIT => DesktopEvent::Quit,
        other => DesktopEvent::ToggleWatch(other.strip_prefix(MENU_WATCH)?.parse().ok()?),
    })
}

fn hotkey_of(combination: keys::Combination) -> Option<HotKey> {
    let code = Code::from_str(&keys::code_name(combination.vk)?).ok()?;
    let modifiers = [
        (keys::CTRL, Modifiers::CONTROL),
        (keys::ALT, Modifiers::ALT),
        (keys::SHIFT, Modifiers::SHIFT),
        (keys::WIN, Modifiers::SUPER),
    ]
    .into_iter()
    .filter(|(bit, _)| combination.modifiers & bit != 0)
    .fold(Modifiers::empty(), |all, (_, modifier)| all | modifier);
    Some(HotKey::new(Some(modifiers), code))
}

fn icon(presence: Presence, sharing: bool) -> Option<Icon> {
    Icon::from_rgba(glyph_rgba(presence, sharing), ICON_SIZE, ICON_SIZE).ok()
}

fn menu_icon() -> Option<MenuIcon> {
    MenuIcon::from_rgba(glyph_rgba(Presence::Live, false), ICON_SIZE, ICON_SIZE).ok()
}

fn glyph_rgba(presence: Presence, sharing: bool) -> Vec<u8> {
    let colour: [u8; 3] = match presence {
        Presence::Away => [0x7f, 0x8c, 0x8d],
        Presence::Live => [0xff, 0x00, 0x54],
        Presence::Muted => [0xe7, 0x4c, 0x3c],
        Presence::Deafened => [0x96, 0x28, 0x1b],
    };
    let (cx, cy) = (0.36f32, 0.5f32);
    let arc = |x: f32, y: f32, radius: f32| {
        let (dx, dy) = (x - cx, y - cy);
        let distance = (dx * dx + dy * dy).sqrt();
        (distance - radius).abs() <= 0.05 && (-dy).atan2(dx).abs() <= 42f32.to_radians()
    };
    let glyph = |x: f32, y: f32| {
        let (dx, dy) = (x - cx, y - cy);
        dx * dx + dy * dy <= 0.11 * 0.11 || arc(x, y, 0.24) || arc(x, y, 0.4)
    };
    let mark = |x: f32, y: f32| {
        let (dx, dy) = (x - 0.5, y - 0.5);
        let slash = (dx + dy).abs() < 0.06;
        let backslash = (dx - dy).abs() < 0.06;
        (dx * dx + dy * dy).sqrt() < 0.5
            && match presence {
                Presence::Muted => slash,
                Presence::Deafened => slash || backslash,
                _ => false,
            }
    };

    const SHARING: [u8; 3] = [0x34, 0xc7, 0x59];
    let dot = |x: f32, y: f32| {
        let (dx, dy) = (x - 0.76, y - 0.76);
        sharing && dx * dx + dy * dy <= 0.23 * 0.23
    };

    const SS: u32 = 4;
    let mut rgba = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for py in 0..ICON_SIZE {
        for px in 0..ICON_SIZE {
            let (mut glyph_hits, mut mark_hits, mut dot_hits) = (0u32, 0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / ICON_SIZE as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / ICON_SIZE as f32;
                    if dot(x, y) {
                        dot_hits += 1;
                    } else if mark(x, y) {
                        mark_hits += 1;
                    } else if glyph(x, y) {
                        glyph_hits += 1;
                    }
                }
            }
            let total = (SS * SS) as f32;
            let (glyph_part, mark_part, dot_part) = (
                glyph_hits as f32 / total,
                mark_hits as f32 / total,
                dot_hits as f32 / total,
            );
            let alpha = glyph_part + mark_part + dot_part;

            let mix = |channel: usize| {
                if alpha == 0.0 {
                    0
                } else {
                    ((colour[channel] as f32 * glyph_part
                        + 255.0 * mark_part
                        + SHARING[channel] as f32 * dot_part)
                        / alpha) as u8
                }
            };
            rgba.extend_from_slice(&[mix(0), mix(1), mix(2), (alpha * 255.0) as u8]);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glyph_becomes_an_icon_for_the_tray_and_for_the_menu() {
        assert_eq!(
            glyph_rgba(Presence::Live, false).len(),
            (ICON_SIZE * ICON_SIZE * 4) as usize
        );
        assert!(icon(Presence::Muted, true).is_some());
        assert!(menu_icon().is_some());
    }

    #[test]
    fn every_menu_item_says_what_it_does() {
        assert_eq!(menu_event(MENU_OPEN), Some(DesktopEvent::ShowWindow));
        assert_eq!(menu_event(MENU_QUIT), Some(DesktopEvent::Quit));
        assert_eq!(menu_event("watch:7"), Some(DesktopEvent::ToggleWatch(7)));
        assert_eq!(menu_event("watch:Raccoon"), None);
        assert_eq!(menu_event(""), None);
    }
}
