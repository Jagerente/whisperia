use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use super::*;

fn name(text: &str) -> Name {
    Name::new(text).unwrap()
}

fn room(text: &str) -> RoomName {
    RoomName::new(text).unwrap()
}

fn password(text: &str) -> Password {
    Password::new(text).unwrap()
}

fn rules(secret: Option<&str>, limit: Option<u8>) -> Option<RoomRules> {
    Some(RoomRules {
        password: secret.map(password),
        limit: limit.map(RoomLimit::new),
    })
}

fn drain(rx: &mut UnboundedReceiver<ServerMsg>) -> Vec<ServerMsg> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn changes(rx: &mut UnboundedReceiver<ServerMsg>) -> Vec<RoomState> {
    drain(rx)
        .into_iter()
        .filter_map(|msg| match msg {
            ServerMsg::RoomChanged(state) => Some(state),
            _ => None,
        })
        .collect()
}

fn someone<C>(hub: &mut Hub<C>, who: &str, conn: C) -> (PeerId, UnboundedReceiver<ServerMsg>) {
    let (tx, rx) = unbounded_channel();
    let id = hub
        .connect(&name(who), Avatar::None, conn, tx, 0)
        .unwrap()
        .id;
    (id, rx)
}

fn enter<C>(hub: &mut Hub<C>, id: PeerId, into: &str) -> Result<Vec<PeerInfo>, JoinError> {
    hub.join(id, &room(into), None, None)
        .map(|(others, _)| others)
}

fn enter_with<C>(
    hub: &mut Hub<C>,
    id: PeerId,
    into: &str,
    secret: Option<&str>,
    new_rules: Option<RoomRules>,
) -> Result<RoomState, JoinError> {
    let secret = secret.map(password);
    hub.join(id, &room(into), secret.as_ref(), new_rules)
        .map(|(_, state)| state)
}

fn summary(into: &str, people: &[&str]) -> RoomSummary {
    RoomSummary {
        name: room(into),
        people: people
            .iter()
            .enumerate()
            .map(|(at, p)| RoomPerson {
                name: name(p),
                avatar: Avatar::None,
                host: at == 0,
            })
            .collect(),
        locked: false,
        limit: None,
    }
}

fn occupancy<C>(hub: &Hub<C>) -> (usize, usize) {
    let Occupancy { rooms, people } = hub.occupancy();
    (rooms, people)
}

fn talkers(hub: &Hub<char>, from: PeerId, kind: AudioKind) -> String {
    hub.audio_targets(from, kind).copied().collect()
}

const MUTED: VoiceState = VoiceState {
    muted: true,
    deafened: false,
};

const DEAF: VoiceState = VoiceState {
    muted: false,
    deafened: true,
};

const SOUND: ShareInfo = ShareInfo {
    sound: true,
    minimized: false,
};

#[test]
fn rooms_appear_with_the_first_person_and_vanish_with_the_last() {
    let mut hub = Hub::new(8);
    let (a, mut rx_a) = someone(&mut hub, "Panda", 'a');
    let (b, mut rx_b) = someone(&mut hub, "Raccoon", 'b');
    assert_eq!(occupancy(&hub), (0, 2));

    assert_eq!(enter(&mut hub, a, "Backroom").unwrap(), []);
    assert_eq!(occupancy(&hub), (1, 2));
    assert_eq!(
        drain(&mut rx_a),
        vec![ServerMsg::Rooms(vec![summary("Backroom", &["Panda"])])]
    );

    enter(&mut hub, b, "Room 83").unwrap();
    assert_eq!(occupancy(&hub), (2, 2));
    hub.leave(b);
    assert_eq!(hub.rooms(), vec![summary("Backroom", &["Panda"])]);
    assert_eq!(hub.disconnect(a), Some(name("Panda")));
    assert_eq!(occupancy(&hub), (0, 1));
    assert_eq!(drain(&mut rx_b).last(), Some(&ServerMsg::Rooms(vec![])));
    assert_eq!(hub.disconnect(a), None);
}

