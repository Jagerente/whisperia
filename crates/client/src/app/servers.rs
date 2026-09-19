use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use whisperia_proto::{control::Name, peek::Glance, PROTOCOL_VERSION};

use crate::{
    address::ServerAddress, config::RecentServer, discovery::DiscoveredServer, net::PeekError,
};

pub const LOOK_AGAIN: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occupancy {
    pub rooms: usize,
    pub people: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Checking,
    Up(Option<Occupancy>),
    OtherVersion(Option<u16>),
    Silent,
}

impl Status {
    pub fn of_found(server: &DiscoveredServer) -> Self {
        match server.protocol {
            Some(PROTOCOL_VERSION) => Self::Up(
                server
                    .rooms
                    .zip(server.people)
                    .map(|(rooms, people)| Occupancy { rooms, people }),
            ),
            theirs => Self::OtherVersion(theirs),
        }
    }

    pub fn of_peek(outcome: &Result<Glance, PeekError>) -> Self {
        match outcome {
            Ok(glance) if glance.protocol == PROTOCOL_VERSION => Self::Up(Some(Occupancy {
                rooms: glance.rooms as usize,
                people: glance.people as usize,
            })),
            Ok(glance) => Self::OtherVersion(Some(glance.protocol)),
            Err(PeekError::Stranger(_)) => Self::OtherVersion(None),
            Err(PeekError::Silent(_)) => Self::Silent,
        }
    }

    pub fn or_announced(self, announced: Self) -> Self {
        match (self, announced) {
            (Self::Checking, announced) => announced,
            (Self::OtherVersion(None), theirs @ Self::OtherVersion(Some(_))) => theirs,
            (heard, _) => heard,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: Name,
    pub found: Option<DiscoveredServer>,
    pub recent: Option<RecentServer>,
    pub status: Status,
}

impl Entry {
    pub fn address(&self) -> String {
        match (&self.recent, self.peek_address()) {
            (Some(recent), _) => recent.address.to_string(),
            (None, Some(found)) => found.host().to_owned(),
            (None, None) => String::new(),
        }
    }

    pub fn peek_address(&self) -> Option<ServerAddress> {
        match (&self.found, &self.recent) {
            (Some(found), _) => found.addrs.first().map(|addr| ServerAddress::from(*addr)),
            (None, Some(recent)) => Some(recent.address.clone()),
            (None, None) => None,
        }
    }

    pub fn gone(&self) -> bool {
        self.found.is_some() && self.recent.is_none() && self.status == Status::Silent
    }
}

pub fn gather(
    found: Vec<DiscoveredServer>,
    recent: &[RecentServer],
    lookout: &Lookout,
) -> Vec<Entry> {
    let mut left: Vec<&RecentServer> = recent.iter().collect();
    let mut entries: Vec<Entry> = found
        .into_iter()
        .map(|server| {
            let known = server
                .id
                .and_then(|id| left.iter().position(|recent| recent.id == Some(id)))
                .map(|at| left.remove(at).clone());
            let status = server
                .addrs
                .first()
                .map_or(Status::Checking, |addr| {
                    lookout.status(&ServerAddress::from(*addr))
                })
                .or_announced(Status::of_found(&server));
            Entry {
                name: server.name.clone(),
                found: Some(server),
                recent: known,
                status,
            }
        })
        .collect();
    entries.extend(left.into_iter().map(|recent| Entry {
        name: recent.name.clone(),
        found: None,
        status: lookout.status(&recent.address),
        recent: Some(recent.clone()),
    }));
    entries
}

pub fn watched(entries: &[Entry]) -> impl Iterator<Item = ServerAddress> + '_ {
    entries.iter().filter_map(Entry::peek_address)
}

struct Sight {
    status: Status,
    due: Instant,
    asking: bool,
}

#[derive(Default)]
pub struct Lookout {
    sights: HashMap<ServerAddress, Sight>,
}

impl Lookout {
    pub fn due_now(
        &mut self,
        now: Instant,
        addresses: impl Iterator<Item = ServerAddress>,
    ) -> Vec<ServerAddress> {
        let wanted: Vec<ServerAddress> = addresses.collect();
        self.sights.retain(|address, _| wanted.contains(address));
        let mut ask = Vec::new();
        for address in wanted {
            let sight = self.sights.entry(address.clone()).or_insert(Sight {
                status: Status::Checking,
                due: now,
                asking: false,
            });
            if !sight.asking && now >= sight.due {
                sight.asking = true;
                ask.push(address);
            }
        }
        ask
    }

    pub fn heard(&mut self, now: Instant, address: &ServerAddress, status: Status) {
        if let Some(sight) = self.sights.get_mut(address) {
            sight.status = status;
            sight.asking = false;
            sight.due = now + LOOK_AGAIN;
        }
    }

    pub fn status(&self, address: &ServerAddress) -> Status {
        self.sights
            .get(address)
            .map_or(Status::Checking, |sight| sight.status)
    }

