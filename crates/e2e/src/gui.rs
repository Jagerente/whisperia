use std::{
    ops::{Deref, DerefMut},
    sync::{mpsc::Receiver, Arc},
    time::{Duration, Instant},
};

use whisperia::{
    audio::fake::FakeBackend,
    config::{Appearance, Config, Paths},
    desktop::{DesktopCommand, DesktopEvent, DesktopLink, DesktopStatus},
    discovery::Search,
    dsp::denoise::DenoiseMode,
    gui::{Harness, Options},
    instance::Instance,
    preview::fake::FakePreviews,
    screen::fake::FakeScreen,
};

use crate::{TempDir, TestServer};

pub const PATIENCE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);
pub const LEFT_SIDE: f32 = 292.0;

pub fn named(name: &str) -> Config {
    Config {
        name: whisperia_proto::control::Name::new(name).expect("a test person has a name"),
        name_confirmed: true,
        avatar: crate::people::animal_of(name),
        denoise: DenoiseMode::Off,
        ..Config::default()
    }
}

pub struct Window {
    ui: Harness,
    dir: TempDir,
}

impl Window {
    pub fn saved_settings(&self) -> Config {
        let paths = Paths::resolve(Some(self.dir.path().to_owned())).unwrap();
        assert!(
            paths.config.exists(),
            "{} was never written",
            paths.config.display()
        );
        let (config, warning) = Config::load(&paths.config);
        assert_eq!(warning, None);
        config
    }
}

impl Deref for Window {
    type Target = Harness;

    fn deref(&self) -> &Harness {
        &self.ui
    }
}

impl DerefMut for Window {
    fn deref_mut(&mut self) -> &mut Harness {
        &mut self.ui
    }
}

pub struct Opening {
    config: Config,
    desktop: Option<DesktopLink>,
    screen: Arc<FakeScreen>,
    previews: Arc<FakePreviews>,
    backend: FakeBackend,
    instance: Option<Instance>,
    dir: Option<TempDir>,
    search: Search,
}

impl Opening {
    pub fn lan(mut self) -> Self {
        self.search = Search::Lan;
        self
    }

    pub fn desktop(mut self, link: DesktopLink) -> Self {
        self.desktop = Some(link);
        self
    }

    pub fn screen(mut self, screen: Arc<FakeScreen>) -> Self {
        self.screen = screen;
        self
    }

    pub fn previews(mut self, previews: Arc<FakePreviews>) -> Self {
        self.previews = previews;
        self
    }

    pub fn backend(mut self, backend: FakeBackend) -> Self {
        self.backend = backend;
        self
    }

    pub fn instance(mut self, instance: Instance) -> Self {
        self.instance = Some(instance);
        self
    }

    pub fn in_dir(mut self, dir: TempDir) -> Self {
        self.dir = Some(dir);
        self
    }

    pub fn open(self) -> Window {
        let dir = self.dir.unwrap_or_else(|| TempDir::new("gui"));
        let follows_windows = self.config.appearance == Appearance::System;
        let mut ui = Harness::new(Options {
            paths: Paths::resolve(Some(dir.path().to_owned())).unwrap(),
            config: self.config,
            backend: Arc::new(self.backend),
            screen: self.screen,
            previews: self.previews,
            connect: None,
            desktop: self.desktop,
            instance: self.instance,
            waker: whisperia::wake::Waker::default(),
            search: self.search,
        })
        .unwrap();
        if follows_windows {
            ui.use_dark_colours(std::env::var_os("WW_GUI_LIGHT").is_none());
        }
        Window { ui, dir }
    }
}

pub fn opening(config: Config) -> Opening {
    Opening {
        config,
        desktop: None,
        screen: Arc::new(FakeScreen::new()),
        previews: Arc::new(FakePreviews::default()),
        backend: FakeBackend::quiet(48_000, Some((440.0, 0.3))),
        instance: None,
        dir: None,
        search: Search::Nowhere,
    }
}

pub fn window(config: Config) -> Window {
    opening(config).open()
}

pub fn window_heard(config: Config) -> (Window, FakeBackend) {
    let backend = FakeBackend::new(48_000, Some((440.0, 0.3)));
    let ui = opening(config).backend(backend.clone()).open();
    (ui, backend)
}

#[track_caller]
pub fn wait_until(ui: &mut Harness, what: &str, mut done: impl FnMut(&mut Harness) -> bool) {
    wait_until_within(ui, what, PATIENCE, &mut done);
}

#[track_caller]
pub fn wait_until_within(
    ui: &mut Harness,
    what: &str,
    timeout: Duration,
    mut done: impl FnMut(&mut Harness) -> bool,
) {
    let deadline = Instant::now() + timeout;
    loop {
        ui.tick();
        if done(ui) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what} never happened. The window says:\n{}",
            ui.screen()
        );
        std::thread::sleep(POLL);
    }
}

