#![allow(unsafe_code)]

use std::cell::RefCell;

use windows_sys::Win32::{
    Foundation::{GetLastError, LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentProcessId,
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
            KEYEVENTF_KEYUP, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            CallNextHookEx, GetForegroundWindow, GetWindowThreadProcessId, SetWindowsHookExW,
            UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, WH_KEYBOARD_LL, WM_KEYDOWN,
            WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

use super::{
    keys::{Combination, ALT, CTRL, SHIFT, WIN},
    DesktopEvent,
};
use crate::wake::WakingSender;

const VK_UNASSIGNED: u16 = 0xE8;

struct Binding {
    combination: Combination,
    event: DesktopEvent,
    held: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    fired: Option<DesktopEvent>,
    swallow: bool,
}

#[derive(Default)]
struct Matcher {
    bindings: Vec<Binding>,
}

impl Matcher {
    fn bind(bindings: Vec<(Combination, DesktopEvent)>, held: impl Fn(u16) -> bool) -> Self {
        Self {
            bindings: bindings
                .into_iter()
                .map(|(combination, event)| Binding {
                    held: held(combination.vk),
                    combination,
                    event,
                })
                .collect(),
        }
    }

    fn key(&mut self, vk: u16, pressed: bool, modifiers: u8) -> Outcome {
        let mut outcome = Outcome {
            fired: None,
            swallow: false,
        };
        for binding in self.bindings.iter_mut().filter(|b| b.combination.vk == vk) {
            if !pressed {
                binding.held = false;
                continue;
            }
            if binding.held {
                outcome.swallow = true;
            } else if modifiers == binding.combination.modifiers {
                binding.held = true;
                outcome.swallow = true;
                outcome.fired = Some(binding.event.clone());
            }
        }
        outcome
    }
}

struct Table {
    matcher: Matcher,
    events: WakingSender<DesktopEvent>,
    capturing: bool,
}

thread_local! {
    static TABLE: RefCell<Option<Table>> = const { RefCell::new(None) };
}

pub struct KeyboardHook {
    handle: HHOOK,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl KeyboardHook {
    pub fn install(
        bindings: Vec<(Combination, DesktopEvent)>,
        events: WakingSender<DesktopEvent>,
    ) -> Result<Self, String> {
        TABLE.with_borrow_mut(|table| {
            *table = Some(Table {
                matcher: Matcher::bind(bindings, is_down),
                events,
                capturing: false,
            })
        });
        let handle =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), std::ptr::null_mut(), 0) };
        if handle.is_null() {
            TABLE.with_borrow_mut(|table| *table = None);
            let code = unsafe { GetLastError() };
            return Err(format!("keyboard hook refused (error {code})"));
        }
        Ok(Self {
            handle,
            _not_send: std::marker::PhantomData,
        })
    }

    pub fn rebind(&self, bindings: Vec<(Combination, DesktopEvent)>) {
        TABLE.with_borrow_mut(|table| {
            if let Some(table) = table {
                table.matcher = Matcher::bind(bindings, is_down);
            }
        });
    }

    pub fn capture(&self, on: bool) {
        TABLE.with_borrow_mut(|table| {
            if let Some(table) = table {
                table.capturing = on;
            }
        });
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        unsafe { UnhookWindowsHookEx(self.handle) };
        TABLE.with_borrow_mut(|table| *table = None);
    }
}

fn is_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 }
}

fn modifiers_down() -> u8 {
    [
        (VK_CONTROL, CTRL),
        (VK_MENU, ALT),
        (VK_SHIFT, SHIFT),
        (VK_LWIN, WIN),
        (VK_RWIN, WIN),
    ]
    .into_iter()
    .filter(|(vk, _)| is_down(*vk))
    .fold(0, |bits, (_, bit)| bits | bit)
}

fn is_modifier(vk: u16) -> bool {
    matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

fn our_window_is_in_front() -> bool {
    let mut process = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut process) };
    process != 0 && process == unsafe { GetCurrentProcessId() }
}

fn keep_start_menu_shut() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_UNASSIGNED,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let taps = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe {
        SendInput(
            taps.len() as u32,
            taps.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
}

fn capture(table: &mut Table, vk: u16) -> bool {
    table.capturing = false;
    if !our_window_is_in_front() {
        let _ = table.events.send(DesktopEvent::KeyCaptureCancelled);
        return false;
    }
    let modifiers = modifiers_down();
    let _ = table.events.send(if vk == VK_ESCAPE && modifiers == 0 {
        DesktopEvent::KeyCaptureCancelled
    } else {
        DesktopEvent::KeyCaptured(Combination { vk, modifiers })
    });
    true
}

fn on_key(vk: u16, pressed: bool) -> bool {
    TABLE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return false;
        };
        let Some(table) = guard.as_mut() else {
            return false;
        };
        if table.capturing && pressed && !is_modifier(vk) {
            return capture(table, vk);
        }
        let modifiers = if pressed { modifiers_down() } else { 0 };
        let outcome = table.matcher.key(vk, pressed, modifiers);
        if let Some(event) = outcome.fired {
            let _ = table.events.send(event);
            if modifiers & WIN != 0 {
                keep_start_menu_shut();
            }
        }
        outcome.swallow
    })
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let key = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let pressed = match wparam as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some(true),
            WM_KEYUP | WM_SYSKEYUP => Some(false),
            _ => None,
        };
        if let Some(pressed) = pressed {
            if on_key(key.vkCode as u16, pressed) {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: u16 = 0x4D;

    fn matcher() -> Matcher {
        Matcher::bind(
            vec![(
                Combination {
                    vk: M,
                    modifiers: CTRL | ALT,
                },
                DesktopEvent::ToggleMute,
            )],
            |_| false,
        )
    }

    #[test]
    fn a_combination_fires_once_while_held_and_is_kept_from_other_programs() {
        let mut keys = matcher();
        let first = keys.key(M, true, CTRL | ALT);
        assert_eq!(first.fired, Some(DesktopEvent::ToggleMute));
        assert!(first.swallow);
        let repeat = keys.key(M, true, CTRL | ALT);
        assert_eq!(
            repeat,
            Outcome {
                fired: None,
                swallow: true
            }
        );
        assert!(!keys.key(M, false, 0).swallow);
        assert!(keys.key(M, true, CTRL | ALT).fired.is_some());
    }

    #[test]
    fn other_modifiers_leave_the_key_to_other_programs() {
        let mut keys = matcher();
        assert_eq!(
            keys.key(M, true, CTRL),
            Outcome {
                fired: None,
                swallow: false
            }
        );
        assert!(!keys.key(M, true, CTRL | ALT | SHIFT).swallow);
    }

    #[test]
    fn letting_go_of_the_modifiers_first_does_not_leak_the_repeats() {
        let mut keys = matcher();
        keys.key(M, true, CTRL | ALT);
        assert!(keys.key(M, true, 0).swallow);
        assert!(keys.key(M, true, 0).fired.is_none());
    }

    #[test]
    fn a_key_already_down_when_bound_does_not_fire_until_pressed_again() {
        let mut keys = Matcher::bind(
            vec![(
                Combination {
                    vk: M,
                    modifiers: CTRL,
                },
                DesktopEvent::ToggleMute,
            )],
            |_| true,
        );
        assert!(keys.key(M, true, CTRL).fired.is_none());
        keys.key(M, false, 0);
        assert!(keys.key(M, true, CTRL).fired.is_some());
    }
}
