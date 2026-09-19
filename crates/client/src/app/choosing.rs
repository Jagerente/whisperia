use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
        Arc,
    },
    time::{Duration, Instant},
};

use crate::{
    screen::{
        ListedSource, PickedSource, Preview, ScreenBackend, ScreenError, SourceKey, SourceKind,
    },
    wake::{self, Waker, WakingSender},
};

const LOOK_AGAIN: Duration = Duration::from_secs(1);
const PICTURE_AGAIN: Duration = Duration::from_secs(3);

const PICTURING: Duration = Duration::from_millis(300);

enum Found {
    Sources(Vec<ListedSource>),
    Preview(SourceKey, Preview),
    Icon(SourceKey, Preview),
}

pub struct Offered {
    pub(crate) id: i32,
    pub(crate) source: ListedSource,
    pub(crate) preview: Option<Preview>,

    pub(crate) pictures: u32,
    pub(crate) icon: Option<Preview>,
}

pub struct Choosing {
    found: Receiver<Result<Found, ScreenError>>,

    _stop: Sender<()>,
    windows_in_sight: Arc<AtomicBool>,
    listed: bool,
    sources: Vec<Offered>,
    tab: SourceKind,
    chosen_screen: Option<i32>,
    chosen_window: Option<i32>,
    shared_now: Option<SourceKey>,
    next_id: i32,
}

impl Choosing {
    pub(super) fn start(
        screens: Arc<dyn ScreenBackend>,
        tab: SourceKind,
        shared_now: Option<SourceKey>,
        waker: &Waker,
    ) -> Option<Self> {
        let (found_tx, found) = wake::channel(waker);
        let (stop, stopped) = mpsc::channel();
        let mut choosing = Self::new(found, stop, tab);
        choosing.shared_now = shared_now;
        let windows_in_sight = choosing.windows_in_sight.clone();
        std::thread::Builder::new()
            .name("ww-share-list".into())
            .spawn(move || look(&*screens, &found_tx, &stopped, &windows_in_sight))
            .ok()?;
        Some(choosing)
    }

