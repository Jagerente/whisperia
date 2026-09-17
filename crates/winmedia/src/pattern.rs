use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Once,
    },
    thread::JoinHandle,
};

use windows::{
    core::w,
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            Dwm::DwmFlush,
            Gdi::{CreateSolidBrush, DeleteObject, FillRect, GetDC, ReleaseDC},
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
            PeekMessageW, RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage,
            CW_USEDEFAULT, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SW_MINIMIZE,
            WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        },
    },
};

use crate::{Context, Error, SourceId};

const CLASS: windows::core::PCWSTR = w!("WhisperiaTestPattern");
const SIZE: (i32, i32) = (960, 540);

pub struct Pattern {
    source: SourceId,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Pattern {
    pub fn show() -> Result<Self, Error> {
        let (opened_tx, opened) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("ww-test-pattern".into())
            .spawn(move || match open() {
                Ok(window) => {
                    let _ = opened_tx.send(Ok(window.0 as isize));
                    animate(window, &stopping);

                    let _ = unsafe { DestroyWindow(window) };
                }
                Err(e) => {
                    let _ = opened_tx.send(Err(e));
                }
            })?;
        let handle = opened
            .recv()
            .map_err(|_| Error::Stillborn("test pattern"))??;
        Ok(Self {
            source: SourceId::Window {
                handle,
                process: std::process::id(),
            },
            stop,
            thread: Some(thread),
        })
    }

    pub fn source(&self) -> SourceId {
        self.source
    }

    fn window(&self) -> HWND {
        match self.source {
            SourceId::Window { handle, .. } => HWND(handle as *mut _),
            SourceId::Screen { .. } => HWND::default(),
        }
    }

    pub fn resize(&self, width: i32, height: i32) -> Result<(), Error> {
        unsafe {
            SetWindowPos(
                self.window(),
                None,
                0,
                0,
                width,
                height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
        .context("resizing the test pattern")
    }

    pub fn minimize(&self) {
        let _ = unsafe { ShowWindow(self.window(), SW_MINIMIZE) };
    }

    pub fn is_minimized(&self) -> bool {
        crate::sources::is_minimized(self.window())
    }
}

impl Drop for Pattern {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

unsafe extern "system" fn answer(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(window, message, w, l) }
}

fn open() -> Result<HWND, Error> {
    static REGISTERED: Once = Once::new();
    REGISTERED.call_once(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(answer),
            lpszClassName: CLASS,
            ..Default::default()
        };

        unsafe { RegisterClassW(&class) };
    });

    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Whisperia test pattern"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            SIZE.0,
            SIZE.1,
            None,
            None,
            None,
            None,
        )
    }
    .context("opening the test pattern")
}

fn animate(window: HWND, stop: &AtomicBool) {
    let mut frame = 0u32;
    while !stop.load(Ordering::Relaxed) {
        pump();
        paint(window, frame);
        frame = frame.wrapping_add(1);

        if unsafe { DwmFlush() }.is_err() {
            std::thread::yield_now();
        }
    }
}

fn pump() {
    let mut message = MSG::default();

    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
        let _ = unsafe { TranslateMessage(&message) };

        unsafe { DispatchMessageW(&message) };
    }
}

fn paint(window: HWND, frame: u32) {
    let mut area = RECT::default();

    if unsafe { GetClientRect(window, &mut area) }.is_err() {
        return;
    }
    let shade = (frame % 200) as u8 + 30;
    let colour = COLORREF(u32::from(shade) | u32::from(255 - shade) << 8 | 0x60 << 16);

    let canvas = unsafe { GetDC(Some(window)) };

    let brush = unsafe { CreateSolidBrush(colour) };

    unsafe { FillRect(canvas, &area, brush) };

    let _ = unsafe { DeleteObject(brush.into()) };

    unsafe { ReleaseDC(Some(window), canvas) };
}