    pub fn next_due(&self) -> Option<Instant> {
        self.sights
            .values()
            .filter(|sight| !sight.asking)
            .map(|sight| sight.due)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::Fingerprint;

    fn found(name: &str, id: u8, addr: &str, protocol: Option<u16>) -> DiscoveredServer {
        DiscoveredServer {
            key: format!("{name}._whisperia._udp.local."),
            name: Name::new(name).unwrap(),
            id: Some(Fingerprint::from([id; 32])),
            addrs: vec![addr.parse().unwrap()],
            protocol,
            rooms: Some(1),
            people: Some(3),
        }
    }

    fn recent(name: &str, address: &str, id: Option<u8>) -> RecentServer {
        RecentServer {
            name: Name::new(name).unwrap(),
            address: address.parse().unwrap(),
            id: id.map(|byte| Fingerprint::from([byte; 32])),
        }
    }

    fn at(address: &str) -> ServerAddress {
        address.parse().unwrap()
    }

    #[test]
    fn a_server_found_and_connected_before_is_one_line() {
        let ours = Some(PROTOCOL_VERSION);
        let entries = gather(
            vec![
                found("DESKTOP-AV83", 1, "192.168.1.52:47820", ours),
                found("DESKTOP-R8ZE", 2, "192.168.1.60:47820", ours),
            ],
            &[
                recent("DESKTOP-AV83", "192.168.1.52", Some(1)),
                recent("DESKTOP-R8ZE", "desktop-r8ze", Some(2)),
                recent("DESKTOP-6TNC", "10.0.0.5:5000", Some(3)),
            ],
            &Lookout::default(),
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["DESKTOP-AV83", "DESKTOP-R8ZE", "DESKTOP-6TNC"]);
        assert!(entries[0].recent.is_some() && entries[1].recent.is_some());
        assert_eq!(entries[1].address(), "desktop-r8ze");
        assert_eq!(
            entries[0].status,
            Status::Up(Some(Occupancy {
                rooms: 1,
                people: 3
            }))
        );
        assert_eq!(entries[2].status, Status::Checking);
    }

    #[test]
    fn a_server_at_a_new_address_is_still_the_one_remembered() {
        let entries = gather(
            vec![found(
                "DESKTOP-AV83",
                1,
                "10.0.0.7:47820",
                Some(PROTOCOL_VERSION),
            )],
            &[recent("DESKTOP-AV83", "192.168.1.52", Some(1))],
            &Lookout::default(),
        );
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn two_servers_with_the_same_name_stay_two_lines() {
        let entries = gather(
            vec![found(
                "DESKTOP-AV83",
                1,
                "10.0.0.7:47820",
                Some(PROTOCOL_VERSION),
            )],
            &[recent("DESKTOP-AV83", "10.0.0.7", Some(9))],
            &Lookout::default(),
        );
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn a_found_server_that_stops_answering_is_offline_and_then_gone() {
        let ours = Some(PROTOCOL_VERSION);
        let away = || found("DESKTOP-AV83", 1, "192.168.1.52:47820", ours);
        let mine = [recent("DESKTOP-AV83", "192.168.1.52", Some(1))];
        let t0 = Instant::now();
        let mut lookout = Lookout::default();

        let entries = gather(vec![away()], &[], &lookout);
        assert!(
            matches!(entries[0].status, Status::Up(Some(_))),
            "what it announced stands until a peek says otherwise"
        );
        assert!(!entries[0].gone());
        assert_eq!(
            lookout.due_now(t0, watched(&entries)),
            [at("192.168.1.52")],
            "a server on the network is asked as well, not only a remembered one"
        );
        lookout.heard(t0, &at("192.168.1.52"), Status::Silent);

        let entries = gather(vec![away()], &[], &lookout);
        assert_eq!(entries[0].status, Status::Silent);
        assert!(entries[0].gone());

        let remembered = gather(vec![away()], &mine, &lookout);
        assert_eq!(remembered[0].status, Status::Silent);
        assert!(
            !remembered[0].gone(),
            "a server connected to before stays on the list, offline"
        );
    }

    #[test]
    fn a_server_gone_from_the_list_is_still_asked_and_comes_back() {
        let ours = Some(PROTOCOL_VERSION);
        let away = || found("DESKTOP-AV83", 1, "192.168.1.52:47820", ours);
        let t0 = Instant::now();
        let mut lookout = Lookout::default();
        let address = at("192.168.1.52");

        lookout.due_now(t0, watched(&gather(vec![away()], &[], &lookout)));
        lookout.heard(t0, &address, Status::Silent);
        assert!(gather(vec![away()], &[], &lookout)[0].gone());

        let later = t0 + LOOK_AGAIN;
        assert_eq!(
            lookout.due_now(later, watched(&gather(vec![away()], &[], &lookout))),
            std::slice::from_ref(&address),
            "what is hidden is still watched, so it does not flicker back"
        );
        lookout.heard(later, &address, Status::Up(None));
        let entries = gather(vec![away()], &[], &lookout);
        assert_eq!(entries[0].status, Status::Up(None));
        assert!(!entries[0].gone());
    }

    #[test]
    fn a_server_found_with_another_version_says_which() {
        let newer = Some(PROTOCOL_VERSION + 1);
        let old = found("DESKTOP-P4LW", 1, "192.168.1.7:47820", newer);
        assert_eq!(Status::of_found(&old), Status::OtherVersion(newer));
        let unsaid = found("DESKTOP-HB21", 2, "192.168.1.8:47820", None);
        assert_eq!(Status::of_found(&unsaid), Status::OtherVersion(None));
        let mut quiet = found(
            "DESKTOP-3MRX",
            3,
            "192.168.1.9:47820",
            Some(PROTOCOL_VERSION),
        );
        quiet.people = None;
        assert_eq!(Status::of_found(&quiet), Status::Up(None));
    }

    #[test]
    fn a_peek_tells_up_from_another_version_from_nobody() {
        let glance = |protocol| Glance {
            protocol,
            name: "DESKTOP-AV83".into(),
            rooms: 2,
            people: 5,
        };
        assert_eq!(
            Status::of_peek(&Ok(glance(PROTOCOL_VERSION))),
            Status::Up(Some(Occupancy {
                rooms: 2,
                people: 5
            }))
        );
        assert_eq!(
            Status::of_peek(&Ok(glance(PROTOCOL_VERSION + 3))),
            Status::OtherVersion(Some(PROTOCOL_VERSION + 3))
        );
        assert_eq!(
            Status::of_peek(&Err(PeekError::Stranger("no application protocol".into()))),
            Status::OtherVersion(None)
        );
        assert_eq!(
            Status::of_peek(&Err(PeekError::Silent("no answer".into()))),
            Status::Silent
        );
    }

    #[test]
    fn a_peek_says_more_than_an_announcement_but_never_less() {
        let theirs = Status::OtherVersion(Some(PROTOCOL_VERSION + 1));
        let busy = Status::Up(Some(Occupancy {
            rooms: 2,
            people: 5,
        }));
        assert_eq!(Status::Checking.or_announced(theirs), theirs);
        assert_eq!(Status::OtherVersion(None).or_announced(theirs), theirs);
        assert_eq!(
            Status::OtherVersion(None).or_announced(Status::Up(None)),
            Status::OtherVersion(None)
        );
        assert_eq!(busy.or_announced(Status::Up(None)), busy);
        assert_eq!(Status::Silent.or_announced(busy), Status::Silent);
    }

    #[test]
    fn an_address_is_asked_once_at_a_time_and_again_after_a_while() {
        let t0 = Instant::now();
        let mut lookout = Lookout::default();
        let both = || [at("192.168.1.5"), at("laptop:5000")].into_iter();
        assert_eq!(lookout.due_now(t0, both()).len(), 2);
        assert_eq!(lookout.status(&at("laptop:5000")), Status::Checking);
        assert!(lookout.due_now(t0 + LOOK_AGAIN * 2, both()).is_empty());

        lookout.heard(t0, &at("laptop:5000"), Status::Silent);
        assert_eq!(lookout.status(&at("laptop:5000")), Status::Silent);
        assert!(lookout.due_now(t0 + LOOK_AGAIN / 2, both()).is_empty());
        assert_eq!(
            lookout.due_now(t0 + LOOK_AGAIN, both()),
            [at("laptop:5000")]
        );
        assert_eq!(
            lookout.status(&at("laptop:5000")),
            Status::Silent,
            "what was last heard stays until the next answer"
        );
    }

    #[test]
    fn the_next_look_is_due_when_the_first_answered_address_is() {
        let t0 = Instant::now();
        let mut lookout = Lookout::default();
        assert_eq!(lookout.next_due(), None);
        let both = || [at("192.168.1.5"), at("laptop:5000")].into_iter();
        lookout.due_now(t0, both());
        assert_eq!(lookout.next_due(), None, "both are being asked");
        lookout.heard(t0, &at("laptop:5000"), Status::Silent);
        assert_eq!(lookout.next_due(), Some(t0 + LOOK_AGAIN));
        lookout.heard(t0 + LOOK_AGAIN / 2, &at("192.168.1.5"), Status::Silent);
        assert_eq!(lookout.next_due(), Some(t0 + LOOK_AGAIN));
    }

    #[test]
    fn an_address_no_longer_listed_is_forgotten() {
        let t0 = Instant::now();
        let mut lookout = Lookout::default();
        lookout.due_now(t0, [at("laptop:5000")].into_iter());
        lookout.heard(t0, &at("laptop:5000"), Status::Silent);
        lookout.due_now(t0, [at("192.168.1.5")].into_iter());
        assert_eq!(lookout.status(&at("laptop:5000")), Status::Checking);
        lookout.heard(t0, &at("laptop:5000"), Status::Silent);
        assert_eq!(lookout.status(&at("laptop:5000")), Status::Checking);
    }
}
