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
    #[error("cannot learn how long Windows waits on a program that does not respond: {0}")]
    Patience(std::io::Error),
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
    use tokio::net::windows::named_pipe::ServerOptions;
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

    use crate::pipes::{self, Patience};

    let name = pipe_name(settings);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(InstanceError::Runtime)?;
    loop {
        let first = {
            let _inside = runtime.enter();
            ServerOptions::new().first_pipe_instance(true).create(&name)
        };
        match first {
            Ok(server) => return listen(runtime, name, server, waker).map(Claim::First),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
            Err(e) => return Err(InstanceError::Create(e)),
        }
        let patience = Patience::before_not_responding().map_err(InstanceError::Patience)?;
        let freed = match runtime.block_on(tell(&name, patience)) {
            Ok(()) => return Ok(Claim::Elsewhere),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                pipes::wait_for_a_free_end(&name, patience)
            }
            Err(e) => Err(e),
        };
        match freed {
            Ok(()) => {}
            Err(e) if went_away(&e) => {}
            Err(e) => return Err(InstanceError::Silent(e)),
        }
    }
}

#[cfg(windows)]
fn went_away(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;

    matches!(
        e.kind(),
        ErrorKind::NotFound | ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof
    )
}

#[cfg(windows)]
const ANSWER: u8 = b'!';

#[cfg(windows)]
async fn tell(name: &str, patience: crate::pipes::Patience) -> std::io::Result<()> {
    use tokio::{io::AsyncReadExt, net::windows::named_pipe::ClientOptions};

    let mut pipe = ClientOptions::new().open(name)?;
    let mut answer = [0u8; 1];
    tokio::time::timeout(patience.duration(), pipe.read_exact(&mut answer))
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
    use tokio::net::windows::named_pipe::ServerOptions;

    let spare = {
        let _inside = runtime.enter();
        ServerOptions::new().create(&name)
    }
    .map_err(InstanceError::Create)?;
    let (calls_to, calls) = crate::wake::channel(waker);
    let (stop_to, stop) = tokio::sync::oneshot::channel::<()>();
    let serve = async move {
        tokio::join!(
            answer_calls(server, &name, &calls_to),
            answer_calls(spare, &name, &calls_to),
        );
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

#[cfg(windows)]
async fn answer_calls(
    mut server: tokio::net::windows::named_pipe::NamedPipeServer,
    name: &str,
    calls_to: &crate::wake::WakingSender<()>,
) {
    use tokio::{io::AsyncWriteExt, net::windows::named_pipe::ServerOptions};

    loop {
        let connected = server.connect().await;
        let next = match ServerOptions::new().create(name) {
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

    #[cfg(windows)]
    fn settings_of(test: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ww-instance-{test}-{}", std::process::id()))
    }

    #[cfg(windows)]
    fn pipe_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn two_starts_at_once_both_reach_the_first_copy() {
        use tokio::{io::AsyncReadExt, net::windows::named_pipe::ClientOptions};

        let dir = settings_of("together");
        let Ok(Claim::First(first)) = claim(&dir, &Waker::default()) else {
            panic!("the first start does not get the settings");
        };
        let name = pipe_name(&dir);
        pipe_runtime().block_on(async {
            let mut one = ClientOptions::new().open(&name).unwrap();
            let mut two = ClientOptions::new()
                .open(&name)
                .expect("a free end is ready while the first caller is served");
            let mut answers = [0u8; 2];
            one.read_exact(&mut answers[..1]).await.unwrap();
            two.read_exact(&mut answers[1..]).await.unwrap();
            assert_eq!(answers, [ANSWER; 2]);
        });
        assert!(first.called());
    }

    #[cfg(windows)]
    #[test]
    fn a_copy_that_takes_the_call_but_never_answers_is_given_up_on() {
        use tokio::net::windows::named_pipe::ServerOptions;

        let dir = settings_of("deaf");
        let runtime = pipe_runtime();
        let _deaf = {
            let _inside = runtime.enter();
            ServerOptions::new()
                .first_pipe_instance(true)
                .create(pipe_name(&dir))
                .unwrap()
        };
        assert!(matches!(
            claim(&dir, &Waker::default()),
            Err(InstanceError::Silent(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn a_copy_that_never_frees_an_end_is_given_up_on() {
        use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

        let dir = settings_of("taken");
        let runtime = pipe_runtime();
        let (_stuck, _taken) = {
            let _inside = runtime.enter();
            let stuck = ServerOptions::new()
                .first_pipe_instance(true)
                .create(pipe_name(&dir))
                .unwrap();
            (stuck, ClientOptions::new().open(pipe_name(&dir)).unwrap())
        };
        assert!(matches!(
            claim(&dir, &Waker::default()),
            Err(InstanceError::Silent(_))
        ));
    }
}
