#![allow(unsafe_code)]

use std::{cell::RefCell, sync::OnceLock};

use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        Recovery::{
            RegisterApplicationRestart, RESTART_NO_CRASH, RESTART_NO_HANG, RESTART_NO_REBOOT,
        },
    },
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WM_ENDSESSION, WNDCLASSW,
    },
};

type OnEnd = Box<dyn FnOnce()>;

thread_local! {
    static ON_END: RefCell<Option<OnEnd>> = const { RefCell::new(None) };
}

static CLASS: OnceLock<u16> = OnceLock::new();

pub struct SessionEnd {
    window: HWND,
}

impl SessionEnd {
    pub fn watch(on_end: impl FnOnce() + 'static) -> Result<Self, String> {
        let name: Vec<u16> = "WhisperiaSessionEnd".encode_utf16().chain([0]).collect();
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let class = *CLASS.get_or_init(|| {
            let description = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: name.as_ptr(),
            };
            unsafe { RegisterClassW(&description) }
        });
        if class == 0 {
            return Err("no window class to hear the end of the session".into());
        }
        ON_END.with_borrow_mut(|slot| *slot = Some(Box::new(on_end)));
        let window = unsafe {
            CreateWindowExW(
                0,
                name.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if window.is_null() {
            ON_END.with_borrow_mut(|slot| *slot = None);
            let code = unsafe { GetLastError() };
            return Err(format!(
                "no window to hear the end of the session (error {code})"
            ));
        }
        Ok(Self { window })
    }
}

impl Drop for SessionEnd {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.window) };
        ON_END.with_borrow_mut(|slot| *slot = None);
    }
}

pub fn restart_after_updates(command_line: &str) -> Result<(), String> {
    let wide: Vec<u16> = command_line.encode_utf16().chain([0]).collect();
    let code = unsafe {
        RegisterApplicationRestart(
            wide.as_ptr(),
            RESTART_NO_CRASH | RESTART_NO_HANG | RESTART_NO_REBOOT,
        )
    };
    if code == 0 {
        Ok(())
    } else {
        Err(format!(
            "an update will not start Whisperia again (error {code:#x})"
        ))
    }
}

fn session_ends() {
    let on_end = ON_END.with(|slot| slot.try_borrow_mut().ok().and_then(|mut slot| slot.take()));
    if let Some(on_end) = on_end {
        on_end();
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_ENDSESSION && wparam != 0 {
        session_ends();
        return 0;
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, ENDSESSION_CLOSEAPP};

    use super::*;

    fn end_session(watch: &SessionEnd, ending: bool) {
        unsafe {
            SendMessageW(
                watch.window,
                WM_ENDSESSION,
                usize::from(ending),
                ENDSESSION_CLOSEAPP as isize,
            )
        };
    }

    fn watched() -> (SessionEnd, Rc<Cell<u32>>) {
        let ended = Rc::new(Cell::new(0));
        let counting = ended.clone();
        let watch = SessionEnd::watch(move || counting.set(counting.get() + 1))
            .expect("a window for the end of the session");
        (watch, ended)
    }

    #[test]
    fn the_end_of_the_session_is_heard_once() {
        let (watch, ended) = watched();
        end_session(&watch, true);
        end_session(&watch, true);
        assert_eq!(ended.get(), 1);
    }

    #[test]
    fn an_update_restarts_with_the_command_line_and_nothing_else_does() {
        use windows_sys::Win32::System::{
            Recovery::GetApplicationRestartSettings, Threading::GetCurrentProcess,
        };

        restart_after_updates(r#"--settings-dir "D:\My Settings" --hidden"#)
            .expect("restarting after updates");
        let mut command_line = [0u16; 1024];
        let mut length = command_line.len() as u32;
        let mut flags = 0;
        let code = unsafe {
            GetApplicationRestartSettings(
                GetCurrentProcess(),
                command_line.as_mut_ptr(),
                &mut length,
                &mut flags,
            )
        };
        assert_eq!(code, 0);
        let command_line = String::from_utf16_lossy(&command_line[..length as usize - 1]);
        assert_eq!(command_line, r#"--settings-dir "D:\My Settings" --hidden"#);
        assert_eq!(
            flags,
            RESTART_NO_CRASH | RESTART_NO_HANG | RESTART_NO_REBOOT
        );
    }

    #[test]
    fn a_session_end_called_off_is_not_heard() {
        let (watch, ended) = watched();
        end_session(&watch, false);
        assert_eq!(ended.get(), 0);
    }
}
