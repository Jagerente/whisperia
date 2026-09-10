use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
};

use mdns_sd::{ServiceDaemon, ServiceEvent};
use whisperia_proto::{
    control::Name, MDNS_SERVICE_TYPE, MDNS_TXT_ADDRS, MDNS_TXT_ID, MDNS_TXT_NAME, MDNS_TXT_PEERS,
    MDNS_TXT_PROTOCOL, MDNS_TXT_ROOMS,
};

use crate::trust::Fingerprint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredServer {
    pub key: String,
    pub name: Name,
    pub id: Option<Fingerprint>,
    pub addrs: Vec<SocketAddr>,
    pub protocol: Option<u16>,
    pub rooms: Option<usize>,
    pub people: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Search {
    Lan,
    #[cfg(feature = "testing")]
    Nowhere,
}

pub struct Discovery {
    daemon: ServiceDaemon,
    found: Arc<Mutex<Vec<DiscoveredServer>>>,
}

impl Discovery {
    pub fn start(changed: impl Fn() + Send + 'static) -> Result<Self, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        let events = daemon.browse(MDNS_SERVICE_TYPE)?;
        let found = Arc::new(Mutex::new(Vec::new()));

        let list = found.clone();
        std::thread::Builder::new()
            .name("ww-discovery".into())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    let mut list = list.lock().unwrap_or_else(|p| p.into_inner());
                    match event {
                        ServiceEvent::ServiceResolved(service) => {
                            if let Some(server) = describe(&service) {
                                take_in(&mut list, server);
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            list.retain(|s| s.key != fullname)
                        }
                        _ => continue,
                    }
                    drop(list);
                    changed();
                }
            })
            .map_err(|e| mdns_sd::Error::Msg(format!("cannot start discovery thread: {e}")))?;

        Ok(Self { daemon, found })
    }

    pub fn servers(&self) -> Vec<DiscoveredServer> {
        self.found.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

fn take_in(list: &mut Vec<DiscoveredServer>, server: DiscoveredServer) {
    list.retain(|s| s.key != server.key && (server.id.is_none() || s.id != server.id));
    list.push(server);
    list.sort_by(|a, b| a.name.cmp(&b.name).then(a.key.cmp(&b.key)));
}

fn describe(service: &mdns_sd::ResolvedService) -> Option<DiscoveredServer> {
    let txt = |key| service.txt_properties.get_property_val_str(key);
    let resolved = service.addresses.iter().map(|a| a.to_ip_addr());
    let ips = candidate_addresses(resolved, txt(MDNS_TXT_ADDRS));
    if ips.is_empty() {
        return None;
    }
    let host = service.host.trim_end_matches('.');
    let name = [txt(MDNS_TXT_NAME), Some(host), Some("server")]
        .into_iter()
        .flatten()
        .find_map(|raw| Name::new(raw).ok())?;

    Some(DiscoveredServer {
        key: service.fullname.clone(),
        name,
        id: txt(MDNS_TXT_ID).and_then(|hex| hex.parse().ok()),
        addrs: ips
            .into_iter()
            .map(|ip| SocketAddr::new(ip, service.port))
            .collect(),
        protocol: txt(MDNS_TXT_PROTOCOL).and_then(|n| n.parse().ok()),
        rooms: txt(MDNS_TXT_ROOMS).and_then(|n| n.parse().ok()),
        people: txt(MDNS_TXT_PEERS).and_then(|n| n.parse().ok()),
    })
}

fn candidate_addresses(
    resolved: impl Iterator<Item = IpAddr>,
    listed: Option<&str>,
) -> Vec<IpAddr> {
    let listed = listed
        .unwrap_or_default()
        .split(',')
        .take(16)
        .filter_map(|s| s.trim().parse::<std::net::Ipv4Addr>().ok())
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
        .map(IpAddr::V4);
    let mut ips: Vec<IpAddr> = resolved.filter(IpAddr::is_ipv4).chain(listed).collect();
    ips.sort_by_key(|ip| (address_rank(ip), *ip));
    ips.dedup();
    ips
}

fn address_rank(ip: &IpAddr) -> u8 {
    match ip {
        IpAddr::V4(v4) if v4.is_loopback() => 9,
        IpAddr::V4(v4) if v4.is_link_local() => 6,
        IpAddr::V4(v4) if v4.octets()[0] == 192 && v4.octets()[1] == 168 => 0,
        IpAddr::V4(v4) if v4.octets()[0] == 10 => 1,
        IpAddr::V4(v4) if v4.is_private() => 2,
        IpAddr::V4(_) => 3,

        IpAddr::V6(_) => 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(key: &str, id: Option<u8>, addr: &str) -> DiscoveredServer {
        DiscoveredServer {
            key: key.into(),
            name: Name::new(key).unwrap(),
            id: id.map(|byte| Fingerprint::from([byte; 32])),
            addrs: vec![addr.parse().unwrap()],
            protocol: None,
            rooms: None,
            people: None,
        }
    }

    #[test]
    fn a_server_announced_again_replaces_what_it_said_before() {
        let mut list = Vec::new();
        take_in(&mut list, found("A", Some(1), "192.168.1.5:47820"));
        take_in(&mut list, found("A", Some(1), "192.168.1.6:47820"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].addrs[0].to_string(), "192.168.1.6:47820");
    }

    #[test]
    fn one_server_announced_under_two_names_is_one_line() {
        let mut list = Vec::new();
        take_in(&mut list, found("A", Some(1), "192.168.1.5:47820"));
        take_in(&mut list, found("B", Some(1), "10.0.0.5:47820"));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn two_servers_sharing_an_address_are_both_listed() {
        let mut list = Vec::new();
        take_in(&mut list, found("A", Some(1), "172.17.0.1:47820"));
        take_in(&mut list, found("B", Some(2), "172.17.0.1:47820"));
        take_in(&mut list, found("C", None, "172.17.0.1:47820"));
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn server_listed_addresses_are_merged_and_garbage_is_ignored() {
        let resolved = ["172.25.80.1", "fe80::1"]
            .iter()
            .map(|s| s.parse().unwrap());
        let ips = candidate_addresses(
            resolved,
            Some("192.168.1.52, 172.25.80.1,nonsense,127.0.0.1,::1"),
        );
        let order: Vec<String> = ips.iter().map(ToString::to_string).collect();
        assert_eq!(order, ["192.168.1.52", "172.25.80.1"]);

        let mut ips: Vec<IpAddr> = [
            "172.20.0.1",
            "127.0.0.1",
            "10.0.0.2",
            "192.168.1.5",
            "169.254.3.3",
        ]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
        ips.sort_by_key(address_rank);
        let order: Vec<String> = ips.iter().map(ToString::to_string).collect();
        assert_eq!(
            order,
            [
                "192.168.1.5",
                "10.0.0.2",
                "172.20.0.1",
                "169.254.3.3",
                "127.0.0.1"
            ],
            "home addresses are tried first"
        );
    }
}
