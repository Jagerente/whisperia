#![cfg(windows)]

use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use whisperia_winmedia::{Pattern, Source, WindowNews, WindowWatch};

const PATIENCE: Duration = Duration::from_secs(5);

fn next_news(heard: &Receiver<WindowNews>, wanted: impl Fn(&WindowNews) -> bool) -> WindowNews {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match heard.recv_timeout(left) {
            Ok(news) if wanted(&news) => return news,
            Ok(_) => {}
            Err(_) => panic!("the window said nothing of the kind within {PATIENCE:?}"),
        }
    }
}

#[test]
#[ignore = "needs a desktop session"]
fn a_watched_window_tells_its_size_and_when_it_is_minimized_and_back() {
    let pattern = Pattern::show().expect("the test pattern opens");
    let (tell, heard) = mpsc::channel();
    let _watch = WindowWatch::start(
        pattern.source(),
        Box::new(move |news| {
            let _ = tell.send(news);
        }),
    )
    .expect("the window can be watched")
    .expect("a window is watched");

    let WindowNews::Resized(first) = next_news(&heard, |n| matches!(n, WindowNews::Resized(_)))
    else {
        unreachable!()
    };
    pattern.resize(700, 500).expect("the pattern resizes");
    let WindowNews::Resized(second) = next_news(&heard, |n| matches!(n, WindowNews::Resized(_)))
    else {
        unreachable!()
    };
    println!("{first:?} became {second:?}");
    assert!(
        second.0 < first.0 && second.1 < first.1,
        "{first:?} {second:?}"
    );

    pattern.minimize();
    next_news(&heard, |n| *n == WindowNews::Minimized);
    let _source = Source::open(pattern.source()).expect("a minimized window opens");
    next_news(&heard, |n| *n == WindowNews::Restored);
    assert!(
        !pattern.is_minimized(),
        "sharing a minimized window restores it"
    );
}
