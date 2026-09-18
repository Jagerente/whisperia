#![allow(unsafe_code)]

use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

pub fn show(text: &str) {
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain([0]).collect() };
    let (text, caption) = (wide(text), wide("Whisperia"));
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}