#[track_caller]
pub fn wait_for(ui: &mut Harness, needle: &str) -> String {
    wait_for_within(ui, needle, PATIENCE)
}

#[track_caller]
pub fn wait_for_within(ui: &mut Harness, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        ui.tick();
        let screen = ui.screen();
        if screen.contains(needle) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "\"{needle}\" never appeared. The window says:\n{screen}"
        );
        std::thread::sleep(POLL);
    }
}

#[track_caller]
pub fn wait_until_gone(ui: &mut Harness, needle: &str) {
    wait_until_gone_within(ui, needle, PATIENCE);
}

#[track_caller]
pub fn wait_until_gone_within(ui: &mut Harness, needle: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        ui.tick();
        let screen = ui.screen();
        if !screen.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "\"{needle}\" never went away. The window says:\n{screen}"
        );
        std::thread::sleep(POLL);
    }
}

#[track_caller]
pub fn wait_for_text(ui: &mut Harness, text: &str) {
    wait_until(ui, &format!("the text \"{text}\""), |ui| ui.has_text(text));
}

#[track_caller]
pub fn wait_until_text_gone(ui: &mut Harness, text: &str) {
    wait_until(ui, &format!("the text \"{text}\" going away"), |ui| {
        !ui.has_text(text)
    });
}

#[track_caller]
pub fn wait_for_value(ui: &mut Harness, label: &str, value: &str) {
    wait_until(ui, &format!("\"{label}\" = {value}"), |ui| {
        ui.value_of(label).as_deref() == Some(value)
    });
}

#[track_caller]
pub fn wait_for_switch(ui: &mut Harness, label: &str, on: bool) {
    wait_until(ui, &format!("\"{label}\" turning {on}"), |ui| {
        ui.is_on(label) == Some(on)
    });
}

#[track_caller]
pub fn press(ui: &mut Harness, label: &str) {
    assert!(
        ui.press(label),
        "nothing is called \"{label}\". The window says:\n{}",
        ui.screen()
    );
    ui.tick();
}

#[track_caller]
pub fn type_into(ui: &mut Harness, label: &str, text: &str) {
    assert!(
        ui.type_into(label, text),
        "no field is called \"{label}\". The window says:\n{}",
        ui.screen()
    );
    ui.tick();
}

#[track_caller]
pub fn slide(ui: &mut Harness, label: &str, value: f32) {
    assert!(
        ui.slide(label, value),
        "no slider is called \"{label}\". The window says:\n{}",
        ui.screen()
    );
    ui.tick();
}

#[track_caller]
pub fn hover(ui: &mut Harness, label: &str) {
    assert!(
        ui.hover(label),
        "nothing to point at is called \"{label}\". The window says:\n{}",
        ui.screen()
    );
    ui.tick();
}

#[track_caller]
pub fn middle_of(ui: &Harness, label: &str) -> (f32, f32) {
    ui.middle_of(label).unwrap_or_else(|| {
        panic!(
            "nothing is called \"{label}\". The window says:\n{}",
            ui.screen()
        )
    })
}

pub fn picture(ui: &Harness, name: &str) {
    if let Some(dir) = std::env::var_os("WW_GUI_PICTURES") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        ui.picture(&dir.join(format!("{name}.png"))).unwrap();
    }
}

pub fn statuses(commands: &Receiver<DesktopCommand>) -> Vec<DesktopStatus> {
    commands
        .try_iter()
        .filter_map(|command| match command {
            DesktopCommand::Status(status) => Some(status),
            _ => None,
        })
        .collect()
}

