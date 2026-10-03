use whisperia::net::SessionEvent;
use whisperia_e2e::{
    net::{enter, member, names, next_event_where, wait_for_occupancy},
    TestServer,
};
use whisperia_proto::control::ClientMsg;

#[tokio::test(flavor = "multi_thread")]
async fn moving_to_another_room_is_told_and_counted() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut raccoon = member(server.addr(), "Raccoon").await;
    let mut panda = member(server.addr(), "Panda").await;
    next_event_where(
        &mut raccoon.events,
        "Panda joining",
        |event| matches!(event, SessionEvent::PeerJoined(p) if p.name == "Panda"),
    )
    .await;
    wait_for_occupancy(&server, (1, 2)).await;

    enter(&panda.session, "Room 83");
    let moved = next_event_where(&mut panda.events, "Room 83 to be joined", |event| {
        matches!(event, SessionEvent::Joined { .. })
    })
    .await;
    assert!(
        matches!(moved, SessionEvent::Joined { room, peers, .. } if room == "Room 83" && peers.is_empty())
    );
    let panda_id = panda.id;
    next_event_where(
        &mut raccoon.events,
        "Panda leaving General",
        |event| matches!(event, SessionEvent::PeerLeft(id) if *id == panda_id),
    )
    .await;
    wait_for_occupancy(&server, (2, 2)).await;

    panda.session.send(ClientMsg::Leave);
    let rooms = next_event_where(
        &mut panda.events,
        "a list of one room",
        |event| matches!(event, SessionEvent::Rooms(list) if list.len() == 1),
    )
    .await;
    let SessionEvent::Rooms(rooms) = rooms else {
        unreachable!("only a room list was waited for");
    };
    assert_eq!(rooms[0].name, "General");
    assert_eq!(names(&rooms[0]), ["Raccoon"]);
    wait_for_occupancy(&server, (1, 2)).await;

    drop(raccoon);
    drop(panda);
    wait_for_occupancy(&server, (0, 0)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn leaving_is_told() {
    let server = TestServer::start("DESKTOP-AV83");
    let panda = member(server.addr(), "Panda").await;
    let mut raccoon = member(server.addr(), "Raccoon").await;
    let panda_id = panda.id;
    drop(panda);
    next_event_where(
        &mut raccoon.events,
        "Panda leaving",
        |event| matches!(event, SessionEvent::PeerLeft(id) if *id == panda_id),
    )
    .await;
}
