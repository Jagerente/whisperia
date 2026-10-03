use std::sync::atomic::Ordering;

use whisperia::net::SessionEvent;
use whisperia_e2e::{
    net::{
        answer_keyframes, catch_without_asking, member_of, next_event_where,
        send_pictures_and_sound, share_start, share_stop, unwatch, wait_for_pictures, watch,
        watch_only, Events, KEYFRAME_BYTES,
    },
    TestServer,
};
use whisperia_proto::{control::ShareInfo, PeerId};

async fn told_of_a_share(events: &mut Events, sharer: PeerId) -> Option<ShareInfo> {
    match next_event_where(
        events,
        "news of the share",
        |event| matches!(event, SessionEvent::PeerShare { id, .. } if *id == sharer),
    )
    .await
    {
        SessionEvent::PeerShare { share, .. } => share,
        _ => unreachable!("only news of the share was waited for"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn watching_starts_on_a_keyframe_and_misses_nothing() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = member_of(server.addr(), "Panda", "Backroom").await;
    let mut raccoon = member_of(server.addr(), "Raccoon", "Backroom").await;

    share_start(&panda.session, true);
    let pictures = send_pictures_and_sound(&panda.session);
    assert_eq!(
        told_of_a_share(&mut raccoon.events, panda.id).await,
        Some(ShareInfo {
            sound: true,
            minimized: false
        })
    );
    let events = std::mem::replace(&mut panda.events, tokio::sync::mpsc::unbounded_channel().1);
    let sharer = answer_keyframes(events, &pictures);
    pictures.wait_until_sent(20).await;

    let seen = watch(&raccoon.session, panda.id);
    let shown = wait_for_pictures(&seen, 50).await;
    assert!(sharer.asked_for_keyframes.load(Ordering::Relaxed));
    assert!(shown[0].0, "starts with a keyframe: {:?}", &shown[..3]);
    assert!(shown[0].1 > 10, "in the middle of the share");
    assert_eq!(shown[0].2, KEYFRAME_BYTES, "whole, however large");
    assert!(
        shown.windows(2).all(|w| w[1].1 == w[0].1 + 1),
        "in order, none missing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sharer_learns_who_watches() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = member_of(server.addr(), "Panda", "Backroom").await;
    let mut raccoon = member_of(server.addr(), "Raccoon", "Backroom").await;
    share_start(&panda.session, false);
    let pictures = send_pictures_and_sound(&panda.session);
    told_of_a_share(&mut raccoon.events, panda.id).await;
    let events = std::mem::replace(&mut panda.events, tokio::sync::mpsc::unbounded_channel().1);
    let sharer = answer_keyframes(events, &pictures);

    let seen = watch(&raccoon.session, panda.id);
    wait_for_pictures(&seen, 5).await;
    sharer.wait_for_viewers(&[raccoon.id]).await;

    unwatch(&raccoon.session, panda.id);
    sharer.wait_for_viewers(&[]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pictures_reach_only_those_who_asked_for_them() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = member_of(server.addr(), "Panda", "Backroom").await;
    let mut raccoon = member_of(server.addr(), "Raccoon", "Backroom").await;
    let fox = member_of(server.addr(), "Fox", "Backroom").await;
    share_start(&panda.session, true);
    let pictures = send_pictures_and_sound(&panda.session);
    told_of_a_share(&mut raccoon.events, panda.id).await;
    let events = std::mem::replace(&mut panda.events, tokio::sync::mpsc::unbounded_channel().1);
    answer_keyframes(events, &pictures);

    let unasked = catch_without_asking(&fox.session, panda.id);
    let seen = watch(&raccoon.session, panda.id);
    wait_for_pictures(&seen, 50).await;
    assert!(
        unasked.lock().unwrap().is_empty(),
        "Fox did not ask to watch"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pictures_before_a_share_reach_nobody() {
    let server = TestServer::start("DESKTOP-AV83");
    let panda = member_of(server.addr(), "Panda", "Backroom").await;
    let mut raccoon = member_of(server.addr(), "Raccoon", "Backroom").await;

    let pictures = send_pictures_and_sound(&panda.session);
    let seen = watch(&raccoon.session, panda.id);
    pictures.wait_until_sent(35).await;
    assert!(
        seen.lock().unwrap().is_empty(),
        "pictures of a share nobody was told of"
    );

    share_start(&panda.session, false);
    told_of_a_share(&mut raccoon.events, panda.id).await;
    watch_only(&raccoon.session, panda.id);
    pictures.keyframe.store(true, Ordering::Relaxed);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while seen.lock().unwrap().len() < 10 {
        assert!(
            std::time::Instant::now() < deadline,
            "the announced share never arrived"
        );
        pictures.keyframe.store(true, Ordering::Relaxed);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_share_tells_everyone() {
    let server = TestServer::start("DESKTOP-AV83");
    let panda = member_of(server.addr(), "Panda", "Backroom").await;
    let mut raccoon = member_of(server.addr(), "Raccoon", "Backroom").await;
    let mut fox = member_of(server.addr(), "Fox", "Backroom").await;
    share_start(&panda.session, true);
    let pictures = send_pictures_and_sound(&panda.session);
    for events in [&mut raccoon.events, &mut fox.events] {
        assert!(told_of_a_share(events, panda.id).await.is_some());
    }
    let seen = watch(&raccoon.session, panda.id);
    wait_for_pictures(&seen, 5).await;

    drop(pictures);
    share_stop(&panda.session);
    for events in [&mut raccoon.events, &mut fox.events] {
        assert_eq!(told_of_a_share(events, panda.id).await, None);
    }
}