#[test]
fn whoever_enters_is_told_to_the_room_and_the_list_to_everybody() {
    let mut hub = Hub::new(8);
    let (a, mut rx_a) = someone(&mut hub, "Panda", 'a');
    let (b, mut rx_b) = someone(&mut hub, "Raccoon", 'b');
    enter(&mut hub, a, "Backroom").unwrap();
    drain(&mut rx_a);
    drain(&mut rx_b);

    let others = enter(&mut hub, b, "Backroom").unwrap();
    assert_eq!(others.len(), 1);
    assert_eq!(others[0].name, "Panda");
    let told = drain(&mut rx_a);
    assert!(
        matches!(&told[0], ServerMsg::PeerJoined(p) if p.id == b),
        "{told:?}"
    );
    assert_eq!(
        told[1],
        ServerMsg::Rooms(vec![summary("Backroom", &["Panda", "Raccoon"])])
    );
    assert_eq!(drain(&mut rx_b).len(), 1, "only the list, not its own join");

    enter(&mut hub, b, "Room 83").unwrap();
    assert_eq!(drain(&mut rx_a)[0], ServerMsg::PeerLeft(b));
}

#[test]
fn a_taken_name_gets_a_number_and_its_owner_learns_it() {
    let mut hub = Hub::new(8);
    let (tx, _rx) = unbounded_channel();
    hub.connect(&name("Panda"), Avatar::None, 'a', tx.clone(), 0)
        .unwrap();
    let second = hub
        .connect(&name("Panda"), Avatar::None, 'b', tx, 0)
        .unwrap();
    assert_eq!(second.name, "Panda (2)");
}

#[test]
fn a_name_is_given_back_once_whoever_held_it_leaves() {
    let mut hub = Hub::new(8);
    let (ghost, _rx_ghost) = someone(&mut hub, "Panda", 'a');
    let (me, mut rx_me) = someone(&mut hub, "Panda", 'b');
    let (raccoon, mut rx_raccoon) = someone(&mut hub, "Raccoon", 'c');
    enter(&mut hub, me, "General").unwrap();
    enter(&mut hub, raccoon, "General").unwrap();
    drain(&mut rx_me);
    drain(&mut rx_raccoon);

    hub.disconnect(ghost);
    let renamed = ServerMsg::PeerRenamed {
        id: me,
        name: name("Panda"),
    };
    assert_eq!(drain(&mut rx_me)[0], renamed);
    assert_eq!(drain(&mut rx_raccoon)[0], renamed);
    assert_eq!(hub.peers[&me].info.name, "Panda");
}

#[test]
fn a_name_let_go_by_a_rename_is_given_back() {
    let mut hub = Hub::new(8);
    let (first, _rx_first) = someone(&mut hub, "Panda", 'a');
    let (second, mut rx_second) = someone(&mut hub, "Panda", 'b');
    hub.rename(first, &name("Fox"));
    assert_eq!(
        drain(&mut rx_second),
        vec![ServerMsg::PeerRenamed {
            id: second,
            name: name("Panda")
        }]
    );
}

#[test]
fn a_number_chosen_on_purpose_is_kept() {
    let mut hub = Hub::new(8);
    let (first, _rx_first) = someone(&mut hub, "Panda", 'a');
    let (second, mut rx_second) = someone(&mut hub, "Panda (2)", 'b');
    hub.disconnect(first);
    assert!(drain(&mut rx_second).is_empty());
    assert_eq!(hub.peers[&second].info.name, "Panda (2)");
}

#[test]
fn a_long_name_taken_twice_still_fits_with_its_number() {
    let mut hub = Hub::new(8);
    let long = "A".repeat(MAX_NAME_CHARS);
    someone(&mut hub, &long, ());
    let (b, _rx) = someone(&mut hub, &long, ());
    someone(&mut hub, &long, ());
    let names: Vec<&Name> = hub.peers.values().map(|p| &p.info.name).collect();
    assert!(names.iter().all(|n| n.chars().count() <= MAX_NAME_CHARS));
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 3);
    assert!(hub.peers[&b].info.name.ends_with(" (2)"));
}