#[track_caller]
pub fn told(commands: &Receiver<DesktopCommand>, wanted: DesktopCommand) {
    let deadline = Instant::now() + PATIENCE;
    while !commands.try_iter().any(|command| command == wanted) {
        assert!(Instant::now() < deadline, "the tray never heard {wanted:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[track_caller]
pub fn reach(ui: &mut Harness, address: &str) {
    type_into(ui, "Server address", address);
    press(ui, "Connect");
    wait_for(ui, "[button] New room");
}

#[track_caller]
pub fn enter_general(ui: &mut Harness, address: &str, me: &str) {
    reach(ui, address);
    if !ui.press("Enter General") {
        press(ui, "Open");
    }
    wait_for(ui, &format!("{me} (you)"));
}

#[track_caller]
pub fn say(ui: &mut Harness, text: &str) {
    type_into(ui, "Message", text);
    press(ui, "Send");
}

pub type Colour = (i32, i32, i32);

pub fn is_grey((r, g, b): Colour) -> bool {
    r.max(g).max(b) - r.min(g).min(b) < 30
}

pub fn is_red((r, g, b): Colour) -> bool {
    r > g + 80 && r > b + 60
}

pub fn is_colourful((r, g, b): Colour) -> bool {
    r.max(g).max(b) - r.min(g).min(b) > 40
}

fn colour_now(ui: &Harness, label: &str) -> Option<Colour> {
    let (r, g, b) = ui.most_colourful_in(label).ok()?;
    Some((r.into(), g.into(), b.into()))
}

#[track_caller]
pub fn wait_for_colour(ui: &mut Harness, label: &str, looks_right: impl Fn(Colour) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        ui.tick();
        let colour = colour_now(ui, label);
        if colour.is_some_and(&looks_right) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "\"{label}\" stays {colour:?}. The window says:\n{}",
            ui.screen()
        );
        std::thread::sleep(POLL);
    }
}

#[track_caller]
pub fn settled_colour(ui: &mut Harness, label: &str) -> Colour {
    let deadline = Instant::now() + PATIENCE;
    let mut before = None;
    loop {
        ui.tick();
        let now = colour_now(ui, label);
        if now.is_some() && now == before {
            return now.unwrap_or_default();
        }
        assert!(
            Instant::now() < deadline,
            "\"{label}\" never settled on a colour, last {now:?}. The window says:\n{}",
            ui.screen()
        );
        before = now;
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[track_caller]
pub fn wait_for_brightness(ui: &mut Harness, label: &str, looks_right: impl Fn(u8) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        ui.tick();
        let brightest = ui.brightest_in(label).ok();
        if brightest.is_some_and(&looks_right) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "\"{label}\" stays at brightness {brightest:?}"
        );
        std::thread::sleep(POLL);
    }
}

pub fn room_of(server: &TestServer, names: &[&str]) -> Vec<Window> {
    let mut windows: Vec<Window> = names
        .iter()
        .map(|name| {
            let mut ui = window(named(name));
            enter_general(&mut ui, &server.address(), name);
            ui
        })
        .collect();
    for (ui, me) in windows.iter_mut().zip(names) {
        for other in names.iter().filter(|other| *other != me) {
            wait_until(ui, &format!("{other} in {me}'s room"), |ui| ui.has(other));
        }
    }
    windows
}

pub struct TwoWithScreens {
    pub panda: Window,
    pub panda_screen: Arc<FakeScreen>,
    pub raccoon: Window,
    pub raccoon_screen: Arc<FakeScreen>,
    pub desktop: whisperia::wake::WakingSender<DesktopEvent>,
    pub commands: Receiver<DesktopCommand>,
    pub server: TestServer,
}

pub fn two_with_screens() -> TwoWithScreens {
    let server = TestServer::start("DESKTOP-6TNC");
    let (link, desktop, commands) = DesktopLink::channels(&whisperia::wake::Waker::default());
    let panda_screen = Arc::new(FakeScreen::new());
    let mut panda = opening(named("Panda"))
        .desktop(link)
        .screen(panda_screen.clone())
        .open();
    let raccoon_screen = Arc::new(FakeScreen::new());
    let mut raccoon = opening(named("Raccoon"))
        .screen(raccoon_screen.clone())
        .open();
    enter_general(&mut panda, &server.address(), "Panda");
    enter_general(&mut raccoon, &server.address(), "Raccoon");
    wait_until(&mut panda, "Raccoon in the room", |ui| ui.has("Raccoon"));
    wait_until(&mut raccoon, "Panda in the room", |ui| ui.has("Panda"));
    TwoWithScreens {
        panda,
        panda_screen,
        raccoon,
        raccoon_screen,
        desktop,
        commands,
        server,
    }
}

#[track_caller]
pub fn open_the_picker(ui: &mut Harness) {
    press(ui, "Share my screen");
    wait_for(ui, "[button] Screen 2 (off)");
    wait_until_gone(ui, "Finding screens and windows");
}

#[track_caller]
pub fn share(ui: &mut Harness, source: &str) {
    share_from(ui, "What to share: Screens", source);
}

#[track_caller]
pub fn share_window(ui: &mut Harness, source: &str) {
    share_from(ui, "What to share: Windows", source);
}

#[track_caller]
fn share_from(ui: &mut Harness, tab: &str, source: &str) {
    press(ui, "Share my screen");
    wait_for(ui, "Share your screen");
    press(ui, tab);
    wait_until(ui, &format!("{source} to choose from"), |ui| ui.has(source));
    press(ui, source);
    wait_until(ui, &format!("{source} chosen"), |ui| {
        ui.is_on(source) == Some(true)
    });
    press(ui, "Share");
    wait_for(ui, &format!("You are sharing {source}"));
}

pub fn room_with_a_share() -> TwoWithScreens {
    let mut two = two_with_screens();
    share(&mut two.panda, "Screen 1");
    wait_for(&mut two.raccoon, "[button] Watch what Panda shows");
    two
}

#[track_caller]
pub fn watch(watcher: &mut Harness, sharer: &str) {
    press(watcher, &format!("Watch what {sharer} shows"));
    wait_for(watcher, &format!("[button] Stop watching {sharer}"));
}
