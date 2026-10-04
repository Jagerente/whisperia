#![cfg(windows)]

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

use whisperia::{
    config::ShareConfig,
    net::SessionEvent,
    screen::{self, ShareOut, SourceKey, SourceKind, SourceNews},
};
use whisperia_e2e::{
    net::{member_of, next_event_where, share_start, watch},
    TestServer,
};
use whisperia_winmedia::{Pattern, SourceId};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a desktop session and a hardware H.264 encoder"]
async fn a_moving_window_arrives_at_the_other_end_at_the_rate_asked_for() {
    let server = TestServer::named("DESKTOP-AV83").max_peers(4).start();
    let mut panda = member_of(server.addr(), "Panda", "Backroom").await;
    let mut raccoon = member_of(server.addr(), "Raccoon", "Backroom").await;

    let backend = screen::default_backend();
    let pattern = Pattern::show().expect("the test pattern opens");
    let SourceId::Window { handle, process } = pattern.source() else {
        unreachable!("the pattern is a window");
    };
    let moving = SourceKey {
        kind: SourceKind::Window,
        handle: handle as u64,
        process,
    };
    share_start(&panda.session, false);
    let queue = panda.session.picture_sender();
    let (sent, sent_bytes) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let counters = (sent.clone(), sent_bytes.clone());
    let ended = Arc::new(Mutex::new(None));
    let end = ended.clone();
    let share = backend
        .share(
            moving,
            &ShareConfig::default(),
            ShareOut {
                pictures: Box::new(move |picture| {
                    counters.0.fetch_add(1, Ordering::Relaxed);
                    counters
                        .1
                        .fetch_add(picture.data.len() as u64, Ordering::Relaxed);
                    let _ = queue.try_send(picture);
                }),
                sound: None,
                ended: Box::new(move |reason| *end.lock().unwrap() = Some(reason.to_string())),
                news: Box::new(|_| {}),
            },
        )
        .expect("the screen can be shown");
    println!("encoder: {}", share.details());

    let sharer = panda.id;
    next_event_where(
        &mut raccoon.events,
        "news of the share",
        |event| matches!(event, SessionEvent::PeerShare { id, share: Some(_) } if *id == sharer),
    )
    .await;
    let seen = watch(&raccoon.session, sharer);
    let arrived = || seen.lock().unwrap().len() as u64;

    let mut arrived_at_first = 0;
    for (seconds, quality) in [(3, None), (2, Some((720, 30)))] {
        if let Some((max_height, fps)) = quality {
            arrived_at_first = arrived();
            println!(
                "{} pictures sent ({} KiB), {arrived_at_first} arrived; now {max_height}p at {fps}",
                sent.swap(0, Ordering::Relaxed),
                sent_bytes.swap(0, Ordering::Relaxed) / 1024,
            );
            share.reconfigure(&ShareConfig { max_height, fps });
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
        while tokio::time::Instant::now() < deadline {
            while let Ok(event) = panda.events.try_recv() {
                if let SessionEvent::KeyframeWanted = event {
                    share.request_keyframe();
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    let (sent, arrived) = (sent.load(Ordering::Relaxed), arrived());
    println!(
        "{sent} pictures sent ({} KiB) at the lower quality, {arrived} arrived in all",
        sent_bytes.load(Ordering::Relaxed) / 1024,
    );
    assert!(
        arrived_at_first > 30,
        "pictures made it to the other end: {arrived_at_first}"
    );
    assert!(
        arrived > arrived_at_first + 20,
        "and kept coming after the change: {arrived_at_first} then {arrived}"
    );
    assert!(
        (30..=70).contains(&sent),
        "two seconds at 30 pictures a second, not at 60: {sent}"
    );
    let ended = ended.lock().unwrap();
    assert!(ended.is_none(), "{ended:?}");
}

#[test]
#[ignore = "needs a desktop session and a hardware H.264 encoder"]
fn a_resized_window_is_sent_at_its_new_size() {
    let backend = screen::default_backend();
    let pattern = Pattern::show().expect("the test pattern opens");
    let SourceId::Window { handle, process } = pattern.source() else {
        unreachable!("the pattern is a window");
    };
    let size = Arc::new(Mutex::new((0u16, 0u16)));
    let seen = size.clone();
    let (tell, heard) = mpsc::channel();
    let share = backend
        .share(
            SourceKey {
                kind: SourceKind::Window,
                handle: handle as u64,
                process,
            },
            &ShareConfig::default(),
            ShareOut {
                pictures: Box::new(move |picture| {
                    *seen.lock().unwrap() = (picture.width, picture.height);
                }),
                sound: None,
                ended: Box::new(|_| {}),
                news: Box::new(move |news| {
                    let _ = tell.send(news);
                }),
            },
        )
        .expect("the window can be shown");
    let sent_while = |wanted: &dyn Fn((u16, u16)) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            for news in heard.try_iter() {
                if let SourceNews::Resized(size) = news {
                    share.follow(size);
                }
            }
            let now = *size.lock().unwrap();
            if wanted(now) {
                return now;
            }
            assert!(Instant::now() < deadline, "still sent at {now:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    let first = sent_while(&|(width, height)| width > 0 && height > 0);
    pattern.resize(700, 500).expect("the pattern resizes");
    let second = sent_while(&|(width, height)| width < first.0 && height < first.1);
    println!("sent at {first:?}, then at {second:?} once the window was made smaller");
    assert_eq!(
        share.stream().size,
        (u32::from(second.0), u32::from(second.1))
    );
}