#[test]
fn a_rename_is_told_to_the_room_and_to_the_one_renamed() {
    let mut hub = Hub::new(8);
    let (a, mut rx_a) = someone(&mut hub, "Panda", 'a');
    let (b, mut rx_b) = someone(&mut hub, "Raccoon", 'b');
    let (_, mut rx_c) = someone(&mut hub, "Fox", 'c');
    enter(&mut hub, a, "one").unwrap();
    enter(&mut hub, b, "one").unwrap();
    for rx in [&mut rx_a, &mut rx_b, &mut rx_c] {
        drain(rx);
    }

    hub.rename(a, &name("Raccoon"));
    let renamed = ServerMsg::PeerRenamed {
        id: a,
        name: name("Raccoon (2)"),
    };
    assert_eq!(drain(&mut rx_a)[0], renamed);
    assert_eq!(drain(&mut rx_b)[0], renamed);
    let outside = drain(&mut rx_c);
    assert!(matches!(&outside[..], [ServerMsg::Rooms(_)]), "{outside:?}");

    hub.rename(a, &name("Raccoon (2)"));
    assert!(drain(&mut rx_a).is_empty(), "the same name is no rename");
}

#[test]
fn a_rename_outside_a_room_is_told_to_the_one_renamed_alone() {
    let mut hub = Hub::new(8);
    let (a, mut rx_a) = someone(&mut hub, "Panda", 'a');
    let (_, mut rx_b) = someone(&mut hub, "Raccoon", 'b');
    hub.rename(a, &name("Fox"));
    assert_eq!(
        drain(&mut rx_a),
        vec![ServerMsg::PeerRenamed {
            id: a,
            name: name("Fox")
        }]
    );
    assert!(drain(&mut rx_b).is_empty());
}

#[test]
fn voice_reaches_the_room_unless_muted_or_deafened() {
    let mut hub = Hub::new(8);
    let (a, _rx_a) = someone(&mut hub, "a", 'a');
    let (b, _rx_b) = someone(&mut hub, "b", 'b');
    let (c, _rx_c) = someone(&mut hub, "c", 'c');
    let (d, _rx_d) = someone(&mut hub, "d", 'd');
    enter(&mut hub, a, "one").unwrap();
    enter(&mut hub, b, "one").unwrap();
    enter(&mut hub, c, "two").unwrap();

    assert_eq!(talkers(&hub, a, AudioKind::Voice), "b");
    assert_eq!(talkers(&hub, c, AudioKind::Voice), "");
    assert_eq!(talkers(&hub, d, AudioKind::Voice), "");
    hub.set_state(b, DEAF);
    assert_eq!(talkers(&hub, a, AudioKind::Voice), "");
    hub.set_state(a, MUTED);
    assert_eq!(talkers(&hub, b, AudioKind::Voice), "a");
    hub.set_state(b, VoiceState::default());
    assert_eq!(talkers(&hub, a, AudioKind::Voice), "");
}

#[test]
fn chat_stays_inside_the_room_and_is_not_echoed() {
    let mut hub = Hub::new(8);
    let (a, _rx_a) = someone(&mut hub, "a", 'a');
    let (b, mut rx_b) = someone(&mut hub, "b", 'b');
    let (c, mut rx_c) = someone(&mut hub, "c", 'c');
    let (d, _rx_d) = someone(&mut hub, "d", 'd');
    enter(&mut hub, a, "one").unwrap();
    enter(&mut hub, b, "one").unwrap();
    enter(&mut hub, c, "two").unwrap();
    drain(&mut rx_b);
    drain(&mut rx_c);

    let said = ChatText::new("Hello World!").unwrap();
    hub.chat(a, said.clone());
    hub.chat(d, ChatText::new("from the list").unwrap());
    assert_eq!(
        drain(&mut rx_b),
        vec![ServerMsg::Chat {
            from: a,
            text: said
        }]
    );
    assert!(
        drain(&mut rx_c).is_empty(),
        "room two hears nothing of room one"
    );
    hub.chat(c, ChatText::new("alone").unwrap());
    assert!(drain(&mut rx_c).is_empty(), "not echoed to the author");
}

