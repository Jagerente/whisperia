use whisperia::{net::ConnectError, trust::Fingerprint};
use whisperia_e2e::{
    net::{connect_trusting, member, next_event_where, target, trust, try_connect, with_password},
    TestServer,
};
use whisperia_proto::control::RejectReason;

use whisperia::net::SessionEvent;

#[tokio::test(flavor = "multi_thread")]
async fn the_welcome_names_the_server_and_its_rooms() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut raccoon = member(server.addr(), "Raccoon").await;
    let panda = member(server.addr(), "Panda").await;

    assert_eq!(panda.welcome.server_name, "DESKTOP-AV83");
    assert_eq!(panda.welcome.rooms.len(), 1);
    assert_eq!(panda.welcome.rooms[0].name, "General");
    assert_eq!(
        whisperia_e2e::net::names(&panda.welcome.rooms[0]),
        ["Raccoon"]
    );
    let joined = next_event_where(&mut raccoon.events, "Panda joining", |event| {
        matches!(event, SessionEvent::PeerJoined(_))
    })
    .await;
    assert!(matches!(joined, SessionEvent::PeerJoined(p) if p.name == "Panda"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_welcome_tells_a_second_panda_the_name_they_got() {
    let server = TestServer::start("DESKTOP-AV83");
    let _first = member(server.addr(), "Panda").await;
    let second = member(server.addr(), "Panda").await;
    assert_eq!(second.welcome.name, "Panda (2)");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_is_refused() {
    let server = TestServer::named("DESKTOP-AV83")
        .password("correct_pass")
        .start();
    let without = try_connect(target(server.addr()), "Panda").await;
    assert!(matches!(
        without,
        Err(ConnectError::Rejected(RejectReason::BadPassword))
    ));
    let wrong = try_connect(with_password(target(server.addr()), "wrong_pass"), "Panda").await;
    assert!(matches!(
        wrong,
        Err(ConnectError::Rejected(RejectReason::BadPassword))
    ));
    let right = try_connect(
        with_password(target(server.addr()), "correct_pass"),
        "Panda",
    )
    .await;
    assert!(right.is_ok(), "the right password lets in");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_server_refuses() {
    let server = TestServer::named("DESKTOP-AV83").max_peers(1).start();
    let _raccoon = try_connect(target(server.addr()), "Raccoon").await.unwrap();
    let full = try_connect(target(server.addr()), "Panda").await;
    assert!(matches!(
        full,
        Err(ConnectError::Rejected(RejectReason::ServerFull))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_certificate_is_refused_until_accepted() {
    let mut server = TestServer::start("DESKTOP-AV83");
    let (addr, port) = (server.addr(), server.port());
    let trust = trust();
    let first = connect_trusting(target(addr), "Panda", None, trust.clone())
        .await
        .unwrap();
    drop(first);
    server.shutdown().await;

    let impostor = TestServer::named("DESKTOP-AV83").on_port(port).start();
    let refused = connect_trusting(target(addr), "Panda", None, trust.clone()).await;
    let Err(ConnectError::IdentityChanged { fingerprint }) = refused else {
        panic!("a changed certificate must be refused");
    };
    assert_eq!(fingerprint, Fingerprint::from(impostor.fingerprint()));

    let accepted = connect_trusting(target(addr), "Panda", Some(fingerprint), trust).await;
    assert!(accepted.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_is_not_the_one_remembered_is_refused_even_at_first_sight() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut expected = target(server.addr());
    expected.expected = Some(Fingerprint::from([7; 32]));
    let refused = try_connect(expected, "Panda").await;
    assert!(matches!(refused, Err(ConnectError::IdentityChanged { .. })));
}
