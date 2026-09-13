use std::{
    sync::mpsc::{self, Sender},
    thread::JoinHandle,
};

use wasapi::{DeviceEnumerator, DeviceEventCallbacks, Direction, Role, WasapiError};

use super::{AudioError, DeviceNews, DeviceWatch, DevicesChanged, Flow};
use crate::com::Com;

struct Watching {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.stop = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn callbacks(changed: &DevicesChanged) -> DeviceEventCallbacks {
    let mut callbacks = DeviceEventCallbacks::new();
    let tell = changed.clone();
    callbacks.set_device_added_callback(move |_| tell(DeviceNews::Changed));
    let tell = changed.clone();
    callbacks.set_device_removed_callback(move |_| tell(DeviceNews::Changed));
    let tell = changed.clone();
    callbacks.set_device_state_callback(move |_, _| tell(DeviceNews::Changed));
    let tell = changed.clone();
    callbacks.set_default_device_callback(move |direction, role, _| {
        if role == Role::Console {
            tell(DeviceNews::NewDefault(match direction {
                Direction::Capture => Flow::Capture,
                Direction::Render => Flow::Playback,
            }));
        }
    });
    callbacks
}

pub(super) fn watch(changed: DevicesChanged) -> Result<DeviceWatch, AudioError> {
    let (stop, stopped) = mpsc::channel::<()>();
    let (ready_tx, ready) = mpsc::channel::<Result<(), WasapiError>>();
    let thread = std::thread::Builder::new()
        .name("ww-devices".into())
        .spawn(move || {
            let _com = Com::enter();
            let registered = DeviceEnumerator::new()
                .and_then(|devices| devices.register_notification_callback(callbacks(&changed)));
            match registered {
                Ok(registration) => {
                    let _ = ready_tx.send(Ok(()));
                    let _ = stopped.recv();
                    drop(registration);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })
        .map_err(|e| AudioError::Backend(format!("cannot start a thread: {e}")))?;
    let watching = Watching {
        stop: Some(stop),
        thread: Some(thread),
    };
    match ready.recv() {
        Ok(Ok(())) => Ok(Box::new(watching)),
        Ok(Err(e)) => Err(AudioError::Backend(e.to_string())),
        Err(_) => Err(AudioError::Backend(
            "the device watch stopped while starting".into(),
        )),
    }
}
