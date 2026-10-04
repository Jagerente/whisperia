#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    if std::env::var_os("WW_GUI_HARDWARE").is_none() {
        println!("skipped: set WW_GUI_HARDWARE=1 to let it open a window");
        return;
    }
    hardware::run();
}

#[cfg(windows)]
mod hardware {
    use std::{
        cell::Cell,
        rc::Rc,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        time::{Duration, Instant},
    };

    use whisperia::{
        audio::fake::FakeBackend,
        config::{Hotkeys, Paths, ShareConfig},
        desktop,
        discovery::Search,
        gui,
        net::SessionEvent,
        preview::fake::FakePreviews,
        screen::{self, ShareOut, SourceKey, SourceKind},
        wake::Waker,
    };
    use whisperia_e2e::{gui::named, net, TempDir, TestServer};
    use whisperia_winmedia::{Pattern, SourceId};

    const ROOM: &str = "General";

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Step {
        EnterRoom,
        Watch,
        FirstPicture,
        Windowed,
        FullScreen,
        WindowedAgain,
        Choose,
        Sharing,
        AskKey,
        KeyAsked,
        Done,
    }

    pub(super) fn run() {
        let dir = TempDir::new("gui-hw");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _in_runtime = runtime.enter();
        let server = TestServer::named("DESKTOP-AV83").max_peers(4).start();
        let addr = server.addr();

        let (raccoon, _, mut raccoon_events) = runtime.block_on(net::connect(addr, "Raccoon"));
        net::enter(&raccoon, ROOM);
        net::share_start(&raccoon, false);
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
        let queue = raccoon.picture_sender();
        let ended = Arc::new(Mutex::new(None));
        let end = ended.clone();
        let share = Rc::new(
            backend
                .share(
                    moving,
                    &ShareConfig::default(),
                    ShareOut {
                        pictures: Box::new(move |picture| {
                            let _ = queue.try_send(picture);
                        }),
                        sound: None,
                        ended: Box::new(move |reason| {
                            *end.lock().unwrap() = Some(reason.to_string())
                        }),
                        news: Box::new(|_| {}),
                    },
                )
                .expect("the screen can be shown"),
        );
        println!("encoder: {}", share.details());
        let keyframe_wanted = Arc::new(AtomicBool::new(false));
        let wanted = keyframe_wanted.clone();
        runtime.spawn(async move {
            while let Some(event) = raccoon_events.recv().await {
                if let SessionEvent::KeyframeWanted = event {
                    wanted.store(true, Ordering::Relaxed);
                }
            }
        });

        let press_keys = std::env::var_os("WW_PRESS_KEYS").is_some();
        let waker = Waker::default();
        let options = gui::Options {
            paths: Paths::resolve(Some(dir.join("panda"))).unwrap(),
            config: named("Panda"),
            backend: Arc::new(FakeBackend::quiet(48_000, Some((440.0, 0.3)))),
            screen: screen::default_backend(),
            previews: Arc::new(FakePreviews::default()),
            connect: Some(addr.to_string()),
            desktop: press_keys
                .then(|| desktop::start(&Hotkeys::default(), &waker))
                .flatten(),
            instance: None,
            waker,
            search: Search::Nowhere,
        };

        let seen = Rc::new(Cell::new((Step::EnterRoom, 0u64, 0u64, 0u64)));
        let report = seen.clone();
        let started = Instant::now();
        let mut step = Step::EnterRoom;
        let mut since = Instant::now();
        let (mut windowed, mut full_screen, mut smaller) = (0u64, 0u64, 0u64);
        let raccoon_share = share.clone();
        gui::run_driven(options, move |hands| {
            if keyframe_wanted.swap(false, Ordering::Relaxed) {
                raccoon_share.request_keyframe();
            }
            let next = match step {
                Step::EnterRoom => hands.press(&format!("Enter {ROOM}")).then_some(Step::Watch),
                Step::Watch => hands
                    .press("Watch what Raccoon shows")
                    .then_some(Step::FirstPicture),
                Step::FirstPicture => (hands.pictures_shown() > 0
                    && hands.screen().contains("[image] Raccoon's screen"))
                .then_some(Step::Windowed),
                Step::Windowed if since.elapsed() >= Duration::from_secs(3) => {
                    windowed = hands.pictures_shown();
                    assert!(hands.press("Full screen"), "{}", hands.screen());
                    Some(Step::FullScreen)
                }
                Step::FullScreen if since.elapsed() >= Duration::from_secs(3) => {
                    full_screen = hands.pictures_shown() - windowed;
                    assert!(hands.press("Leave full screen"), "{}", hands.screen());
                    raccoon_share.reconfigure(&ShareConfig {
                        max_height: 720,
                        fps: 30,
                    });
                    Some(Step::WindowedAgain)
                }
                Step::WindowedAgain if since.elapsed() >= Duration::from_secs(3) => {
                    smaller = hands.pictures_shown() - windowed - full_screen;
                    assert!(hands.press("Share my screen"), "{}", hands.screen());
                    Some(Step::Choose)
                }
                Step::Choose => {
                    let asked = hands.screen();
                    let offered = asked
                        .lines()
                        .filter(|line| {
                            line.ends_with(" (off)")
                                && !line.starts_with("[button] Resolution: ")
                                && !line.starts_with("[button] Frames per second: ")
                        })
                        .count();
                    (since.elapsed() > Duration::from_secs(1)).then(|| {
                        println!("offered: {offered} screens and windows in sight");
                        assert!(offered >= 1, "nothing to choose from");
                        assert!(hands.press("Screen 1"), "no first screen");
                        assert!(hands.press("Share"), "no button to share with");
                        Step::Sharing
                    })
                }
                Step::Sharing if since.elapsed() >= Duration::from_secs(2) => {
                    let room = hands.screen();
                    assert!(
                        room.contains("\nYou are sharing\n"),
                        "the share did not start. The window says:\n{room}"
                    );
                    assert!(
                        hands.press("Stop sharing my screen"),
                        "the share did not last"
                    );
                    if !press_keys {
                        report.set((Step::Done, windowed, full_screen, smaller));
                        return false;
                    }
                    Some(Step::AskKey)
                }
                Step::AskKey => {
                    assert!(hands.press("Settings"), "{}", hands.screen());
                    assert!(hands.press("Keybinds"), "{}", hands.screen());
                    assert!(hands.press("Key for Mute / Unmute"), "{}", hands.screen());
                    println!("waiting for Ctrl+Shift+F8...");
                    Some(Step::KeyAsked)
                }
                Step::KeyAsked => {
                    let settings = hands.screen();
                    if since.elapsed() > Duration::from_secs(20) {
                        let line = settings.lines().find(|l| l.contains("Key for Mute"));
                        panic!("no key came: {line:?}");
                    }
                    assert!(
                        !settings.contains("Key for Mute / Unmute = Ctrl+Alt+M"),
                        "the window was not in front, or Esc was pressed"
                    );
                    if settings.contains("Key for Mute / Unmute = Ctrl+Shift+F8") {
                        report.set((Step::Done, windowed, full_screen, smaller));
                        return false;
                    }
                    None
                }
                _ => None,
            };
            if let Some(next) = next {
                println!("{:>5.1} s  {step:?} done", started.elapsed().as_secs_f32());
                (step, since) = (next, Instant::now());
            }
            report.set((step, windowed, full_screen, smaller));
            started.elapsed() < Duration::from_secs(40)
        })
        .expect("the window opens");

        let (step, windowed, full_screen, smaller) = seen.get();
        println!(
            "pictures shown: {windowed} in 3 s in the window, {full_screen} in 3 s full screen, \
             {smaller} in 3 s at 720p30"
        );
        assert_eq!(step, Step::Done, "the script got stuck");
        assert!(
            windowed > 60,
            "pictures made it into the window: {windowed}"
        );
        assert!(
            full_screen > 60,
            "and kept coming full screen: {full_screen}"
        );
        assert!(
            (40..=110).contains(&smaller),
            "and after the change of size, at its rate: {smaller}"
        );
        let ended = ended.lock().unwrap();
        assert!(ended.is_none(), "{ended:?}");
        drop(server);
    }
}