#[test]
fn an_animal_travels_with_its_owner() {
    let mut hub = Hub::new(8);
    let (tx_b, mut rx_b) = unbounded_channel();
    let a = hub
        .connect(&name("Panda"), Avatar::Panda, 'a', tx_b.clone(), 0)
        .unwrap()
        .id;
    let b = hub
        .connect(&name("Raccoon"), Avatar::None, 'b', tx_b, 0)
        .unwrap()
        .id;
    enter(&mut hub, a, "Backroom").unwrap();
    assert_eq!(hub.rooms()[0].people[0].avatar, Avatar::Panda);
    assert_eq!(
        enter(&mut hub, b, "Backroom").unwrap()[0].avatar,
        Avatar::Panda
    );
    drain(&mut rx_b);

    hub.set_avatar(a, Avatar::Fox);
    assert_eq!(
        drain(&mut rx_b)[0],
        ServerMsg::PeerAvatar {
            id: a,
            avatar: Avatar::Fox
        }
    );
    assert_eq!(hub.rooms()[0].people[0].avatar, Avatar::Fox);
    hub.set_avatar(a, Avatar::Fox);
    assert!(drain(&mut rx_b).is_empty(), "the same animal is no change");
    hub.set_avatar(999, Avatar::Bee);
}

#[test]
fn a_media_connection_needs_the_token_it_was_given() {
    let mut hub = Hub::new(8);
    let (tx, _rx) = unbounded_channel();
    let b = hub
        .connect(&name("b"), Avatar::None, 'b', tx, 22)
        .unwrap()
        .id;
    assert!(!hub.attach_media(b, 11, 'x'));
    assert!(!hub.attach_media(999, 22, 'x'));
    assert!(hub.attach_media(b, 22, 'B'));
}

struct Stage {
    hub: Hub<char>,
    sharer: PeerId,
    sharer_told: UnboundedReceiver<ServerMsg>,
    watcher: PeerId,
    watcher_told: UnboundedReceiver<ServerMsg>,
    other: PeerId,
    elsewhere: PeerId,
}

fn stage() -> Stage {
    let mut hub = Hub::new(8);
    let mut join = |who: &str, conn: char, token: u64, into: &str| {
        let (tx, rx) = unbounded_channel();
        let id = hub
            .connect(&name(who), Avatar::None, conn, tx, token)
            .unwrap()
            .id;
        enter(&mut hub, id, into).unwrap();
        (id, rx)
    };
    let (sharer, mut sharer_told) = join("a", 'a', 11, "one");
    let (watcher, mut watcher_told) = join("b", 'b', 22, "one");
    let (other, _) = join("c", 'c', 33, "one");
    let (elsewhere, _) = join("d", 'd', 44, "two");
    assert!(hub.attach_media(watcher, 22, 'B'));
    assert!(hub.attach_media(other, 33, 'C'));
    drain(&mut sharer_told);
    drain(&mut watcher_told);
    Stage {
        hub,
        sharer,
        sharer_told,
        watcher,
        watcher_told,
        other,
        elsewhere,
    }
}

#[test]
fn a_share_is_announced_to_the_room() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    assert_eq!(
        drain(&mut s.watcher_told),
        vec![ServerMsg::PeerShare {
            id: s.sharer,
            share: Some(SOUND)
        }]
    );
    let (newcomer, _rx) = someone(&mut s.hub, "e", 'e');
    let roster = enter(&mut s.hub, newcomer, "one").unwrap();
    assert_eq!(
        roster.iter().find(|p| p.id == s.sharer).unwrap().share,
        Some(SOUND),
        "whoever comes later sees it in the roster"
    );
}

#[test]
fn only_a_share_in_the_same_room_can_be_watched() {
    let mut s = stage();
    assert!(!s.hub.watch(s.watcher, s.sharer), "nothing shared yet");
    s.hub.share_start(s.sharer, SOUND);
    assert!(!s.hub.watch(s.elsewhere, s.sharer));
    assert!(!s.hub.watch(s.sharer, s.sharer));
    assert!(s.hub.watch(s.watcher, s.sharer));
}

#[test]
fn the_sharer_learns_who_watches_and_is_asked_for_a_keyframe() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    s.hub.watch(s.watcher, s.sharer);
    assert_eq!(
        drain(&mut s.sharer_told),
        vec![
            ServerMsg::Viewers(vec![s.watcher]),
            ServerMsg::KeyframeWanted
        ]
    );
    s.hub.unwatch(s.watcher, s.sharer);
    assert_eq!(drain(&mut s.sharer_told), vec![ServerMsg::Viewers(vec![])]);
    s.hub.unwatch(s.watcher, s.sharer);
    assert!(drain(&mut s.sharer_told).is_empty());
}

