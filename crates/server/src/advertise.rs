use std::{
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use mdns_sd::{ServiceDaemon, ServiceInfo};
use whisperia_proto::{
    control::Name, MDNS_SERVICE_TYPE, MDNS_TXT_ADDRS, MDNS_TXT_ID, MDNS_TXT_NAME, MDNS_TXT_PEERS,
    MDNS_TXT_PROTOCOL, MDNS_TXT_ROOMS, PROTOCOL_VERSION,
};

use crate::hub::Occupancy;

const MIN_GAP: Duration = Duration::from_millis(1100);

enum Msg {
    Occupancy(Occupancy),
    Stop,
}

pub struct Advertiser {
    tx: Sender<Msg>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub struct Announcer(Sender<Msg>);

impl Announcer {
    pub fn announce(&self, occupancy: Occupancy) {
        let _ = self.0.send(Msg::Occupancy(occupancy));
    }
}

struct Template {
    server_name: String,
    id: String,
    instance: String,
    host: String,
    port: u16,
    listed: Vec<String>,
}

impl Template {
    fn service(&self, occupancy: Occupancy, probe: bool) -> Result<ServiceInfo, mdns_sd::Error> {
        let properties = [
            (MDNS_TXT_NAME, self.server_name.clone()),
            (MDNS_TXT_ID, self.id.clone()),
            (MDNS_TXT_PROTOCOL, PROTOCOL_VERSION.to_string()),
            (MDNS_TXT_ADDRS, self.listed.join(",")),
            (MDNS_TXT_ROOMS, occupancy.rooms.to_string()),
            (MDNS_TXT_PEERS, occupancy.people.to_string()),
        ];
        let mut service = ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            &self.instance,
            &format!("{}.local.", self.host),
            "",
            self.port,
            &properties[..],
        )?
        .enable_addr_auto();
        service.set_requires_probe(probe);
        Ok(service)
    }
}

impl Advertiser {
    pub fn start(
        server_name: &Name,
        fingerprint: &[u8; 32],
        port: u16,
        addrs: &[std::net::Ipv4Addr],
    ) -> Result<Self, mdns_sd::Error> {
        let listed: Vec<String> = match addrs {
            [] => routable_ipv4(),
            given => given.iter().map(ToString::to_string).collect(),
        };
        let host = machine_name();
        let template = Template {
            server_name: server_name.to_string(),
            id: fingerprint.iter().map(|b| format!("{b:02x}")).collect(),
            instance: format!("{server_name} [{host}:{port}]"),
            host,
            port,
            listed,
        };
        let daemon = ServiceDaemon::new()?;
        let first = template.service(Occupancy::default(), true)?;
        let fullname = first.get_fullname().to_owned();
        daemon.register(first)?;

        let (tx, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("ww-advertise".into())
            .spawn(move || {
                let mut pending = None;
                let mut last_sent = Instant::now();
                loop {
                    let received = match pending {
                        Some(_) => rx.recv_timeout(MIN_GAP.saturating_sub(last_sent.elapsed())),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match received {
                        Ok(Msg::Occupancy(occupancy)) => pending = Some(occupancy),
                        Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    if let Some(occupancy) = pending {
                        if last_sent.elapsed() >= MIN_GAP {
                            let announced = template
                                .service(occupancy, false)
                                .and_then(|service| daemon.register(service));
                            if let Err(e) = announced {
                                tracing::debug!("mDNS re-announcement failed: {e}");
                            }
                            last_sent = Instant::now();
                            pending = None;
                        }
                    }
                }
                if let Ok(done) = daemon.unregister(&fullname) {
                    let _ = done.recv_timeout(Duration::from_millis(500));
                }
                let _ = daemon.shutdown();
            })
            .map_err(|e| mdns_sd::Error::Msg(format!("cannot start announcement thread: {e}")))?;

        Ok(Self {
            tx,
            worker: Some(worker),
        })
    }

    pub fn announcer(&self) -> Announcer {
        Announcer(self.tx.clone())
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn routable_ipv4() -> Vec<String> {
    let mut found: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|interface| match interface.ip() {
            std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() => {
                Some(ip.to_string())
            }
            _ => None,
        })
        .collect();
    found.sort();
    found.dedup();
    found.truncate(12);
    found
}

pub(crate) fn machine_name() -> String {
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let label: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let label = label.trim_matches('-');
    if label.is_empty() {
        "whisperia-host".to_owned()
    } else {
        label.to_owned()
    }
}
