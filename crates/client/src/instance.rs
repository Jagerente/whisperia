use std::{path::Path, sync::mpsc::Receiver};

use crate::wake::Waker;

#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    #[error("cannot start a runtime for the single-instance pipe: {0}")]
    Runtime(std::io::Error),
    #[error("cannot create the single-instance pipe: {0}")]
    Create(std::io::Error),
    #[error("cannot start the single-instance thread: {0}")]
    Thread(std::io::Error),
    #[error("another copy holds the settings but does not answer: {0}")]
    Silent(std::io::Error),
}

pub enum Claim {
    First(Instance),
    Elsewhere,
}

pub struct Instance {
    calls: Receiver<()>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    listening: Option<std::thread::JoinHandle<()>>,
}

impl Instance {
    pub fn called(&self) -> bool {
        self.calls.try_iter().count() > 0
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(listening) = self.listening.take() {
            let _ = listening.join();
        }
    }
}

#[cfg(windows)]
fn pipe_name(settings: &Path) -> String {
    use sha2::{Digest, Sha256};

    let absolute = std::path::absolute(settings).unwrap_or_else(|_| settings.to_path_buf());
    let text = absolute
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!(r"\\.\pipe\whisperia-{hex}")
}

#[cfg(not(windows))]
pub fn claim(_settings: &Path, _waker: &Waker) -> Result<Claim, InstanceError> {
    let (_calls_to, calls) = std::sync::mpsc::channel();
    Ok(Claim::First(Instance {
        calls,
        stop: None,
        listening: None,
    }))
}

#[cfg(windows)]
pub fn claim(settings: &Path, waker: &Waker) -> Result<Claim, InstanceError> {
    use std::time::Duration;

    use tokio::net::windows::named_pipe::ServerOptions;

    const ATTEMPTS: u32 = 20;
    const BETWEEN: Duration = Duration::from_millis(100);

    let name = pipe_name(settings);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(InstanceError::Runtime)?;
    let mut last = std::io::Error::from(std::io::ErrorKind::NotFound);
    for _ in 0..ATTEMPTS {
        let first = {
            let _inside = runtime.enter();
            ServerOptions::new().first_pipe_instance(true).create(&name)
        };
        match first {
            Ok(server) => return listen(runtime, name, server, waker).map(Claim::First),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
            Err(e) => return Err(InstanceError::Create(e)),
        }
        match runtime.block_on(tell(&name)) {
            Ok(()) => return Ok(Claim::Elsewhere),
            Err(e) => last = e,
        }
        std::thread::sleep(BETWEEN);
    }
    Err(InstanceError::Silent(last))
}

#[cfg(windows)]
const ANSWER: u8 = b'!';

#[cfg(windows)]
async fn tell(name: &str) -> std::io::Result<()> {
    use tokio::{io::AsyncReadExt, net::windows::named_pipe::ClientOptions};

    let mut pipe = ClientOptions::new().open(name)?;
    let mut answer = [0u8; 1];
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        pipe.read_exact(&mut answer),
    )
    .await
    .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
    if answer[0] == ANSWER {
        Ok(())
    } else {
        Err(std::io::Error::from(std::io::ErrorKind::InvalidData))
    }
}

#[cfg(windows)]
fn listen(
    runtime: tokio::runtime::Runtime,
    name: String,
    server: tokio::net::windows::named_pipe::NamedPipeServer,
    waker: &Waker,
) -> Result<Instance, InstanceError> {
    use tokio::{io::AsyncWriteExt, net::windows::named_pipe::ServerOptions};

    let (calls_to, calls) = crate::wake::channel(waker);
    let (stop_to, stop) = tokio::sync::oneshot::channel::<()>();
    let serve = async move {
        let mut server = server;
        loop {
            let connected = server.connect().await;
            let next = match ServerOptions::new().create(&name) {
                Ok(next) => next,
                Err(e) => {
                    tracing::warn!("the single-instance pipe stops listening: {e}");
                    return;
                }
            };
            let mut caller = std::mem::replace(&mut server, next);
            match connected {
                Ok(()) => {
                    if calls_to.send(()).is_err() {
                        return;
                    }
                    let _ = caller.write_all(&[ANSWER]).await;
                }
                Err(e) => tracing::warn!("a second start did not get through: {e}"),
            }
        }
    };
    let listening = std::thread::Builder::new()
        .name("ww-instance".into())
        .spawn(move || {
            runtime.block_on(async {
                tokio::select! {
                    () = serve => {}
                    _ = stop => {}
                }
            });
        })
        .map_err(InstanceError::Thread)?;
    Ok(Instance {
        calls,
        stop: Some(stop_to),
        listening: Some(listening),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn one_folder_is_one_name_however_it_is_written() {
        assert_eq!(
            pipe_name(Path::new(
                r"C:\Users\Panda\AppData\Roaming\whisperia\config"
            )),
            pipe_name(Path::new(
                "c:/users/panda/appdata/roaming/whisperia/config/"
            ))
        );
        assert_ne!(
            pipe_name(Path::new(
                r"C:\Users\Panda\AppData\Roaming\whisperia\config"
            )),
            pipe_name(Path::new(
                r"C:\Users\Penguin\AppData\Roaming\whisperia\config"
            ))
        );
        assert!(pipe_name(Path::new("anything")).starts_with(r"\\.\pipe\whisperia-"));
    }

    #[cfg(windows)]
    #[test]
    fn a_second_start_calls_the_first_and_goes() {
        let dir = std::env::temp_dir().join(format!("ww-instance-{}", std::process::id()));
        let waker = Waker::default();
        let Ok(Claim::First(first)) = claim(&dir, &waker) else {
            panic!("the first start does not get the settings");
        };
        assert!(!first.called());
        assert!(matches!(claim(&dir, &waker), Ok(Claim::Elsewhere)));
        assert!(matches!(claim(&dir, &waker), Ok(Claim::Elsewhere)));
        assert!(first.called());
        assert!(!first.called());

        drop(first);
        assert!(
            matches!(claim(&dir, &waker), Ok(Claim::First(_))),
            "the settings are free as soon as the first copy is gone"
        );
    }
}