#[test]
fn pictures_and_share_sound_reach_those_who_watch() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    assert!(s.hub.picture_targets(s.sharer).is_empty());
    assert_eq!(talkers(&s.hub, s.sharer, AudioKind::Share), "");
    s.hub.watch(s.watcher, s.sharer);
    assert_eq!(s.hub.picture_targets(s.sharer), vec![(s.watcher, 'B')]);
    assert_eq!(talkers(&s.hub, s.sharer, AudioKind::Share), "b");

    s.hub.set_state(s.sharer, MUTED);
    assert_eq!(talkers(&s.hub, s.sharer, AudioKind::Voice), "");
    assert_eq!(
        talkers(&s.hub, s.sharer, AudioKind::Share),
        "b",
        "a muted voice leaves the share sound"
    );
    s.hub.set_state(s.watcher, DEAF);
    assert_eq!(talkers(&s.hub, s.sharer, AudioKind::Share), "");
}

#[test]
fn a_share_without_sound_is_silent() {
    let mut s = stage();
    s.hub.share_start(
        s.sharer,
        ShareInfo {
            sound: false,
            minimized: false,
        },
    );
    s.hub.watch(s.watcher, s.sharer);
    assert_eq!(talkers(&s.hub, s.sharer, AudioKind::Share), "");
}

#[test]
fn a_minimized_share_is_told_and_keeps_its_watchers() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    s.hub.watch(s.watcher, s.sharer);
    drain(&mut s.watcher_told);
    let minimized = ShareInfo {
        minimized: true,
        ..SOUND
    };
    s.hub.share_start(s.sharer, minimized);
    assert_eq!(
        drain(&mut s.watcher_told),
        vec![ServerMsg::PeerShare {
            id: s.sharer,
            share: Some(minimized)
        }]
    );
    assert!(!s.hub.picture_targets(s.sharer).is_empty());
}

#[test]
fn a_watcher_without_a_media_connection_gets_no_pictures() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    let (newcomer, _rx) = someone(&mut s.hub, "e", 'e');
    enter(&mut s.hub, newcomer, "one").unwrap();
    assert!(s.hub.watch(newcomer, s.sharer));
    assert!(s.hub.picture_targets(s.sharer).is_empty());
}

#[test]
fn a_keyframe_is_asked_for_only_by_a_watcher() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    s.hub.watch(s.watcher, s.sharer);
    drain(&mut s.sharer_told);
    s.hub.need_keyframe(s.other, s.sharer);
    assert!(drain(&mut s.sharer_told).is_empty());
    s.hub.need_keyframe(s.watcher, s.sharer);
    assert_eq!(drain(&mut s.sharer_told), vec![ServerMsg::KeyframeWanted]);
}

#[test]
fn a_media_connection_that_comes_later_asks_for_keyframes_of_what_is_watched() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    s.hub.detach_media(s.watcher, |conn| *conn == 'B');
    s.hub.watch(s.watcher, s.sharer);
    drain(&mut s.sharer_told);
    assert!(s.hub.attach_media(s.watcher, 22, 'B'));
    assert_eq!(drain(&mut s.sharer_told), vec![ServerMsg::KeyframeWanted]);
}

#[test]
fn leaving_the_room_stops_watching() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    s.hub.watch(s.watcher, s.sharer);
    drain(&mut s.sharer_told);
    s.hub.leave(s.watcher);
    assert_eq!(drain(&mut s.sharer_told)[0], ServerMsg::PeerLeft(s.watcher));
    assert!(s.hub.picture_targets(s.sharer).is_empty());
}

#[test]
fn a_stopped_share_is_told_and_watched_by_nobody() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    s.hub.watch(s.watcher, s.sharer);
    drain(&mut s.watcher_told);
    s.hub.share_stop(s.sharer);
    assert_eq!(
        drain(&mut s.watcher_told),
        vec![ServerMsg::PeerShare {
            id: s.sharer,
            share: None
        }]
    );
    assert!(s.hub.picture_targets(s.sharer).is_empty());
}