    fn new(found: Receiver<Result<Found, ScreenError>>, stop: Sender<()>, tab: SourceKind) -> Self {
        Self {
            found,
            _stop: stop,
            windows_in_sight: Arc::new(AtomicBool::new(tab == SourceKind::Window)),
            listed: false,
            sources: Vec::new(),
            tab,
            chosen_screen: None,
            chosen_window: None,
            shared_now: None,
            next_id: 0,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.listed
    }

    pub fn tab(&self) -> SourceKind {
        self.tab
    }

    pub fn offered(&self, kind: SourceKind) -> impl Iterator<Item = &Offered> {
        self.sources
            .iter()
            .filter(move |offered| offered.source.key.kind == kind)
    }

    pub fn chosen(&self) -> Option<i32> {
        match self.tab {
            SourceKind::Screen => self.chosen_screen,
            SourceKind::Window => self.chosen_window,
        }
    }

    pub(super) fn show(&mut self, tab: SourceKind) {
        self.tab = tab;
        self.windows_in_sight
            .store(tab == SourceKind::Window, Ordering::Relaxed);
    }

    pub(super) fn pick(&mut self, id: i32) {
        match self.kind_of(id) {
            Some(SourceKind::Screen) => self.chosen_screen = Some(id),
            Some(SourceKind::Window) => self.chosen_window = Some(id),
            None => {}
        }
    }

    pub(super) fn take(&mut self, id: i32) -> Option<PickedSource> {
        let at = self.sources.iter().position(|offered| offered.id == id)?;
        Some(self.sources.remove(at).source.into_picked())
    }

    fn offered_mut(&mut self, key: SourceKey) -> Option<&mut Offered> {
        self.sources
            .iter_mut()
            .find(|offered| offered.source.key == key)
    }

    fn kind_of(&self, id: i32) -> Option<SourceKind> {
        self.sources
            .iter()
            .find(|offered| offered.id == id)
            .map(|offered| offered.source.key.kind)
    }

    pub(super) fn tick(&mut self) -> Result<bool, ScreenError> {
        let mut arrived = false;
        loop {
            match self.found.try_recv() {
                Ok(Ok(Found::Sources(listed))) => self.take_listing(listed),
                Ok(Ok(Found::Preview(key, preview))) => {
                    if let Some(offered) = self.offered_mut(key) {
                        offered.preview = Some(preview);
                        offered.pictures = offered.pictures.wrapping_add(1);
                    }
                }
                Ok(Ok(Found::Icon(key, icon))) => {
                    if let Some(offered) = self.offered_mut(key) {
                        offered.icon = Some(icon);
                    }
                }
                Ok(Err(e)) => return Err(e),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(arrived),
            }
            arrived = true;
        }
    }

    fn take_listing(&mut self, mut listed: Vec<ListedSource>) {
        let mut seen = HashSet::new();
        listed.retain(|source| seen.insert(source.key));
        let mut listed: Vec<Option<ListedSource>> = listed.into_iter().map(Some).collect();
        let mut kept = Vec::with_capacity(self.sources.len());
        for mut offered in std::mem::take(&mut self.sources) {
            let still = listed
                .iter_mut()
                .find(|source| source.as_ref().is_some_and(|s| s.key == offered.source.key))
                .and_then(Option::take);
            if let Some(source) = still {
                offered.source = source;
                kept.push(offered);
            }
        }
        for source in listed.into_iter().flatten() {
            self.sources.push(Offered {
                id: self.next_id,
                source,
                preview: None,
                pictures: 0,
                icon: None,
            });
            self.next_id += 1;
        }
        self.sources.extend(kept);
        self.listed = true;

        let there = |id: &i32| self.sources.iter().any(|offered| offered.id == *id);
        self.chosen_screen = self.chosen_screen.filter(there);
        self.chosen_window = self.chosen_window.filter(there);
        if let Some(offered) = self.shared_now.take().and_then(|key| {
            self.sources
                .iter()
                .find(|offered| offered.source.key == key)
        }) {
            match offered.source.key.kind {
                SourceKind::Screen => self.chosen_screen = Some(offered.id),
                SourceKind::Window => self.chosen_window = Some(offered.id),
            }
        }
        if self.chosen_screen.is_none() {
            let screens: Vec<i32> = self.offered(SourceKind::Screen).map(|o| o.id).collect();
            if let [only] = screens[..] {
                self.chosen_screen = Some(only);
            }
        }
    }
}

fn look(
    screens: &dyn ScreenBackend,
    found: &WakingSender<Result<Found, ScreenError>>,
    stop: &Receiver<()>,
    windows_in_sight: &AtomicBool,
) {
    let stopped = || !matches!(stop.try_recv(), Err(TryRecvError::Empty));
    let picture = |key: SourceKey| match screens.preview(key) {
        Some(preview) => found.send(Ok(Found::Preview(key, preview))).is_ok(),
        None => true,
    };
    let mut pictured: HashMap<SourceKey, Instant> = HashMap::new();
    loop {
        let listed = match screens.list() {
            Ok(listed) => listed,
            Err(e) => {
                let _ = found.send(Err(e));
                return;
            }
        };
        let mut keys: Vec<SourceKey> = listed.iter().map(|source| source.key).collect();
        if found.send(Ok(Found::Sources(listed))).is_err() {
            return;
        }
        pictured.retain(|key, _| keys.contains(key));
        let in_sight = if windows_in_sight.load(Ordering::Relaxed) {
            SourceKind::Window
        } else {
            SourceKind::Screen
        };
        keys.sort_by_key(|key| key.kind != in_sight);
        keys.retain(|key| !pictured.contains_key(key));

        for key in keys {
            if stopped() {
                return;
            }
            if let Some(icon) = screens.icon(key) {
                if found.send(Ok(Found::Icon(key, icon))).is_err() {
                    return;
                }
            }
            if !picture(key) {
                return;
            }
            pictured.insert(key, Instant::now());
        }

        let started = Instant::now();
        let mut due: Vec<(Instant, SourceKey)> = pictured
            .iter()
            .filter(|(key, at)| key.kind == in_sight && at.elapsed() >= PICTURE_AGAIN)
            .map(|(key, at)| (*at, *key))
            .collect();
        due.sort_by_key(|(at, _)| *at);
        for (_, key) in due {
            if started.elapsed() > PICTURING {
                break;
            }
            if stopped() || !picture(key) {
                return;
            }
            pictured.insert(key, Instant::now());
        }

        if !matches!(
            stop.recv_timeout(LOOK_AGAIN),
            Err(RecvTimeoutError::Timeout)
        ) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(tab: SourceKind) -> Choosing {
        let (_, found) = mpsc::channel();
        let (stop, _) = mpsc::channel();
        Choosing::new(found, stop, tab)
    }

    fn source(kind: SourceKind, handle: u64, label: &str) -> ListedSource {
        ListedSource {
            key: SourceKey {
                kind,
                handle,
                process: 7,
            },
            label: label.into(),
            detail: String::new(),
            minimized: false,
        }
    }

    fn screen(handle: u64) -> ListedSource {
        source(SourceKind::Screen, handle, &format!("Screen {handle}"))
    }

    fn window(handle: u64, label: &str) -> ListedSource {
        source(SourceKind::Window, handle, label)
    }

    fn labels(choosing: &Choosing, kind: SourceKind) -> Vec<&str> {
        choosing
            .offered(kind)
            .map(|offered| offered.source.label.as_str())
            .collect()
    }

    #[test]
    fn what_is_new_comes_first_and_the_rest_stays_where_it_was() {
        let mut choosing = quiet(SourceKind::Window);
        assert!(!choosing.is_complete());
        choosing.take_listing(vec![screen(1), window(10, "Mail"), window(11, "Notes")]);
        assert!(choosing.is_complete());
        assert_eq!(labels(&choosing, SourceKind::Window), ["Mail", "Notes"]);

        choosing.take_listing(vec![
            screen(1),
            window(12, "Game"),
            window(11, "Notes"),
            window(10, "Mail"),
        ]);
        assert_eq!(
            labels(&choosing, SourceKind::Window),
            ["Game", "Mail", "Notes"]
        );

        choosing.take_listing(vec![
            screen(1),
            window(11, "Notes - edited"),
            window(12, "Game"),
        ]);
        assert_eq!(
            labels(&choosing, SourceKind::Window),
            ["Game", "Notes - edited"]
        );
        assert_eq!(labels(&choosing, SourceKind::Screen), ["Screen 1"]);
    }

    #[test]
    fn a_handle_given_to_another_program_is_another_window() {
        let mut choosing = quiet(SourceKind::Window);
        choosing.take_listing(vec![window(10, "Mail")]);
        let before = choosing.offered(SourceKind::Window).next().unwrap().id;
        let mut reborn = window(10, "Calculator");
        reborn.key.process = 8;
        choosing.take_listing(vec![reborn]);
        let after = choosing.offered(SourceKind::Window).next().unwrap().id;
        assert_ne!(before, after);
    }

    #[test]
    fn what_is_listed_twice_is_offered_once() {
        let mut choosing = quiet(SourceKind::Window);
        choosing.take_listing(vec![window(10, "Mail"), window(10, "Mail")]);
        choosing.take_listing(vec![window(10, "Mail"), window(10, "Mail")]);
        let ids: Vec<i32> = choosing.offered(SourceKind::Window).map(|o| o.id).collect();
        assert_eq!(ids, [0]);
    }

    #[test]
    fn a_lone_screen_is_chosen_by_itself() {
        let mut choosing = quiet(SourceKind::Screen);
        choosing.take_listing(vec![screen(1), window(10, "Mail")]);
        assert_eq!(choosing.chosen(), Some(0));
        choosing.show(SourceKind::Window);
        assert_eq!(
            choosing.chosen(),
            None,
            "a window is never chosen for anybody"
        );

        let mut choosing = quiet(SourceKind::Screen);
        choosing.take_listing(vec![screen(1), screen(2)]);
        assert_eq!(choosing.chosen(), None);
    }

    #[test]
    fn what_is_shared_now_is_chosen_once_the_list_arrives() {
        let mut choosing = quiet(SourceKind::Window);
        choosing.shared_now = Some(window(11, "Notes").key);
        choosing.take_listing(vec![screen(1), window(10, "Mail"), window(11, "Notes")]);
        assert_eq!(choosing.chosen(), Some(2));

        choosing.pick(1);
        choosing.take_listing(vec![screen(1), window(10, "Mail"), window(11, "Notes")]);
        assert_eq!(choosing.chosen(), Some(1), "a choice made since is kept");
    }

    #[test]
    fn each_tab_keeps_its_own_choice_until_that_goes_away() {
        let mut choosing = quiet(SourceKind::Screen);
        choosing.take_listing(vec![
            screen(1),
            screen(2),
            window(10, "Mail"),
            window(11, "Notes"),
        ]);
        choosing.pick(1);
        choosing.show(SourceKind::Window);
        choosing.pick(3);
        assert_eq!(choosing.chosen(), Some(3));
        choosing.show(SourceKind::Screen);
        assert_eq!(choosing.chosen(), Some(1));

        choosing.take_listing(vec![screen(1), screen(2), window(10, "Mail")]);
        choosing.show(SourceKind::Window);
        assert_eq!(choosing.chosen(), None, "Notes was closed");
        choosing.pick(99);
        assert_eq!(choosing.chosen(), None);

        assert_eq!(
            choosing.take(1).map(|picked| picked.label).as_deref(),
            Some("Screen 2")
        );
        assert!(choosing.take(1).is_none());
    }

    #[test]
    fn a_picture_finds_its_card_wherever_that_went() {
        let mut choosing = quiet(SourceKind::Window);
        choosing.take_listing(vec![window(10, "Mail")]);
        choosing.take_listing(vec![window(11, "Notes"), window(10, "Mail")]);
        let (found_tx, found) = mpsc::channel();
        choosing.found = found;
        let picture = || Preview {
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        };
        let mail = choosing
            .offered(SourceKind::Window)
            .nth(1)
            .unwrap()
            .source
            .key;
        found_tx.send(Ok(Found::Icon(mail, picture()))).unwrap();
        found_tx.send(Ok(Found::Preview(mail, picture()))).unwrap();
        found_tx.send(Ok(Found::Preview(mail, picture()))).unwrap();
        choosing.tick().unwrap();
        let pictured: Vec<(u32, bool)> = choosing
            .offered(SourceKind::Window)
            .map(|offered| (offered.pictures, offered.icon.is_some()))
            .collect();
        assert_eq!(pictured, [(0, false), (2, true)]);
    }

    #[test]
    fn what_is_in_sight_is_pictured_again_and_the_rest_once() {
        let screens = Arc::new(crate::screen::fake::FakeScreen::new());
        let mut choosing =
            Choosing::start(screens, SourceKind::Screen, None, &Waker::default()).unwrap();
        let deadline = Instant::now() + PICTURE_AGAIN + LOOK_AGAIN * 3;
        let pictures = |choosing: &Choosing, kind| {
            choosing
                .offered(kind)
                .map(|offered| offered.pictures)
                .collect::<Vec<_>>()
        };
        while Instant::now() < deadline && pictures(&choosing, SourceKind::Screen) != [2, 2] {
            std::thread::sleep(Duration::from_millis(50));
            choosing.tick().unwrap();
        }
        assert_eq!(pictures(&choosing, SourceKind::Screen), [2, 2]);
        assert_eq!(pictures(&choosing, SourceKind::Window), [1, 1, 1]);
        assert!(choosing
            .offered(SourceKind::Window)
            .all(|offered| offered.icon.is_some()));
        assert!(choosing
            .offered(SourceKind::Screen)
            .all(|offered| offered.icon.is_none()));
    }
}
