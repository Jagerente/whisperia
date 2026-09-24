use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use tokio::{net::UdpSocket, task::JoinHandle};

#[derive(Default)]
struct Rules {
    lose_out: AtomicUsize,
    lose_in: AtomicUsize,
    cut: AtomicBool,
}

impl Rules {
    fn passes(&self, every: &AtomicUsize, count: usize) -> bool {
        let nth = every.load(Ordering::Relaxed);
        !self.cut.load(Ordering::Relaxed) && (nth == 0 || count % nth != 0)
    }
}

pub struct Relay {
    addr: SocketAddr,
    rules: Arc<Rules>,
    tasks: [JoinHandle<()>; 2],
}

impl Relay {
    pub fn to(server: SocketAddr) -> Self {
        let outside = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let inside = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        inside.connect(server).unwrap();
        let addr = outside.local_addr().unwrap();
        for socket in [&outside, &inside] {
            socket.set_nonblocking(true).unwrap();
        }
        let outside = Arc::new(UdpSocket::from_std(outside).unwrap());
        let inside = Arc::new(UdpSocket::from_std(inside).unwrap());
        let client = Arc::new(Mutex::new(None));
        let rules = Arc::new(Rules::default());

        let (from, to, seen, rule) = (
            outside.clone(),
            inside.clone(),
            client.clone(),
            rules.clone(),
        );
        let out = tokio::spawn(async move {
            let mut buffer = vec![0u8; 65_536];
            let mut count = 0usize;
            while let Ok((length, sender)) = from.recv_from(&mut buffer).await {
                *seen.lock().unwrap_or_else(|p| p.into_inner()) = Some(sender);
                count += 1;
                if rule.passes(&rule.lose_out, count) {
                    let _ = to.send(&buffer[..length]).await;
                }
            }
        });
        let rule = rules.clone();
        let back = tokio::spawn(async move {
            let mut buffer = vec![0u8; 65_536];
            let mut count = 0usize;
            while let Ok(length) = inside.recv(&mut buffer).await {
                count += 1;
                let known = *client.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(client) = known.filter(|_| rule.passes(&rule.lose_in, count)) {
                    let _ = outside.send_to(&buffer[..length], client).await;
                }
            }
        });
        Self {
            addr,
            rules,
            tasks: [out, back],
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn lose_every_nth_on_the_way_out(&self, nth: usize) {
        self.rules.lose_out.store(nth, Ordering::Relaxed);
    }

    pub fn lose_every_nth_on_the_way_in(&self, nth: usize) {
        self.rules.lose_in.store(nth, Ordering::Relaxed);
    }

    pub fn cut(&self) {
        self.rules.cut.store(true, Ordering::Relaxed);
    }

    pub fn restore(&self) {
        self.rules.cut.store(false, Ordering::Relaxed);
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