#[test]
fn a_share_started_again_after_leaving_starts_unwatched() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    s.hub.watch(s.watcher, s.sharer);
    s.hub.leave(s.sharer);
    enter(&mut s.hub, s.sharer, "one").unwrap();
    assert!(!s.hub.watch(s.watcher, s.sharer), "not shared any more");
    s.hub.share_start(s.sharer, SOUND);
    assert!(s.hub.picture_targets(s.sharer).is_empty());
}

#[test]
fn only_the_current_media_connection_is_let_go() {
    let mut s = stage();
    s.hub.share_start(s.sharer, SOUND);
    s.hub.watch(s.other, s.sharer);
    s.hub.detach_media(s.other, |conn| *conn == 'x');
    assert_eq!(s.hub.picture_targets(s.sharer), vec![(s.other, 'C')]);
    s.hub.detach_media(s.other, |conn| *conn == 'C');
    assert!(s.hub.picture_targets(s.sharer).is_empty());
}

#[test]
fn the_server_lets_in_only_so_many() {
    let mut hub = Hub::new(1);
    someone(&mut hub, "a", ());
    let (tx, _rx) = unbounded_channel();
    assert_eq!(
        hub.connect(&name("b"), Avatar::None, (), tx, 0),
        Err(HubFull)
    );
}

#[test]
fn ids_are_never_zero_and_never_taken() {
    let mut hub = Hub::new(4);
    hub.last_id = PeerId::MAX - 1;
    let (first, _rx) = someone(&mut hub, "a", ());
    assert_eq!(first, PeerId::MAX);
    let (second, _rx) = someone(&mut hub, "b", ());
    assert_eq!(second, 1);
    hub.last_id = PeerId::MAX;
    let (third, _rx) = someone(&mut hub, "c", ());
    assert_eq!(third, 2, "1 is taken");
}

#[test]
fn someone_not_connected_enters_no_room() {
    let mut hub: Hub<()> = Hub::new(4);
    assert_eq!(enter(&mut hub, 999, "x"), Err(JoinError::NotConnected));
}

#[test]
fn the_server_holds_only_so_many_rooms() {
    let mut hub = Hub::new(64);
    let ids: Vec<PeerId> = (0..=MAX_ROOMS)
        .map(|i| someone(&mut hub, &i.to_string(), ()).0)
        .collect();
    for (i, id) in ids.iter().enumerate().take(MAX_ROOMS) {
        enter(&mut hub, *id, &format!("room {i}")).unwrap();
    }
    assert_eq!(
        enter(&mut hub, ids[MAX_ROOMS], "one more"),
        Err(JoinError::TooManyRooms)
    );
    assert_eq!(enter(&mut hub, ids[MAX_ROOMS], "room 0").unwrap().len(), 1);
}

#[test]
fn a_change_of_occupancy_is_reported_once() {
    let mut hub = Hub::new(8);
    assert_eq!(hub.occupancy_change(), None);
    let (a, _rx) = someone(&mut hub, "a", ());
    assert_eq!(
        hub.occupancy_change(),
        Some(Occupancy {
            rooms: 0,
            people: 1
        })
    );
    assert_eq!(hub.occupancy_change(), None);
    enter(&mut hub, a, "one").unwrap();
    hub.set_state(a, MUTED);
    assert_eq!(
        hub.occupancy_change(),
        Some(Occupancy {
            rooms: 1,
            people: 1
        })
    );
}

