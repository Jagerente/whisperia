use whisperia::net::SessionEvent;
use whisperia_e2e::{
    net::{member, next_event_where, rename, say},
    TestServer,
};

#[tokio::test(flavor = "multi_thread")]
async fn a_message_is_cleaned_before_it_goes_out() {
    let server = TestServer::start("DESKTOP-AV83");
    let panda = member(server.addr(), "Panda").await;
    let mut raccoon = member(server.addr(), "Raccoon").await;

    say(&panda.session, "  Hello\x07 World! ");
    say(&panda.session, "\n");
    say(&panda.session, "ave 83");
    let mut heard = Vec::new();
    for _ in 0..2 {
        match next_event_where(&mut raccoon.events, "a message", |event| {
            matches!(event, SessionEvent::Chat { .. })
        })
        .await
        {
            SessionEvent::Chat { from, text } => heard.push((from, text.to_string())),
            _ => unreachable!("only messages were waited for"),
        }
    }
    let from_panda = panda.id;
    assert_eq!(
        heard,
        [
            (from_panda, "Hello  World!".to_owned()),
            (from_panda, "ave 83".to_owned())
        ],
        "control characters become spaces, the ends are trimmed and an empty message is dropped"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_taken_name_gets_a_number() {
    let server = TestServer::start("DESKTOP-AV83");
    let panda = member(server.addr(), "Panda").await;
    let mut raccoon = member(server.addr(), "Raccoon").await;

    rename(&panda.session, "Raccoon");
    let panda_id = panda.id;
    let renamed = next_event_where(&mut raccoon.events, "the rename", |event| {
        matches!(event, SessionEvent::PeerRenamed { .. })
    })
    .await;
    assert!(
        matches!(renamed, SessionEvent::PeerRenamed { id, name } if id == panda_id && name == "Raccoon (2)")
    );
}
