use std::{cell::RefCell, sync::mpsc, thread::JoinHandle};

use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW, CHILDID_SELF,
            EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MOVESIZEEND,
            EVENT_SYSTEM_MOVESIZESTART, MSG, OBJID_WINDOW, PM_NOREMOVE, WINEVENT_OUTOFCONTEXT,
            WM_QUIT,
        },
    },
};

use crate::{
    sources::{dpi_aware, is_minimized, size_of_window},
    Error, SourceId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowNews {
    Resized((u32, u32)),
    Minimized,
    Restored,
}

pub type Tell = Box<dyn FnMut(WindowNews) + Send>;

pub struct WindowWatch {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl WindowWatch {
    pub fn start(id: SourceId, tell: Tell) -> Result<Option<Self>, Error> {
        let SourceId::Window { handle, process } = id else {
            return Ok(None);
        };
        let (started_tx, started) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("ww-window-watch".into())
            .spawn(move || {
                dpi_aware();
                let mut message = MSG::default();

                let _ = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE) };
                let Some(_hooks) = Hooks::set(process) else {
                    let _ = started_tx.send(None);
                    return;
                };
                WATCHED.set(Some(Watched {
                    window: HWND(handle as *mut _),
                    moving: false,
                    minimized: false,
                    size: None,
                    tell,
                }));

                let _ = started_tx.send(Some(unsafe { GetCurrentThreadId() }));
                look();
                pump();
                WATCHED.set(None);
            })?;
        match started.recv() {
            Ok(Some(thread_id)) => Ok(Some(Self {
                thread_id,
                thread: Some(thread),
            })),
            _ => {
                let _ = thread.join();
                Err(Error::Stillborn("window watch"))
            }
        }
    }
}

impl Drop for WindowWatch {
    fn drop(&mut self) {
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Watched {
    window: HWND,
    moving: bool,
    minimized: bool,
    size: Option<(u32, u32)>,
    tell: Tell,
}

impl Watched {
    fn look(&mut self) {
        let minimized = is_minimized(self.window);
        if minimized != self.minimized {
            self.minimized = minimized;
            (self.tell)(match minimized {
                true => WindowNews::Minimized,
                false => WindowNews::Restored,
            });
        }
        if minimized || self.moving {
            return;
        }
        let size = size_of_window(self.window);
        if size.is_some() && size != self.size {
            self.size = size;
            if let Some(size) = size {
                (self.tell)(WindowNews::Resized(size));
            }
        }
    }
}

thread_local! {
    static WATCHED: RefCell<Option<Watched>> = const { RefCell::new(None) };
}

fn look() {
    WATCHED.with_borrow_mut(|watched| {
        if let Some(watched) = watched {
            watched.look();
        }
    });
}

struct Hooks([HWINEVENTHOOK; 2]);

impl Hooks {
    fn set(process: u32) -> Option<Self> {
        let hook = |first: u32, last: u32| {
            let hook = unsafe {
                SetWinEventHook(
                    first,
                    last,
                    None,
                    Some(heard),
                    process,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            (!hook.is_invalid()).then_some(hook)
        };
        let system = hook(EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MINIMIZEEND)?;
        let Some(location) = hook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE) else {
            let _ = unsafe { UnhookWinEvent(system) };
            return None;
        };
        Some(Self([system, location]))
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        for hook in self.0 {
            let _ = unsafe { UnhookWinEvent(hook) };
        }
    }
}

unsafe extern "system" fn heard(
    _: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    object: i32,
    child: i32,
    _: u32,
    _: u32,
) {
    if object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32 {
        return;
    }
    WATCHED.with_borrow_mut(|watched| {
        let Some(watched) = watched.as_mut().filter(|w| w.window == window) else {
            return;
        };
        match event {
            EVENT_SYSTEM_MOVESIZESTART => watched.moving = true,
            EVENT_SYSTEM_MOVESIZEEND => watched.moving = false,
            _ => {}
        }
        watched.look();
    });
}

fn pump() {
    let mut message = MSG::default();

    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        unsafe { DispatchMessageW(&message) };
    }
}