#[test]
fn the_first_one_in_hosts_and_the_crown_passes_to_whoever_came_next() {
    let mut hub = Hub::new(8);
    let (a, _rx_a) = someone(&mut hub, "Panda", 'x');
    let (b, mut rx_b) = someone(&mut hub, "Raccoon", 'x');
    let (c, mut rx_c) = someone(&mut hub, "Fox", 'x');

    let state = enter_with(&mut hub, a, "General", None, None).unwrap();
    assert_eq!(
        state,
        RoomState {
            host: a,
            locked: false,
            limit: None
        }
    );
    assert_eq!(
        enter_with(&mut hub, c, "General", None, None).unwrap().host,
        a
    );
    enter(&mut hub, b, "General").unwrap();
    drain(&mut rx_b);
    drain(&mut rx_c);

    hub.leave(a);
    let told = drain(&mut rx_b);
    assert_eq!(told[0], ServerMsg::PeerLeft(a));
    assert!(
        told.contains(&ServerMsg::RoomChanged(RoomState {
            host: c,
            locked: false,
            limit: None
        })),
        "the one in longest, not the lowest id: {told:?}"
    );
    let hosts: Vec<(String, bool)> = hub.rooms()[0]
        .people
        .iter()
        .map(|p| (p.name.to_string(), p.host))
        .collect();
    assert_eq!(
        hosts,
        [("Raccoon".to_owned(), false), ("Fox".to_owned(), true)]
    );

    drain(&mut rx_c);
    hub.set_room_limit(b, Some(RoomLimit::new(4)));
    hub.make_host(b, b);
    hub.make_host(a, b);
    assert!(
        changes(&mut rx_c).is_empty(),
        "only the host changes the room"
    );
    hub.make_host(c, 999);
    hub.make_host(c, a);
    hub.make_host(c, c);
    assert!(changes(&mut rx_c).is_empty(), "the crown stays in the room");

    hub.make_host(c, b);
    assert_eq!(changes(&mut rx_c)[0].host, b);
    hub.disconnect(b);
    assert_eq!(changes(&mut rx_c)[0].host, c);
    hub.leave(c);
    assert!(hub.rooms().is_empty());
    assert_eq!(
        enter_with(&mut hub, a, "General", None, None).unwrap().host,
        a,
        "a room opened again starts afresh"
    );
}

#[test]
fn only_the_host_kicks_and_only_someone_in_the_same_room() {
    let mut hub = Hub::new(8);
    let (a, mut rx_a) = someone(&mut hub, "Panda", 'x');
    let (b, mut rx_b) = someone(&mut hub, "Fox", 'x');
    let (c, mut rx_c) = someone(&mut hub, "Raccoon", 'x');
    let (d, _rx_d) = someone(&mut hub, "Octopus", 'x');
    enter_with(&mut hub, a, "Backroom", None, rules(Some("owl"), None)).unwrap();
    enter_with(&mut hub, b, "Backroom", Some("owl"), None).unwrap();
    enter_with(&mut hub, c, "Backroom", Some("owl"), None).unwrap();
    enter(&mut hub, d, "General").unwrap();
    hub.share_start(b, SOUND);
    assert!(hub.watch(c, b));
    for rx in [&mut rx_a, &mut rx_b, &mut rx_c] {
        drain(rx);
    }

    assert!(!hub.kick(b, c), "only the host");
    assert!(!hub.kick(a, a), "not the host itself");
    assert!(!hub.kick(a, d), "not someone in another room");
    assert!(!hub.kick(a, 999));
    assert!(drain(&mut rx_c).is_empty());

    assert!(hub.kick(a, b));
    assert_eq!(drain(&mut rx_b)[0], ServerMsg::Kicked);
    let told = drain(&mut rx_c);
    assert_eq!(told[0], ServerMsg::PeerKicked(b), "{told:?}");
    assert!(!told.contains(&ServerMsg::PeerLeft(b)), "{told:?}");
    assert!(hub.picture_targets(b).is_empty(), "nobody watches them now");
    assert_eq!(hub.rooms()[0].name, "Backroom");
    assert_eq!(occupancy(&hub), (2, 4));
    assert!(!hub.kick(a, b), "already out");

    assert!(
        enter_with(&mut hub, b, "Backroom", Some("owl"), None).is_ok(),
        "whoever was kicked may come back at once"
    );
}

