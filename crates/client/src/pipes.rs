#![allow(unsafe_code)]

use std::time::Duration;

use windows_sys::Win32::{
    System::Pipes::WaitNamedPipeW,
    UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHUNGAPPTIMEOUT},
};

#[derive(Debug, Clone, Copy)]
pub struct Patience {
    millis: u32,
}

impl Patience {
    pub fn before_not_responding() -> std::io::Result<Self> {
        let mut millis: u32 = 0;
        let read =
            unsafe { SystemParametersInfoW(SPI_GETHUNGAPPTIMEOUT, 0, (&raw mut millis).cast(), 0) };
        if read == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Self { millis })
        }
    }

    pub fn duration(self) -> Duration {
        Duration::from_millis(self.millis.into())
    }
}

pub fn wait_for_a_free_end(name: &str, patience: Patience) -> std::io::Result<()> {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let waited = unsafe { WaitNamedPipeW(wide.as_ptr(), patience.millis) };
    if waited == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
