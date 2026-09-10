use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SendError, Sender},
    Arc, OnceLock,
};

type Wakeup = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Inner {
    wakeup: OnceLock<Wakeup>,
    pending: AtomicBool,
}

#[derive(Clone, Default)]
pub struct Waker(Arc<Inner>);

impl Waker {
    pub fn attach(&self, wakeup: impl Fn() + Send + Sync + 'static) {
        let _ = self.0.wakeup.set(Box::new(wakeup));
    }

    pub fn wake(&self) {
        if self.0.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        match self.0.wakeup.get() {
            Some(wakeup) => wakeup(),
            None => self.0.pending.store(false, Ordering::Release),
        }
    }

    pub fn woken(&self) {
        self.0.pending.store(false, Ordering::Release);
    }
}

pub struct WakingSender<T> {
    sender: Sender<T>,
    waker: Waker,
}

impl<T> Clone for WakingSender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            waker: self.waker.clone(),
        }
    }
}

impl<T> WakingSender<T> {
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let sent = self.sender.send(value);
        self.waker.wake();
        sent
    }
}

pub fn channel<T>(waker: &Waker) -> (WakingSender<T>, Receiver<T>) {
    let (sender, receiver) = mpsc::channel();
    (
        WakingSender {
            sender,
            waker: waker.clone(),
        },
        receiver,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[test]
    fn a_wakeup_is_asked_for_once_until_it_has_been_handled() {
        let waker = Waker::default();
        let asked = Arc::new(AtomicUsize::new(0));
        let counted = asked.clone();
        waker.attach(move || {
            counted.fetch_add(1, Ordering::Relaxed);
        });
        waker.wake();
        waker.wake();
        assert_eq!(asked.load(Ordering::Relaxed), 1);
        waker.woken();
        waker.wake();
        assert_eq!(asked.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn what_is_sent_before_anybody_listens_is_kept() {
        let waker = Waker::default();
        let (sender, receiver) = channel(&waker);
        sender.send(7).unwrap();
        assert_eq!(receiver.try_recv(), Ok(7));
    }
}