#[test]
fn a_locked_room_asks_for_its_password_and_shows_nobody_outside() {
    let mut hub = Hub::new(8);
    let (a, _rx_a) = someone(&mut hub, "Panda", 'x');
    let (b, mut rx_b) = someone(&mut hub, "Mouse", 'x');
    let (c, _rx_c) = someone(&mut hub, "Penguin", 'x');

    let state = enter_with(&mut hub, a, "Backroom", None, rules(Some("owl"), Some(3))).unwrap();
    assert_eq!(
        state,
        RoomState {
            host: a,
            locked: true,
            limit: Some(RoomLimit::new(3))
        }
    );
    assert_eq!(
        hub.rooms(),
        vec![RoomSummary {
            name: room("Backroom"),
            people: Vec::new(),
            locked: true,
            limit: None,
        }]
    );
    assert_eq!(
        enter_with(&mut hub, b, "Backroom", None, None),
        Err(JoinError::Password)
    );
    assert_eq!(
        enter_with(&mut hub, b, "Backroom", Some("OWL"), None),
        Err(JoinError::Password)
    );
    assert_eq!(
        enter_with(&mut hub, b, "Backroom", Some("owl"), rules(None, None)),
        Ok(state)
    );
    drain(&mut rx_b);

    hub.set_room_password(a, None);
    assert!(!changes(&mut rx_b)[0].locked);
    let open = &hub.rooms()[0];
    assert_eq!(
        (open.people.len(), open.limit),
        (2, Some(RoomLimit::new(3)))
    );

    let state = enter_with(&mut hub, c, "Backroom", None, rules(Some("cat"), None)).unwrap();
    assert_eq!(
        (state.host, state.locked, state.limit),
        (a, false, Some(RoomLimit::new(3))),
        "the rules of whoever opens a room that is already open are not used"
    );
}

#[test]
fn a_full_room_lets_nobody_else_in_and_a_lower_limit_pushes_nobody_out() {
    let mut hub = Hub::new(8);
    let (a, _rx_a) = someone(&mut hub, "Panda", 'x');
    let (b, _rx_b) = someone(&mut hub, "Raccoon", 'x');
    let (c, _rx_c) = someone(&mut hub, "Fox", 'x');
    let (d, _rx_d) = someone(&mut hub, "Octopus", 'x');

    enter_with(&mut hub, a, "General", None, rules(None, Some(3))).unwrap();
    enter(&mut hub, b, "General").unwrap();
    enter(&mut hub, c, "General").unwrap();
    assert!(hub.rooms()[0].is_full());
    assert_eq!(enter(&mut hub, d, "General"), Err(JoinError::Full));

    hub.set_room_limit(a, Some(RoomLimit::new(2)));
    assert_eq!(hub.rooms()[0].people.len(), 3);
    hub.leave(b);
    assert_eq!(enter(&mut hub, b, "General"), Err(JoinError::Full));
    hub.set_room_limit(a, None);
    enter(&mut hub, d, "General").unwrap();

    hub.set_room_password(a, Some(password("fox")));
    hub.set_room_limit(a, Some(RoomLimit::new(3)));
    let (e, _rx_e) = someone(&mut hub, "Penguin", 'x');
    assert_eq!(
        enter_with(&mut hub, e, "General", Some("dog"), None),
        Err(JoinError::Password),
        "a stranger learns nothing about a locked room"
    );
    assert_eq!(
        enter_with(&mut hub, e, "General", Some("fox"), None),
        Err(JoinError::Full)
    );
}

#[test]
fn entering_the_room_one_is_already_in_changes_nothing() {
    let mut hub = Hub::new(8);
    let (a, _rx_a) = someone(&mut hub, "Panda", 'x');
    let (b, mut rx_b) = someone(&mut hub, "Raccoon", 'x');
    let opened = enter_with(&mut hub, a, "Backroom", None, rules(Some("owl"), Some(3))).unwrap();
    enter_with(&mut hub, b, "Backroom", Some("owl"), None).unwrap();
    drain(&mut rx_b);

    let (others, state) = hub.join(a, &room("Backroom"), None, None).unwrap();
    assert_eq!(
        state, opened,
        "the host keeps the crown, the lock and the limit"
    );
    assert_eq!(others.len(), 1);
    assert!(drain(&mut rx_b).is_empty(), "nobody is told anything");

    let (alone, _rx) = someone(&mut hub, "Fox", 'x');
    enter_with(&mut hub, alone, "Alone", None, rules(Some("cat"), None)).unwrap();
    assert!(
        enter_with(&mut hub, alone, "Alone", None, None)
            .unwrap()
            .locked,
        "a room with one person is not opened afresh"
    );
}
