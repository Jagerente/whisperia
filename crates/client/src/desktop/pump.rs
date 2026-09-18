#![allow(unsafe_code)]

use windows_sys::Win32::{
    Foundation::POINT,
    System::Threading::{GetCurrentThreadId, INFINITE},
    UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, PostThreadMessageW,
        TranslateMessage, MSG, MWMO_INPUTAVAILABLE, PM_NOREMOVE, PM_REMOVE, QS_ALLINPUT, WM_APP,
    },
};

#[derive(Debug, Clone, Copy)]
pub(super) struct Doorbell(u32);

impl Doorbell {
    pub(super) fn of_this_thread() -> Self {
        let mut message = empty_message();
        unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
        Self(unsafe { GetCurrentThreadId() })
    }

    pub(super) fn ring(self) {
        unsafe { PostThreadMessageW(self.0, WM_APP, 0, 0) };
    }
}

fn empty_message() -> MSG {
    MSG {
        hwnd: std::ptr::null_mut(),
        message: 0,
        wParam: 0,
        lParam: 0,
        time: 0,
        pt: POINT { x: 0, y: 0 },
    }
}

pub(super) fn pump_messages(mut rung: impl FnMut() -> bool) {
    loop {
        unsafe {
            MsgWaitForMultipleObjectsEx(
                0,
                std::ptr::null(),
                INFINITE,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        let mut message = empty_message();
        while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe { TranslateMessage(&message) };
            unsafe { DispatchMessageW(&message) };
        }
        if !rung() {
            return;
        }
    }
}
