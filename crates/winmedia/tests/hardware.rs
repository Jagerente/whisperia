#![cfg(windows)]

use std::{
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use whisperia_winmedia::{
    icon_of, list_sources, preview_of, EndReason, ListedSource, Share, ShareSettings, Source,
    SourceId,
};

fn main_screen() -> (SourceId, (u32, u32)) {
    list_sources()
        .into_iter()
        .find_map(|source| match source {
            ListedSource::Screen { id, size } => Some((id, size)),
            ListedSource::Window { .. } => None,
        })
        .expect("there is a screen")
}

fn settings(screen: (u32, u32), lines: u32, fps: u32) -> ShareSettings {
    let lines = lines.min(screen.1);
    let width = (u64::from(screen.0) * u64::from(lines) / u64::from(screen.1)) as u32;
    ShareSettings::new((width / 2 * 2, lines / 2 * 2), fps, 8_000_000).expect("a size to encode")
}

type Ended = Arc<Mutex<Option<EndReason>>>;

fn remember(ended: &Ended) -> Box<dyn FnOnce(EndReason) + Send> {
    let ended = ended.clone();
    Box::new(move |reason| *ended.lock().unwrap() = Some(reason))
}

#[test]
#[ignore = "needs a desktop session and a hardware H.264 encoder"]
fn keyframes_come_when_asked_for_also_after_a_change_of_quality() {
    let count = |share: &Share, keyframes: &AtomicUsize, requests: usize| {
        let before = keyframes.load(Ordering::Relaxed);
        for _ in 0..requests {
            share.request_keyframe();
            std::thread::sleep(Duration::from_millis(200));
        }
        std::thread::sleep(Duration::from_millis(300));
        keyframes.load(Ordering::Relaxed) - before
    };
    let wait_for_height = |height: &AtomicU32, wanted: u32| {
        let asked = Instant::now();
        while height.load(Ordering::Relaxed) != wanted {
            assert!(
                asked.elapsed() < Duration::from_secs(5),
                "no picture of {wanted} lines within five seconds"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        asked.elapsed()
    };
    let (screen, size) = main_screen();

    for (lines, fps, then) in [
        (1440, 60, 720),
        (720, 30, 1080),
        (720, 15, 1440),
        (u32::MAX, 60, 720),
    ] {
        let (keyframes, height) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicU32::new(0)));
        let seen = (keyframes.clone(), height.clone());
        let ended = Ended::default();
        let first = settings(size, lines, fps);
        let share = Share::start(
            Source::open(screen).expect("the screen opens"),
            first,
            Box::new(move |frame| {
                seen.1.store(frame.height, Ordering::Relaxed);
                if frame.keyframe {
                    seen.0.fetch_add(1, Ordering::Relaxed);
                }
            }),
            remember(&ended),
        )
        .expect("the screen can be shown");
        wait_for_height(&height, first.size().1);
        let fresh = count(&share, &keyframes, 8);

        let second = settings(size, then, 30);
        share.reconfigure(second);
        let took = wait_for_height(&height, second.size().1);
        let changed = count(&share, &keyframes, 8);
        println!(
            "{:?}@{fps}: {fresh} of 8 keyframes; the change to {:?}@30 took {took:.0?}, then {changed} of 8",
            first.size(),
            second.size()
        );
        assert!(
            fresh >= 7,
            "{:?}@{fps}: {fresh} of 8 keyframes",
            first.size()
        );
        assert!(changed >= 7, "after the change: {changed} of 8 keyframes");
        assert!(
            took < Duration::from_millis(500),
            "the change took {took:?}"
        );
        assert!(
            ended.lock().unwrap().is_none(),
            "{:?}",
            ended.lock().unwrap()
        );
    }
}

#[test]
#[ignore = "needs a desktop session and a hardware H.264 encoder"]
fn what_is_listed_has_a_picture_and_can_be_shown() {
    let asked = Instant::now();
    let listed = list_sources();
    let windows = listed
        .iter()
        .filter(|source| matches!(source, ListedSource::Window { .. }))
        .count();
    println!(
        "{} screens and {windows} windows in {:.0?}",
        listed.len() - windows,
        asked.elapsed()
    );
    assert!(listed.len() > windows, "a screen is listed first");

    let (main, size) = main_screen();
    assert_eq!(listed[0].id(), main);
    let pictured = Instant::now();
    let preview = preview_of(main).expect("a screen has a picture");
    let with_pictures = listed
        .iter()
        .filter(|source| preview_of(source.id()).is_some())
        .count();
    println!(
        "{with_pictures} of {} pictured in {:.0?}",
        listed.len(),
        pictured.elapsed()
    );
    let icons: Vec<_> = listed
        .iter()
        .filter_map(|source| icon_of(source.id()))
        .collect();
    println!("{} of {windows} windows with an icon", icons.len());
    assert!(windows == 0 || !icons.is_empty());
    for icon in &icons {
        assert_eq!((icon.width, icon.height), (32, 32));
        let solid = icon.rgba.chunks_exact(4).filter(|p| p[3] > 200).count();
        assert!(solid > 32, "an icon is more than a few dots: {solid}");
    }
    assert_eq!(
        preview.rgba.len(),
        (preview.width * preview.height * 4) as usize
    );
    assert!(preview.width <= 384 && preview.height <= 240);
    let brightest = preview.rgba.chunks_exact(4).flat_map(|p| &p[..3]).max();
    assert!(brightest > Some(&60), "{brightest:?}");
    for source in &listed {
        if let ListedSource::Window { title, .. } = source {
            assert!(!title.trim().is_empty());
        }
    }

    let pictures = Arc::new(AtomicUsize::new(0));
    let seen = pictures.clone();
    let ended = Ended::default();
    let source = Source::open(main).expect("what was listed can be opened");
    assert_eq!(source.size().expect("the screen has a size"), size);
    let share = Share::start(
        source,
        settings(size, 1080, 60),
        Box::new(move |_| {
            seen.fetch_add(1, Ordering::Relaxed);
        }),
        remember(&ended),
    )
    .expect("the screen can be shown");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(pictures.load(Ordering::Relaxed) >= 1);
    assert!(
        ended.lock().unwrap().is_none(),
        "{:?}",
        ended.lock().unwrap()
    );
    drop(share);
}
