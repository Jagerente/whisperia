mod chat;
mod outside;
#[cfg(windows)]
mod pictures;

#[allow(clippy::all, unused_qualifications, unreachable_pub, unsafe_code)]
mod ui {
    slint::include_modules!();
}

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use whisperia_proto::{
    control::{Avatar, Name, RoomLimit, RoomName, RoomSummary, MAX_CHAT_CHARS},
    PeerId, PROTOCOL_VERSION,
};

use self::outside::Outside;
use self::ui::{AppWindow, Ask, Do, Page, Pointer, Standing, State, Strings, Theme};
pub use crate::app::Options;
use crate::{
    app::{
        link::{self, Quality, Way},
        App, Choosing, Effect, Entry, InRoom, KeyRemark, Occupancy, Offered, Online, Prompt,
        Refusal, Stage, Status,
    },
    audio::DeviceList,
    config::{
        Appearance, HotkeyAction, Hotkeys, ShareConfig, SHARE_FRAME_RATES, SHARE_RESOLUTIONS,
    },
    dsp::{cue::Cue, denoise::DenoiseMode, level::SILENCE_DB},
    engine::{AudioTrouble, Side},
    screen::{policy, Preview, SourceKind},
};

const MY_ID: i32 = -1;
const METER_FLOOR_DB: f32 = -60.0;
const SYSTEM_DEFAULT: &str = "System default";
const SOUNDS: [(Cue, &str, &str); 11] = [
    (Cue::Join, "People", "Somebody joins the room"),
    (Cue::Leave, "", "Somebody leaves the room"),
    (Cue::Message, "", "A message arrives"),
    (Cue::Mute, "You", "Microphone muted"),
    (Cue::Unmute, "", "Microphone unmuted"),
    (Cue::Deafen, "", "Deafened"),
    (Cue::Undeafen, "", "Undeafened"),
    (Cue::ShareStart, "Screens", "A screen is shared"),
    (Cue::ShareStop, "", "A screen is no longer shared"),
    (Cue::Connected, "Connection", "You are in the room"),
    (Cue::Lost, "", "The connection is lost"),
];

pub fn run(options: Options) -> anyhow::Result<()> {
    let front = open(options, || {})?;
    run_until_quit(&front)
}

fn open(
    options: Options,
    on_picture_shown: impl Fn() + 'static,
) -> anyhow::Result<Rc<RefCell<Front>>> {
    let pressed_in = Rc::new(std::cell::OnceCell::new());
    let (options, device, theme) = select_renderer(options, pressed_in.clone());
    let front = Front::new(options, theme, Rc::new(outside::Desk))?;
    let _ = pressed_in.set(front.borrow().window.as_weak());
    #[cfg(windows)]
    if let Some(device) = device {
        let tiles = front.borrow().watching.clone();
        pictures::show_with(
            device,
            Box::new(move |sharer, picture| {
                on_picture_shown();
                for row in 0..tiles.row_count() {
                    if let Some(mut tile) =
                        tiles.row_data(row).filter(|t| t.sharer == sharer as i32)
                    {
                        tile.picture = picture.clone();
                        tile.has_picture = true;
                        tiles.set_row_data(row, tile);
                    }
                }
            }),
        );
    }
    #[cfg(not(windows))]
    let _ = (device, on_picture_shown);
    Ok(front)
}

fn wake(window: &slint::Weak<AppWindow>) {
    if let Some(window) = window.upgrade() {
        window.global::<Do>().invoke_wake();
    }
}

fn wake_soon(window: slint::Weak<AppWindow>) {
    slint::Timer::single_shot(Duration::ZERO, move || wake(&window));
}

fn run_until_quit(front: &Rc<RefCell<Front>>) -> anyhow::Result<()> {
    front.borrow().window.show()?;
    front.borrow_mut().tick();
    slint::run_event_loop_until_quit()?;
    Ok(())
}

type PressedIn = Rc<std::cell::OnceCell<slint::Weak<AppWindow>>>;

fn pointer_pressed(window: &AppWindow, x: f32, y: f32) {
    let pointer = window.global::<Pointer>();
    pointer.set_pressed_at(slint::LogicalPosition::new(x, y));
    pointer.set_presses(pointer.get_presses().wrapping_add(1));
}

#[cfg(windows)]
fn select_renderer(
    mut options: Options,
    pressed_in: PressedIn,
) -> (Options, Option<slint::wgpu_30::wgpu::Device>, SystemTheme) {
    use slint::wgpu_30::{wgpu, WGPUConfiguration};

    let theme = SystemTheme::default();
    let events = || WindowEvents {
        theme: theme.clone(),
        waker: options.waker.clone(),
        pressed_in: pressed_in.clone(),
        pointer: slint::winit_030::winit::dpi::PhysicalPosition::default(),
    };
    let made = (|| -> anyhow::Result<(wgpu::Device, whisperia_winmedia::AdapterLuid)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            flags: wgpu::InstanceFlags::from_build_config(),
            backend_options: wgpu::BackendOptions::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("whisperia"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::default(),
            }))?;
        let adapter_luid = whisperia_winmedia::adapter_luid(&device)?;
        slint::BackendSelector::new()
            .require_wgpu_30(WGPUConfiguration::Manual {
                instance,
                adapter,
                device: device.clone(),
                queue,
            })
            .with_winit_custom_application_handler(events())
            .select()?;
        Ok((device, adapter_luid))
    })();
    match made {
        Ok((device, adapter_luid)) => {
            options.screen = Arc::new(pictures::InWindow {
                inner: options.screen.clone(),
                adapter_luid,
            });
            (options, Some(device), theme)
        }
        Err(e) => {
            tracing::warn!("shares cannot be watched on this PC: {e:#}");
            if let Err(e) = slint::BackendSelector::new()
                .with_winit_custom_application_handler(events())
                .select()
            {
                tracing::warn!("hotkeys will not work while the window is in front: {e}");
            }
            (options, None, theme)
        }
    }
}

#[derive(Clone, Default)]
struct SystemTheme(Arc<AtomicU8>);

impl SystemTheme {
    const UNKNOWN: u8 = 0;
    const LIGHT: u8 = 1;
    const DARK: u8 = 2;

    fn scheme(&self) -> slint::language::ColorScheme {
        match self.0.load(Ordering::Relaxed) {
            Self::DARK => slint::language::ColorScheme::Dark,
            _ => slint::language::ColorScheme::Light,
        }
    }

    #[cfg(windows)]
    fn heard(&self, dark: bool) -> bool {
        let now = if dark { Self::DARK } else { Self::LIGHT };
        self.0.swap(now, Ordering::Relaxed) != now
    }

    #[cfg(windows)]
    fn is_known(&self) -> bool {
        self.0.load(Ordering::Relaxed) != Self::UNKNOWN
    }
}

#[cfg(windows)]
struct WindowEvents {
    theme: SystemTheme,
    waker: crate::wake::Waker,
    pressed_in: PressedIn,
    pointer: slint::winit_030::winit::dpi::PhysicalPosition<f64>,
}

#[cfg(windows)]
impl slint::winit_030::CustomApplicationHandler for WindowEvents {
    fn resumed(
        &mut self,
        event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
    ) -> slint::winit_030::EventResult {
        use slint::winit_030::winit::event_loop::DeviceEvents;
        event_loop.listen_device_events(DeviceEvents::Never);
        slint::winit_030::EventResult::Propagate
    }

    fn window_event(
        &mut self,
        _event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
        _window_id: slint::winit_030::winit::window::WindowId,
        winit_window: Option<&slint::winit_030::winit::window::Window>,
        _slint_window: Option<&slint::Window>,
        event: &slint::winit_030::winit::event::WindowEvent,
    ) -> slint::winit_030::EventResult {
        use slint::winit_030::winit::{
            event::{ElementState, WindowEvent},
            window::Theme,
        };
        match event {
            WindowEvent::CursorMoved { position, .. } => self.pointer = *position,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                let scale = winit_window.map_or(1.0, |window| window.scale_factor());
                let at = self.pointer.to_logical::<f32>(scale);
                if let Some(window) = self.pressed_in.get().and_then(slint::Weak::upgrade) {
                    pointer_pressed(&window, at.x, at.y);
                }
            }
            _ => {}
        }
        let told = match event {
            WindowEvent::ThemeChanged(theme) => Some(*theme),
            _ if !self.theme.is_known() => winit_window.and_then(|window| window.theme()),
            _ => None,
        };
        if let Some(theme) = told {
            if self.theme.heard(theme == Theme::Dark) {
                self.waker.wake();
            }
        }
        slint::winit_030::EventResult::Propagate
    }
}

#[cfg(not(windows))]
fn select_renderer(options: Options, _: PressedIn) -> (Options, Option<()>, SystemTheme) {
    (options, None, SystemTheme::default())
}

struct Front {
    core: App,
    window: AppWindow,
    entries: Vec<Entry>,
    devices: DeviceList,
    devices_news: u64,
    looking: Looking,
    settings: Option<ui::SettingsPart>,
    full_screen: bool,
    servers: Rc<VecModel<ui::ServerEntry>>,
    rooms: Rc<VecModel<ui::RoomEntry>>,
    room_guests: Rc<VecModel<ui::Guest>>,
    people: Rc<VecModel<ui::Person>>,
    chat: Rc<VecModel<ui::ChatLine>>,
    lines: RefCell<chat::Lines>,
    outside: Rc<dyn Outside>,
    watching: Rc<VecModel<ui::Tile>>,
    shelf: Rc<VecModel<ui::Tile>>,
    audio_troubles: Rc<VecModel<ui::Trouble>>,
    screens_offered: Rc<VecModel<ui::Source>>,
    windows_offered: Rc<VecModel<ui::Source>>,
    ping_graph: Rc<VecModel<ui::PingPoint>>,
    microphones: Rc<VecModel<SharedString>>,
    speakers: Rc<VecModel<SharedString>>,
    each_sound: Rc<VecModel<ui::Sound>>,
    keybinds: Rc<VecModel<ui::Keybind>>,
    system_theme: SystemTheme,
    scheme: Option<slint::language::ColorScheme>,
    next_tick: slint::Timer,
    quitting: bool,
}

impl Front {
    fn new(
        options: Options,
        system_theme: SystemTheme,
        outside: Rc<dyn Outside>,
    ) -> anyhow::Result<Rc<RefCell<Self>>> {
        let waker = options.waker.clone();
        let core = App::new(options)?;
        let window = AppWindow::new()?;
        let reachable = window.as_weak();
        waker.attach(move || {
            let _ = reachable.upgrade_in_event_loop(|window| window.global::<Do>().invoke_wake());
        });
        let front = Rc::new(RefCell::new(Self {
            core,
            window,
            quitting: false,
            entries: Vec::new(),
            devices: DeviceList::default(),
            devices_news: 0,
            looking: Looking::default(),
            settings: None,
            full_screen: false,
            servers: Rc::default(),
            rooms: Rc::default(),
            room_guests: Rc::default(),
            people: Rc::default(),
            chat: Rc::default(),
            lines: RefCell::default(),
            outside,
            watching: Rc::default(),
            shelf: Rc::default(),
            audio_troubles: Rc::default(),
            screens_offered: Rc::default(),
            windows_offered: Rc::default(),
            ping_graph: Rc::default(),
            microphones: Rc::default(),
            speakers: Rc::default(),
            each_sound: Rc::default(),
            keybinds: Rc::default(),
            system_theme,
            scheme: None,
            next_tick: slint::Timer::default(),
        }));
        front.borrow_mut().apply_appearance();
        {
            let this = front.borrow();
            let state = this.window.global::<State>();
            state.set_version(env!("CARGO_PKG_VERSION").into());
            state.set_source_link(env!("CARGO_PKG_REPOSITORY").into());
            state.set_chat_limit(MAX_CHAT_CHARS as i32);
            state.set_servers(ModelRc::from(this.servers.clone()));
            state.set_rooms(ModelRc::from(this.rooms.clone()));
            state.set_room_guests(ModelRc::from(this.room_guests.clone()));
            state.set_fewest_seats(RoomLimit::FEWEST.into());
            state.set_most_seats(RoomLimit::MOST.into());
            state.set_people(ModelRc::from(this.people.clone()));
            state.set_chat(ModelRc::from(this.chat.clone()));
            state.set_watching(ModelRc::from(this.watching.clone()));
            state.set_shelf(ModelRc::from(this.shelf.clone()));
            state.set_audio_troubles(ModelRc::from(this.audio_troubles.clone()));
            state.set_screens_offered(ModelRc::from(this.screens_offered.clone()));
            state.set_windows_offered(ModelRc::from(this.windows_offered.clone()));
            state.set_ping_graph(ModelRc::from(this.ping_graph.clone()));
            state.set_microphones(ModelRc::from(this.microphones.clone()));
            state.set_speakers(ModelRc::from(this.speakers.clone()));
            state.set_each_sound(ModelRc::from(this.each_sound.clone()));
            state.set_keybinds(ModelRc::from(this.keybinds.clone()));
            state.set_resolutions(strings(
                SHARE_RESOLUTIONS
                    .iter()
                    .map(|lines| ShareConfig::resolution_name(*lines)),
            ));
            state.set_frame_rates(strings(SHARE_FRAME_RATES.iter().map(|fps| fps.to_string())));
            let animals: Vec<ui::Animal> = Avatar::ALL
                .iter()
                .map(|animal| ui::Animal {
                    glyph: glyph(*animal).into(),
                    label: animal_label(*animal).into(),
                })
                .collect();
            state.set_animals(ModelRc::from(animals.as_slice()));
        }
        Self::wire(&front);
        front.borrow_mut().refresh();
        Ok(front)
    }

    fn wire(front: &Rc<RefCell<Self>>) {
        let window = front.borrow().window.clone_strong();
        let actions = window.global::<Do>();
        macro_rules! on {
            ($setter:ident, |$this:ident $(, $arg:ident)*| $body:expr) => {{
                let front = front.clone();
                actions.$setter(move |$($arg),*| {
                    let mut guard = front.borrow_mut();
                    let $this = &mut *guard;
                    $body;
                    $this.refresh();
                    $this.schedule_tick(Instant::now());
                });
            }};
        }

        {
            let front = front.clone();
            window.window().on_close_requested(move || {
                front.borrow_mut().core.window_closed();
                slint::CloseRequestResponse::HideWindow
            });
        }
        on!(on_connect_to, |this, index| {
            if let Some(entry) = this.entries.get(index as usize).cloned() {
                this.core.connect_entry(&entry);
            }
        });
        on!(on_forget, |this, index| {
            if let Some(recent) = this
                .entries
                .get(index as usize)
                .and_then(|entry| entry.recent.clone())
            {
                this.core.forget_recent(&recent);
            }
        });
        on!(on_connect_address, |this, address| {
            if !address.trim().is_empty() {
                this.core.connect_address(&address);
            }
        });
        on!(on_cancel_connecting, |this| this.core.cancel_connect());
        on!(on_disconnect, |this| this.core.disconnect());
        on!(on_look_into, |this, index| {
            this.looking = usize::try_from(index)
                .ok()
                .and_then(|row| this.rooms.row_data(row))
                .and_then(|room| RoomName::new(&room.name).ok())
                .map_or(Looking::NewRoom, Looking::Room);
        });
        on!(on_look_into_new_room, |this| this.looking =
            Looking::NewRoom);
        on!(on_enter_room, |this, name, password| this
            .core
            .enter_room(&name, Some(&password)));
        on!(on_open_room, |this, name, password, seats| this
            .core
            .open_room(&name, Some(&password), limit_of(seats)));
        on!(on_set_room_password, |this, password| this
            .core
            .set_room_password(Some(&password)));
        on!(on_remove_room_password, |this| this
            .core
            .set_room_password(None));
        on!(on_set_room_seats, |this, seats| this
            .core
            .set_room_limit(limit_of(seats)));
        on!(on_make_host, |this, id| this.core.make_host(id as PeerId));
        on!(on_kick, |this, id| this.core.kick(id as PeerId));
        on!(on_leave_room, |this| {
            this.set_full_screen(false);
            this.core.leave_room();
        });
        on!(on_toggle_mute, |this| this.core.toggle_mute());
        on!(on_toggle_deafen, |this| this.core.toggle_deafen());
        on!(on_toggle_share, |this| this.core.toggle_share());
        on!(on_pick_share_tab, |this, tab| this
            .core
            .show_share_tab(source_kind_of(tab)));
        on!(on_pick_source, |this, id| this.core.pick_source(id));
        on!(on_share_source, |this, id| this.core.share_source(id));
        on!(on_change_share, |this| this.core.change_share());
        on!(on_cancel_sharing, |this| this.core.cancel_choosing());
        on!(on_toggle_watch, |this, id| this
            .core
            .toggle_watch(id as PeerId));
        on!(on_stop_watching, |this| this.core.stop_watching());
        on!(on_say, |this, text| this.core.send_chat(&text));
        on!(on_dismiss_notice, |this| this.core.clear_notice());

        on!(on_set_volume, |this, percent| this
            .core
            .set_volume(crate::dsp::percent_to_volume(percent)));
        on!(on_set_person_volume, |this, id, db| this
            .core
            .set_person_volume(id as PeerId, db));
        on!(on_set_screen_volume, |this, id, db| this
            .core
            .set_screen_volume(id as PeerId, db));
        on!(on_toggle_screen_mute, |this, id| this
            .core
            .toggle_screen_mute(id as PeerId));
        on!(on_volumes_settled, |this| this.core.save_config());

        on!(on_open_settings, |this, part| this.open_settings(part));
        on!(on_turn_settings, |this, part| {
            this.core.stop_mic_test();
            this.core.cancel_hotkey_capture();
            this.settings = Some(part);
        });
        on!(on_audio_panel_closed, |this| this.core.stop_mic_test());
        on!(on_close_settings, |this| {
            this.settings = None;
            this.core.stop_mic_test();
            this.core.cancel_hotkey_capture();
            this.core.save_config();
        });
        on!(on_pick_microphone, |this, index| {
            let input = device_at(&this.devices.inputs, index);
            let output = this.core.config.output_device.clone();
            this.core.apply_devices(input, output);
        });
        on!(on_pick_speaker, |this, index| {
            let input = this.core.config.input_device.clone();
            let output = device_at(&this.devices.outputs, index);
            this.core.apply_devices(input, output);
        });
        on!(on_toggle_test, |this| {
            if this.core.is_testing_microphone() {
                this.core.stop_mic_test();
            } else {
                this.core.start_mic_test();
            }
        });
        on!(on_pick_noise, |this, noise| this
            .core
            .set_denoise(denoise_of(noise)));
        on!(on_toggle_auto_level, |this| this.core.toggle_agc());
        on!(on_set_mic_gain, |this, db| this.core.set_mic_gain(db));
        on!(on_pick_resolution, |this, index| {
            if let Some(lines) = SHARE_RESOLUTIONS.get(index as usize) {
                let fps = this.core.config.share.fps;
                this.core.set_stream_quality(*lines, fps);
            }
        });
        on!(on_pick_frame_rate, |this, index| {
            if let Some(fps) = SHARE_FRAME_RATES.get(index as usize) {
                let lines = this.core.config.share.max_height;
                this.core.set_stream_quality(lines, *fps);
            }
        });
        on!(on_toggle_sounds, |this| this.core.toggle_sounds());
        on!(on_toggle_sound, |this, index| {
            if let Some((cue, ..)) = SOUNDS.get(index as usize) {
                this.core.toggle_notification(*cue);
            }
        });
        on!(on_play_sound, |this, index| {
            if let Some((cue, ..)) = SOUNDS.get(index as usize) {
                this.core.play_cue(*cue);
            }
        });
        on!(on_ask_key, |this, index| {
            if let Some(action) = HotkeyAction::ALL.get(index as usize) {
                this.core.capture_hotkey(*action);
            }
        });
        on!(on_remove_key, |this, index| {
            if let Some(action) = HotkeyAction::ALL.get(index as usize) {
                this.core.clear_hotkey(*action);
            }
        });
        on!(on_reset_key, |this, index| {
            if let Some(action) = HotkeyAction::ALL.get(index as usize) {
                this.core.reset_hotkey(*action);
            }
        });
        on!(on_reset_keys, |this| this.core.reset_hotkeys());
        on!(on_pick_close_to_tray, |this, on| this
            .core
            .set_close_to_tray(on));
        on!(on_pick_appearance, |this, appearance| this
            .core
            .set_appearance(appearance_of(appearance)));
        on!(on_rename, |this, name| {
            if let Ok(name) = Name::new(&name) {
                this.core.set_name(name);
            }
        });
        on!(on_pick_animal, |this, index| {
            if let Some(animal) = Avatar::ALL.get(index as usize) {
                this.core.set_avatar(*animal);
            }
        });

        on!(on_answer_trust, |this, accept| this
            .core
            .answer_trust(accept));
        on!(on_answer_password, |this, password| this
            .core
            .answer_password(Some(&password)));
        on!(on_refuse_password, |this| this.core.answer_password(None));
        on!(on_close_version, |this| this.core.close_version_prompt());
        on!(on_set_full_screen, |this, on| this.set_full_screen(on));

        on!(on_open_link, |this, url| this.core.open_link(&url));
        on!(on_hide_preview, |this, line, url| this
            .core
            .hide_preview(u64::try_from(line).unwrap_or_default(), &url));
        on!(on_copy, |this, text| this.core.copy(&text));
        {
            let front = front.clone();
            let window = window.as_weak();
            actions.on_wake(move || match front.try_borrow_mut() {
                Ok(mut front) => front.tick(),
                Err(_) => wake_soon(window.clone()),
            });
        }

        actions.on_face_of(|typed, animal| {
            let name = Name::new(&typed).map(String::from).unwrap_or_default();
            ui::Face {
                initials: initial(&name).into(),
                tint: tint(&name),
                name: name.into(),
                animal,
            }
        });
        actions.on_loudness(|db| loudness(db).into());
        actions.on_percentage(|percent| format!("{percent:.0} %").into());
        window.global::<Strings>().on_first(|text, count| {
            let count = usize::try_from(count).unwrap_or_default();
            text.chars().take(count).collect::<String>().into()
        });
        window.global::<Strings>().on_position(|choices, text| {
            choices
                .iter()
                .position(|choice| choice == text)
                .map_or(-1, |at| at as i32)
        });
    }

    fn set_full_screen(&mut self, on: bool) {
        self.full_screen = on;
        self.window.window().set_fullscreen(on);
        self.window.global::<State>().set_full_screen(on);
    }

    fn open_settings(&mut self, part: ui::SettingsPart) {
        self.settings = Some(part);
        self.read_devices();
    }

    fn read_devices(&mut self) {
        self.devices = self.core.devices();
        self.devices_news = self.core.devices_news();
    }

    fn schedule_tick(&self, now: Instant) {
        match self.core.next_tick(now) {
            Some(at) => {
                let window = self.window.as_weak();
                self.next_tick.start(
                    slint::TimerMode::SingleShot,
                    at.saturating_duration_since(now),
                    move || wake(&window),
                );
            }
            None => self.next_tick.stop(),
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        self.core.tick(now);
        let mut window_state_changed = false;
        for effect in self.core.take_effects() {
            match effect {
                Effect::ShowWindow => self.bring_up(),
                Effect::OpenSettings => {
                    self.open_settings(ui::SettingsPart::General);
                    window_state_changed = true;
                }
                Effect::LookAtRoom(room) => {
                    self.looking = Looking::Room(room);
                    window_state_changed = true;
                }
                Effect::Open(url) => self.outside.open(&url),
                Effect::Copy(text) => self.outside.copy(&text),
                Effect::Quit => {
                    self.core.say_goodbye();
                    self.quitting = true;
                    let _ = slint::quit_event_loop();
                }
            }
        }
        self.apply_appearance();
        if self.core.take_changed() || window_state_changed {
            self.refresh();
        } else {
            self.refresh_live();
        }
        self.schedule_tick(now);
    }

    fn apply_appearance(&mut self) {
        let wanted = match self.core.config.appearance {
            Appearance::System => self.system_theme.scheme(),
            Appearance::Light => slint::language::ColorScheme::Light,
            Appearance::Dark => slint::language::ColorScheme::Dark,
        };
        if self.scheme == Some(wanted) {
            return;
        }
        self.scheme = Some(wanted);
        self.window.global::<Theme>().set_scheme(wanted);
    }

    fn bring_up(&self) {
        let _ = self.window.show();
        #[cfg(windows)]
        {
            use slint::winit_030::WinitWindowAccessor;
            self.window.window().with_winit_window(|window| {
                window.set_minimized(false);
                window.focus_window();
            });
        }
    }

    fn refresh(&mut self) {
        self.apply_appearance();
        self.settle_full_screen();
        let state = self.window.global::<State>();
        let core = &self.core;
        let config = &core.config;

        state.set_me(face(&config.name, config.avatar));
        state.set_animal(
            Avatar::ALL
                .iter()
                .position(|animal| *animal == config.avatar)
                .unwrap_or(0) as i32,
        );
        state.set_name_unknown(!config.name_confirmed);
        match &core.notice {
            Some(notice) => {
                state.set_notice(notice.text.as_str().into());
                state.set_notice_is_error(notice.is_error);
            }
            None => state.set_notice(SharedString::new()),
        }
        state.set_has_tray(core.has_tray());

        match &core.prompt {
            None => state.set_ask(Ask::Nothing),
            Some(Prompt::Trust {
                target,
                fingerprint,
            }) => {
                state.set_ask(Ask::Trust);
                state.set_ask_server(target.label.as_str().into());
                state.set_ask_fingerprint(fingerprint.short().into());
            }
            Some(Prompt::Password { target, wrong }) => {
                state.set_ask(Ask::Password);
                state.set_ask_server(target.label.as_str().into());
                state.set_password_was_wrong(*wrong);
            }
            Some(Prompt::OtherVersion { server, theirs }) => {
                state.set_ask(Ask::OtherVersion);
                state.set_ask_server(server.as_str().into());
                state.set_ask_says(version_in_words(server, *theirs).into());
            }
        }

        state.set_page(match &core.stage {
            Stage::Offline { .. } => Page::Servers,
            Stage::Connecting(_) => Page::Connecting,
            Stage::Online(online) if online.room.is_some() => Page::Room,
            Stage::Online(_) => Page::Rooms,
        });
        match &core.stage {
            Stage::Offline { address } => {
                state.set_address_to_correct(address.as_deref().unwrap_or_default().into());
                let (refused, why) = match &core.refused {
                    Some(refused) => (refused.address.as_str(), sentence(&refused.why.to_string())),
                    None => ("", String::new()),
                };
                state.set_refused_address(refused.into());
                state.set_refused_why(why.into());
                state
                    .set_search_trouble(core.discovery_error.as_deref().unwrap_or_default().into());
                self.entries = core.entries();
                sync(
                    &self.servers,
                    self.entries.iter().map(server_entry).collect(),
                );
            }
            Stage::Connecting(connecting) => {
                state.set_connecting_to(connecting.target.label.as_str().into());
                let (attempt, reason) = match &connecting.retry {
                    Some(retry) => (retry.attempt as i32, retry.reason.as_str()),
                    None => (0, ""),
                };
                state.set_attempt(attempt);
                state.set_retry_reason(sentence(reason).into());
                let (muted, deafened) = core.voice_kept().unwrap_or_default();
                state.set_muted(muted);
                state.set_deafened(deafened);
            }
            Stage::Online(online) => {
                state.set_server_name(online.welcome.server_name.as_str().into());
                state.set_server_address(online.welcome.addr.to_string().into());
                let joining = online.joining.as_ref().map(|joining| joining.room.as_str());
                state.set_joining(joining.unwrap_or_default().into());
                let (locked_out, why) =
                    online
                        .locked_out
                        .as_ref()
                        .map_or(("", ui::Refusal::Nothing), |out| {
                            let why = match out.why {
                                Refusal::Password => ui::Refusal::Password,
                                Refusal::WrongPassword => ui::Refusal::WrongPassword,
                                Refusal::Full => ui::Refusal::Full,
                            };
                            (out.room.as_str(), why)
                        });
                state.set_locked_out(locked_out.into());
                state.set_refusal(why);
                sync(&self.rooms, online.rooms.iter().map(room_entry).collect());
                match &online.room {
                    Some(room) => self.show_room(online, room),
                    None => {
                        let looked = self.looking.settle(&online.rooms);
                        state.set_looking_at_new_room(looked.is_none());
                        state.set_looked_room(looked.map_or(0, |at| at as i32));
                        let guests = looked
                            .and_then(|at| online.rooms.get(at))
                            .into_iter()
                            .flat_map(|room| &room.people)
                            .map(|person| ui::Guest {
                                face: face(&person.name, person.avatar),
                                host: person.host,
                            });
                        sync(&self.room_guests, guests.collect());
                    }
                }
            }
        }
        if state.get_page() != Page::Rooms {
            self.looking = Looking::default();
        }
        if core.room().is_none() {
            sync(&self.watching, Vec::new());
            sync(&self.shelf, Vec::new());
            sync(&self.audio_troubles, Vec::new());
            self.show_sources(None);
        }
        if self.settings.is_some() && self.core.devices_news() != self.devices_news {
            self.read_devices();
        }
        self.show_settings();
    }

    fn settle_full_screen(&mut self) {
        let watching = self
            .core
            .room()
            .is_some_and(|room| room.watched().is_some());
        if self.full_screen && !watching {
            self.set_full_screen(false);
        }
    }

    fn refresh_live(&self) {
        if let Stage::Online(online) = &self.core.stage {
            if let Some(room) = &online.room {
                self.show_people(online, room);
                self.show_connection(online);
            }
        }
        self.show_meters();
    }

    fn show_people(&self, online: &Online, room: &InRoom) {
        let config = &self.core.config;
        let telemetry = self.core.engine.telemetry();
        let heard = telemetry.peers();
        let mic_talking = telemetry.mic_talking.load(Ordering::Relaxed);
        let testing = self.core.is_testing_microphone();
        let silenced = online.muted || online.deafened || testing;
        let watches_me = |id: PeerId| {
            room.share
                .as_ref()
                .is_some_and(|share| share.viewers.contains(&id))
        };
        let me = ui::Person {
            id: MY_ID,
            face: face(&online.name, config.avatar),
            me: true,
            host: room.is_host(),
            talking: mic_talking && !silenced,
            muted: silenced,
            deafened: online.deafened || testing,
            showing: room.share.is_some(),
            ..Default::default()
        };
        let others = room.peers.iter().map(|peer| {
            let t = heard.iter().find(|t| t.id == peer.id);
            ui::Person {
                id: peer.id as i32,
                face: face(&peer.name, peer.avatar),
                me: false,
                host: peer.id == room.host,
                talking: t.is_some_and(|t| t.talking),
                muted: peer.state.muted,
                deafened: peer.state.deafened,
                showing: peer.share.is_some(),
                watched: room.is_watching(peer.id),
                watches_me: watches_me(peer.id),
                screen_has_sound: peer.share.is_some_and(|share| share.sound),
                volume: config.person_volume(&peer.name),
                screen_volume: config.person_screen_volume_heard(&peer.name),
            }
        });
        sync(&self.people, std::iter::once(me).chain(others).collect());
    }

    fn show_room(&self, online: &Online, room: &InRoom) {
        let state = self.window.global::<State>();
        let config = &self.core.config;

        state.set_room_name(room.name.as_str().into());
        state.set_hosting(room.is_host());
        state.set_room_locked(room.locked);
        state.set_room_seats(seats_of(room.shown_limit()));
        state.set_muted(online.muted);
        state.set_deafened(online.deafened);
        sync(
            &self.audio_troubles,
            self.core.audio_troubles().map(trouble_in_words).collect(),
        );
        state.set_showing(room.share.is_some());
        self.show_connection(online);
        self.show_sources(room.choosing.as_ref());
        self.show_people(online, room);

        state.set_chat_heard(i32::try_from(room.chat.heard()).unwrap_or(i32::MAX));
        let rows = self.lines.borrow_mut().rows(
            &room.chat,
            |id| room.peers.iter().any(|peer| peer.id == id),
            |url| self.core.preview_of(url),
        );
        sync(&self.chat, rows);

        let watched = room.watched();
        let picture = watched.and_then(|id| {
            let peer = room.peers.iter().find(|peer| peer.id == id)?;
            let picture = current_picture(id);
            let volume = config.person_screen_volume_heard(&peer.name);
            Some(ui::Tile {
                sharer: id as i32,
                face: face(&peer.name, room.avatar_of(id)),
                shows: ui::Showing::Picture,
                has_picture: picture.is_some(),
                picture: picture.unwrap_or_default(),
                has_sound: peer.share.is_some_and(|share| share.sound),
                minimized: peer.share.is_some_and(|share| share.minimized),
                volume,
                silent: crate::dsp::is_silent(volume),
                ..Default::default()
            })
        });
        sync(&self.watching, picture.into_iter().collect());

        let mut shelf = Vec::new();
        if !self.full_screen {
            for peer in &room.peers {
                if peer.share.is_none() || Some(peer.id) == watched {
                    continue;
                }
                shelf.push(ui::Tile {
                    sharer: peer.id as i32,
                    face: face(&peer.name, peer.avatar),
                    shows: ui::Showing::Offered,
                    minimized: peer.share.is_some_and(|share| share.minimized),
                    ..Default::default()
                });
            }
            for ending in &room.ending {
                shelf.push(ui::Tile {
                    sharer: ending.sharer as i32,
                    face: face(&ending.name, ending.avatar),
                    shows: ui::Showing::Ended,
                    ..Default::default()
                });
            }
            if let Some(share) = &room.share {
                let stream = share.handle.stream();
                shelf.push(ui::Tile {
                    sharer: MY_ID,
                    face: face(&online.name, config.avatar),
                    shows: ui::Showing::Mine,
                    label: share.label.as_str().into(),
                    detail: format!(
                        "{}, {}, {} fps",
                        share.label,
                        policy::screen_detail(stream.size),
                        stream.fps
                    )
                    .into(),
                    minimized: share.info.minimized,
                    ..Default::default()
                });
            }
        }
        sync(&self.shelf, shelf);
    }

    fn show_connection(&self, online: &Online) {
        let state = self.window.global::<State>();
        let link = &online.link;
        let quality = link.quality();
        state.set_quality(graded(quality));
        state.set_connection(quality.word().into());
        state.set_ping(
            link.latest()
                .map(|look| delay_in_words(look.rtt_ms))
                .unwrap_or_default()
                .into(),
        );
        state.set_ping_quality(graded(
            link.latest()
                .map_or(quality, |look| Quality::of_delay(look.rtt_ms)),
        ));
        let loss = link.loss();
        state.set_loss(loss_in_words(loss.worst()).into());
        state.set_loss_caption(
            match loss.way() {
                None => "Lost in the last 30 seconds",
                Some(Way::Out) => "Lost on the way out",
                Some(Way::In) => "Lost on the way in",
            }
            .into(),
        );
        state.set_loss_quality(graded(Quality::of_loss(loss.worst())));
        state.set_server_details(
            format!(
                "{}, {}, certificate {}",
                online.welcome.server_name,
                online.welcome.addr,
                online.welcome.fingerprint.short()
            )
            .into(),
        );

        let top = link.top_ms();
        state.set_ping_top(delay_in_words(top).into());
        state.set_ping_middle(delay_in_words(top / 2.0).into());

        let waiting = link::KEEP.saturating_sub(link.delays().len());
        let points = std::iter::repeat_n(ui::PingPoint::default(), waiting).chain(
            link.delays()
                .zip(link.losses())
                .map(|(ms, lost)| ui::PingPoint {
                    height: (ms / top).clamp(0.04, 1.0),
                    quality: graded(Quality::of_delay(ms).max(Quality::of_loss(lost.worst()))),
                }),
        );
        sync(&self.ping_graph, points.collect());
    }

    fn show_meters(&self) {
        let state = self.window.global::<State>();
        let hearing = self.core.is_testing_microphone() || self.core.room().is_some();
        let telemetry = self.core.engine.telemetry();
        let (mic_db, agc_gain) = match hearing {
            true => (telemetry.mic_db.get(), telemetry.agc_gain_db.get()),
            false => (SILENCE_DB, 0.0),
        };
        state.set_mic_level(along_the_meter(mic_db));
        state.set_auto_level_gain(agc_gain.round());
    }

    fn show_sources(&self, choosing: Option<&Choosing>) {
        let state = self.window.global::<State>();
        state.set_choosing(choosing.is_some());
        let Some(choosing) = choosing else {
            sync(&self.screens_offered, Vec::new());
            sync(&self.windows_offered, Vec::new());
            return;
        };
        state.set_listing(!choosing.is_complete());
        state.set_share_tab(share_tab_of(choosing.tab()));
        state.set_source_chosen(choosing.chosen().is_some());
        state.set_chosen_source(choosing.chosen().unwrap_or_default());
        offer(&self.screens_offered, choosing.offered(SourceKind::Screen));
        offer(&self.windows_offered, choosing.offered(SourceKind::Window));
    }

    fn show_settings(&self) {
        let state = self.window.global::<State>();
        let core = &self.core;
        let config = &core.config;
        let listed = |devices: &[String]| -> Vec<SharedString> {
            std::iter::once(SYSTEM_DEFAULT.into())
                .chain(devices.iter().map(|device| device.as_str().into()))
                .collect()
        };
        let chosen = |devices: &[String], wanted: &Option<String>| {
            wanted
                .as_ref()
                .and_then(|wanted| devices.iter().position(|device| device == wanted))
                .map_or(0, |at| at as i32 + 1)
        };
        sync(&self.microphones, listed(&self.devices.inputs));
        sync(&self.speakers, listed(&self.devices.outputs));
        state.set_microphone(chosen(&self.devices.inputs, &config.input_device));
        state.set_speaker(chosen(&self.devices.outputs, &config.output_device));

        let testing = core.is_testing_microphone();
        state.set_testing(testing);
        let trouble = core.audio_troubles().next().filter(|_| testing);
        state.set_test_trouble(
            trouble
                .map(|t| trouble_in_words(t).title)
                .unwrap_or_default(),
        );
        self.show_meters();

        state.set_settings_open(self.settings.is_some());
        state.set_settings_part(self.settings.unwrap_or_default());
        state.set_noise(noise_of(config.denoise));
        let (credit, link) = credit_for(config.denoise);
        state.set_noise_credit(credit.into());
        state.set_noise_link(link.into());
        state.set_auto_level(config.agc);
        state.set_mic_gain(config.mic_gain_db);
        state.set_volume_percent(crate::dsp::volume_to_percent(config.volume_db));
        state.set_sounds(config.sounds);
        let sounds = SOUNDS.iter().map(|(cue, group, name)| ui::Sound {
            group: (*group).into(),
            name: (*name).into(),
            on: config.notifications.allows(*cue),
        });
        sync(&self.each_sound, sounds.collect());
        let usual = Hotkeys::default();
        let remark = |action: HotkeyAction| match &core.key_remark {
            Some(KeyRemark::Refused { action: at, why }) if *at == action => {
                ((*why).to_owned(), true)
            }
            Some(KeyRemark::Taken { action: at, keys }) if *at == action => (
                format!(
                    "{} was the key for \"{}\", which has none now",
                    keys_in_words(Some(*keys)),
                    action.name()
                ),
                false,
            ),
            _ => (String::new(), false),
        };
        let keybinds = HotkeyAction::ALL.iter().map(|action| {
            let (remark, refused) = remark(*action);
            ui::Keybind {
                name: action.name().into(),
                keys: keys_in_words(config.hotkeys.get(*action)).into(),
                default_keys: keys_in_words(usual.get(*action)).into(),
                asking: core.capturing == Some(*action),
                remark: remark.into(),
                refused,
            }
        });
        sync(&self.keybinds, keybinds.collect());
        state.set_keybinds_are_default(config.hotkeys == usual);
        state.set_close_to_tray(config.close_to_tray);
        state.set_appearance(ui_appearance_of(config.appearance));
        let preset = |presets: &[u32], value: u32| {
            presets
                .iter()
                .position(|preset| *preset == value)
                .map_or(-1, |at| at as i32)
        };
        state.set_resolution(preset(&SHARE_RESOLUTIONS, config.share.max_height));
        state.set_frame_rate(preset(&SHARE_FRAME_RATES, config.share.fps));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum Looking {
    #[default]
    Undecided,
    Room(RoomName),
    NewRoom,
}

impl Looking {
    fn settle(&mut self, rooms: &[RoomSummary]) -> Option<usize> {
        if let Looking::Room(name) = self {
            if let Some(at) = rooms.iter().position(|room| room.name == *name) {
                return Some(at);
            }
        }
        if *self != Looking::NewRoom {
            *self = rooms
                .first()
                .map_or(Looking::NewRoom, |room| Looking::Room(room.name.clone()));
        }
        (*self != Looking::NewRoom).then_some(0)
    }
}

fn people_in_words(count: usize, limit: Option<RoomLimit>) -> String {
    match (count, limit) {
        (count, Some(limit)) => format!("{count} of {limit} people"),
        (1, None) => "1 person".to_owned(),
        (count, None) => format!("{count} people"),
    }
}

fn room_entry(room: &RoomSummary) -> ui::RoomEntry {
    let count = room.people.len();
    ui::RoomEntry {
        name: room.name.as_str().into(),
        tally: match room.limit {
            Some(limit) => format!("{count}/{limit}"),
            None => count.to_string(),
        }
        .into(),
        people: people_in_words(count, room.limit).into(),
        locked: room.locked,
        full: room.is_full(),
    }
}

fn limit_of(seats: ui::Seats) -> Option<RoomLimit> {
    (!seats.any).then(|| {
        let most = seats
            .most
            .clamp(RoomLimit::FEWEST.into(), RoomLimit::MOST.into());
        RoomLimit::new(u8::try_from(most).unwrap_or(RoomLimit::MOST))
    })
}

fn seats_of(limit: Option<RoomLimit>) -> ui::Seats {
    match limit {
        Some(limit) => ui::Seats {
            any: false,
            most: limit.get().into(),
        },
        None => ui::Seats {
            any: true,
            most: RoomLimit::MOST.into(),
        },
    }
}

fn noise_of(mode: DenoiseMode) -> ui::Noise {
    match mode {
        DenoiseMode::Off => ui::Noise::Off,
        DenoiseMode::Light => ui::Noise::Light,
        DenoiseMode::Strong => ui::Noise::Strong,
    }
}

fn denoise_of(noise: ui::Noise) -> DenoiseMode {
    match noise {
        ui::Noise::Off => DenoiseMode::Off,
        ui::Noise::Light => DenoiseMode::Light,
        ui::Noise::Strong => DenoiseMode::Strong,
    }
}

fn ui_appearance_of(appearance: Appearance) -> ui::Appearance {
    match appearance {
        Appearance::System => ui::Appearance::System,
        Appearance::Light => ui::Appearance::Light,
        Appearance::Dark => ui::Appearance::Dark,
    }
}

fn appearance_of(appearance: ui::Appearance) -> Appearance {
    match appearance {
        ui::Appearance::System => Appearance::System,
        ui::Appearance::Light => Appearance::Light,
        ui::Appearance::Dark => Appearance::Dark,
    }
}

fn share_tab_of(kind: SourceKind) -> ui::ShareTab {
    match kind {
        SourceKind::Screen => ui::ShareTab::Screens,
        SourceKind::Window => ui::ShareTab::Windows,
    }
}

fn source_kind_of(tab: ui::ShareTab) -> SourceKind {
    match tab {
        ui::ShareTab::Screens => SourceKind::Screen,
        ui::ShareTab::Windows => SourceKind::Window,
    }
}

fn trouble_in_words(trouble: &AudioTrouble) -> ui::Trouble {
    let (device, noun, them) = match trouble.side() {
        Some(Side::Microphone) => (ui::TroubleWith::Microphone, "Microphone", "It"),
        Some(Side::Speakers) => (ui::TroubleWith::Speakers, "Speakers", "They"),
        None => (ui::TroubleWith::Program, "Sound", "It"),
    };
    let (title, caption) = match trouble {
        AudioTrouble::Missing(Side::Microphone) => (
            "No microphone".to_owned(),
            "Plug one in and it is picked up by itself.".to_owned(),
        ),
        AudioTrouble::Missing(Side::Speakers) => (
            "No speakers".to_owned(),
            "Plug in speakers or headphones and they are picked up by themselves.".to_owned(),
        ),
        AudioTrouble::Unplugged(_, name) => (
            format!("{noun} not found"),
            format!("\"{name}\" is not plugged in."),
        ),
        AudioTrouble::Broken(_) => (
            format!("{noun} not working"),
            format!("{them} would not open. Trying again..."),
        ),
        AudioTrouble::Failed => ("Sound not working".to_owned(), "Trying again...".to_owned()),
    };
    let (button, button_label) = match (trouble, trouble.side()) {
        (AudioTrouble::Missing(_), _) => ("", ""),
        (_, Some(Side::Microphone)) => ("Choose another", "Choose another microphone"),
        (_, Some(Side::Speakers)) => ("Choose another", "Choose other speakers"),
        (_, None) => ("Audio settings", "Audio settings"),
    };
    ui::Trouble {
        device,
        title: title.into(),
        caption: caption.into(),
        button: button.into(),
        button_label: button_label.into(),
    }
}

fn server_entry(entry: &Entry) -> ui::ServerEntry {
    let (standing, occupancy) = match entry.status {
        Status::Checking => (Standing::Checking, None),
        Status::Up(occupancy) => (Standing::Up, occupancy),
        Status::OtherVersion(_) => (Standing::OtherVersion, None),
        Status::Silent => (Standing::Silent, None),
    };
    let Occupancy { rooms, people } = occupancy.unwrap_or(Occupancy {
        rooms: 0,
        people: 0,
    });
    ui::ServerEntry {
        name: entry.name.as_str().into(),
        address: entry.address().into(),
        standing,
        rooms: rooms as i32,
        people: people as i32,
        says: status_in_words(entry.status).into(),
        remembered: entry.recent.is_some(),
    }
}

fn status_in_words(status: Status) -> &'static str {
    match status {
        Status::Checking => "Checking",
        Status::Up(_) => "Online",
        Status::OtherVersion(_) => "Other version",
        Status::Silent => "Offline",
    }
}

fn version_in_words(server: &str, theirs: Option<u16>) -> String {
    let ours = PROTOCOL_VERSION;
    match theirs {
        Some(theirs) if theirs > ours => {
            format!("\"{server}\" has Whisperia {theirs}, this PC has {ours}. Update this PC.")
        }
        Some(theirs) => {
            format!("\"{server}\" has Whisperia {theirs}, this PC has {ours}. Update the server.")
        }
        None => format!("\"{server}\" has another Whisperia version. Update the older one."),
    }
}

fn picture_of(preview: &Preview) -> Option<slint::Image> {
    let bytes = (preview.width as usize)
        .checked_mul(preview.height as usize)?
        .checked_mul(4)?;
    (bytes > 0 && preview.rgba.len() == bytes).then(|| {
        slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
            &preview.rgba,
            preview.width,
            preview.height,
        ))
    })
}

#[cfg(windows)]
fn current_picture(sharer: PeerId) -> Option<slint::Image> {
    pictures::current(sharer)
}

#[cfg(not(windows))]
fn current_picture(_: PeerId) -> Option<slint::Image> {
    None
}

fn offer<'a>(model: &VecModel<ui::Source>, wanted: impl Iterator<Item = &'a Offered>) {
    let wanted: Vec<&Offered> = wanted.collect();
    for row in (0..model.row_count()).rev() {
        if model
            .row_data(row)
            .is_some_and(|shown| !wanted.iter().any(|offered| offered.id == shown.id))
        {
            model.remove(row);
        }
    }
    let card = |offered: &Offered, shown: Option<ui::Source>| {
        let (picture, icon) = shown.map_or((None, None), |shown| {
            (
                (shown.has_preview && shown.pictures == offered.pictures as i32)
                    .then_some(shown.preview),
                shown.has_icon.then_some(shown.icon),
            )
        });
        let picture = picture.or_else(|| offered.preview.as_ref().and_then(picture_of));
        let icon = icon.or_else(|| offered.icon.as_ref().and_then(picture_of));
        ui::Source {
            id: offered.id,
            label: offered.source.label.as_str().into(),
            detail: offered.source.detail.as_str().into(),
            has_preview: picture.is_some(),
            preview: picture.unwrap_or_default(),
            pictures: offered.pictures as i32,
            has_icon: icon.is_some(),
            icon: icon.unwrap_or_default(),
            minimized: offered.source.minimized,
        }
    };
    for (row, offered) in wanted.iter().enumerate() {
        match model.row_data(row) {
            Some(shown) if shown.id == offered.id => {
                if shown.label != offered.source.label.as_str()
                    || shown.detail != offered.source.detail.as_str()
                    || shown.pictures != offered.pictures as i32
                    || shown.has_icon != offered.icon.is_some()
                    || shown.minimized != offered.source.minimized
                {
                    model.set_row_data(row, card(offered, Some(shown)));
                }
            }
            _ => model.insert(row, card(offered, None)),
        }
    }
    while model.row_count() > wanted.len() {
        model.remove(model.row_count() - 1);
    }
}

fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, wanted: Vec<T>) {
    while model.row_count() > wanted.len() {
        model.remove(model.row_count() - 1);
    }
    for (row, item) in wanted.into_iter().enumerate() {
        match model.row_data(row) {
            Some(shown) if shown == item => {}
            Some(_) => model.set_row_data(row, item),
            None => model.push(item),
        }
    }
}

fn strings(items: impl Iterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(VecModel::from(
        items.map(SharedString::from).collect::<Vec<_>>(),
    )))
}

fn keys_in_words(keys: Option<crate::desktop::keys::Combination>) -> String {
    keys.and_then(crate::desktop::keys::name)
        .unwrap_or_default()
}

fn device_at(devices: &[String], index: i32) -> Option<String> {
    usize::try_from(index)
        .ok()?
        .checked_sub(1)
        .and_then(|at| devices.get(at).cloned())
}

fn credit_for(mode: DenoiseMode) -> (&'static str, &'static str) {
    match mode {
        DenoiseMode::Off => ("", ""),
        DenoiseMode::Light => ("RNNoise", "https://jmvalin.ca/demo/rnnoise/"),
        DenoiseMode::Strong => ("DeepFilterNet", "https://github.com/Rikorose/DeepFilterNet"),
    }
}

fn graded(quality: Quality) -> ui::Quality {
    match quality {
        Quality::Good => ui::Quality::Good,
        Quality::Weak => ui::Quality::Weak,
        Quality::Poor => ui::Quality::Poor,
    }
}

fn delay_in_words(ms: f32) -> String {
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{ms:.0} ms")
    }
}

fn loss_in_words(part: f32) -> String {
    let percent = part * 100.0;
    if percent <= 0.0 {
        "0 %".to_owned()
    } else if percent < 0.1 {
        "< 0.1 %".to_owned()
    } else {
        format!("{percent:.1} %")
    }
}

fn loudness(db: f32) -> String {
    if crate::dsp::is_silent(db) {
        "off".to_owned()
    } else {
        format!("{:+.0} dB", db.round() + 0.0)
    }
}

fn along_the_meter(db: f32) -> f32 {
    ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0)
}

fn face(name: &str, avatar: Avatar) -> ui::Face {
    ui::Face {
        name: name.into(),
        initials: initial(name).into(),
        animal: glyph(avatar).into(),
        tint: tint(name),
    }
}

fn sentence(text: &str) -> String {
    let mut chars = text.trim().chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut said: String = first.to_uppercase().chain(chars).collect();
    if !said.ends_with(['.', '!', '?']) {
        said.push('.');
    }
    said
}

fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .or_else(|| name.chars().find(|c| !c.is_whitespace()))
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect()
}

fn glyph(avatar: Avatar) -> &'static str {
    match avatar {
        Avatar::None => "",
        Avatar::Fox => "🦊",
        Avatar::Cat => "🐱",
        Avatar::Dog => "🐶",
        Avatar::Bear => "🐻",
        Avatar::Panda => "🐼",
        Avatar::Koala => "🐨",
        Avatar::Tiger => "🐯",
        Avatar::Lion => "🦁",
        Avatar::Monkey => "🐵",
        Avatar::Hamster => "🐹",
        Avatar::Rabbit => "🐰",
        Avatar::Mouse => "🐭",
        Avatar::Cow => "🐮",
        Avatar::Pig => "🐷",
        Avatar::Frog => "🐸",
        Avatar::Chick => "🐥",
        Avatar::Penguin => "🐧",
        Avatar::Owl => "🦉",
        Avatar::Duck => "🦆",
        Avatar::Wolf => "🐺",
        Avatar::Raccoon => "🦝",
        Avatar::Hedgehog => "🦔",
        Avatar::Unicorn => "🦄",
        Avatar::Octopus => "🐙",
        Avatar::Whale => "🐳",
        Avatar::Shark => "🦈",
        Avatar::Turtle => "🐢",
        Avatar::Butterfly => "🦋",
        Avatar::Bee => "🐝",
    }
}

fn animal_label(avatar: Avatar) -> String {
    match avatar {
        Avatar::None => "No animal".to_owned(),
        animal => {
            let key = animal.key();
            key[..1].to_uppercase() + &key[1..]
        }
    }
}

const TINTS: u32 = 10;

fn tint(name: &str) -> i32 {
    let hash = name.bytes().fold(2166136261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16777619)
    });
    ((hash >> 8) % TINTS) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_version_says_which_side_to_update() {
        let ours = PROTOCOL_VERSION;
        assert_eq!(
            version_in_words("DESKTOP-R8ZE", Some(ours + 1)),
            format!(
                "\"DESKTOP-R8ZE\" has Whisperia {}, this PC has {ours}. Update this PC.",
                ours + 1
            )
        );
        assert_eq!(
            version_in_words("DESKTOP-R8ZE", Some(ours - 1)),
            format!(
                "\"DESKTOP-R8ZE\" has Whisperia {}, this PC has {ours}. Update the server.",
                ours - 1
            )
        );
        assert_eq!(
            version_in_words("DESKTOP-R8ZE", None),
            "\"DESKTOP-R8ZE\" has another Whisperia version. Update the older one."
        );
    }

    #[test]
    fn a_missing_device_offers_no_other_to_choose() {
        let missing = trouble_in_words(&AudioTrouble::Missing(Side::Microphone));
        assert_eq!(missing.title, "No microphone");
        assert_eq!(missing.button, "");
        let broken = trouble_in_words(&AudioTrouble::Broken(Side::Microphone));
        assert_eq!(broken.button_label, "Choose another microphone");
    }

    #[test]
    fn a_reason_is_shown_as_a_sentence() {
        assert_eq!(sentence("server shutting down"), "Server shutting down.");
        assert_eq!(sentence("Timed out."), "Timed out.");
        assert_eq!(sentence("  "), "");
    }

    #[test]
    fn names_become_initials_and_colours() {
        assert_eq!(initial("Panda Raccoon Fox"), "P");
        assert_eq!(initial("panda"), "P");
        assert_eq!(initial("Panda (2)"), "P");
        assert_eq!(initial("(2) panda"), "2");
        assert_eq!(initial(":)"), ":");
        assert_eq!(initial("  "), "");
        assert_eq!(initial("ягеренте"), "Я");
        assert_eq!(tint("Panda"), tint("Panda"));
        assert!((0..TINTS as i32).contains(&tint("Panda")));
        assert_ne!(tint("Panda"), tint("Raccoon"));
    }

    #[test]
    fn every_animal_has_a_cell_to_be_picked_in() {
        assert_eq!(Avatar::ALL.len(), 30);
        assert_eq!(glyph(Avatar::None), "");
        assert_eq!(animal_label(Avatar::None), "No animal");
        assert_eq!(animal_label(Avatar::Fox), "Fox");
        assert_eq!(animal_label(Avatar::Hedgehog), "Hedgehog");
        for animal in Avatar::ALL.into_iter().skip(1) {
            assert_ne!(glyph(animal), "", "{}", animal.key());
        }
        let mut glyphs: Vec<&str> = Avatar::ALL.into_iter().map(glyph).collect();
        glyphs.sort_unstable();
        glyphs.dedup();
        assert_eq!(glyphs.len(), Avatar::ALL.len());
    }

    #[test]
    fn the_room_looked_into_stays_the_one_it_is() {
        let room = |name: &str| RoomSummary {
            name: RoomName::new(name).unwrap(),
            people: vec![whisperia_proto::control::RoomPerson {
                name: Name::new("Panda").unwrap(),
                avatar: Avatar::None,
                host: true,
            }],
            locked: false,
            limit: None,
        };
        let mut looking = Looking::default();
        assert_eq!(looking.settle(&[]), None);

        assert_eq!(looking.settle(&[room("General")]), None);

        let mut looking = Looking::default();
        assert_eq!(looking.settle(&[room("General")]), Some(0));
        assert_eq!(
            looking.settle(&[room("Backroom"), room("General")]),
            Some(1)
        );
        assert_eq!(looking.settle(&[room("Backroom")]), Some(0));
        assert_eq!(looking.settle(&[]), None);
        assert_eq!(looking, Looking::NewRoom);
    }

    #[test]
    fn a_room_in_the_list_says_how_full_it_is_unless_it_is_locked() {
        assert_eq!(people_in_words(1, None), "1 person");
        assert_eq!(people_in_words(4, None), "4 people");
        assert_eq!(people_in_words(1, Some(RoomLimit::new(2))), "1 of 2 people");
        let person = |name: &str| whisperia_proto::control::RoomPerson {
            name: Name::new(name).unwrap(),
            avatar: Avatar::None,
            host: false,
        };
        let mut room = RoomSummary {
            name: RoomName::new("General").unwrap(),
            people: vec![person("Panda"), person("Fox")],
            locked: false,
            limit: None,
        };
        let entry = room_entry(&room);
        assert_eq!((entry.tally.as_str(), entry.full), ("2", false));
        room.limit = Some(RoomLimit::new(2));
        let entry = room_entry(&room);
        assert_eq!(
            (entry.tally.as_str(), entry.people.as_str(), entry.full),
            ("2/2", "2 of 2 people", true)
        );
        room.people.clear();
        room.limit = None;
        room.locked = true;
        let entry = room_entry(&room);
        assert!(entry.locked && !entry.full);
    }

    #[test]
    fn seats_for_any_number_are_no_limit_and_a_number_is_kept_in_range() {
        let most = |people| Some(RoomLimit::new(people));
        let seats = |any, most| ui::Seats { any, most };
        assert_eq!(limit_of(seats(true, 5)), None);
        assert_eq!(limit_of(seats(false, 25)), most(25));
        assert_eq!(limit_of(seats(false, 2)), most(2));
        assert_eq!(limit_of(seats(false, -1)), most(2));
        assert_eq!(limit_of(seats(false, 300)), most(25));
        assert!(seats_of(None).any);
        assert_eq!(seats_of(most(5)), seats(false, 5));
        assert_eq!(limit_of(seats_of(None)), None);
        assert_eq!(limit_of(seats_of(most(7))), most(7));
    }

    #[test]
    fn choices_in_the_window_map_both_ways() {
        for mode in [DenoiseMode::Off, DenoiseMode::Light, DenoiseMode::Strong] {
            assert_eq!(denoise_of(noise_of(mode)), mode);
        }
        for appearance in [Appearance::System, Appearance::Light, Appearance::Dark] {
            assert_eq!(appearance_of(ui_appearance_of(appearance)), appearance);
        }
        for kind in [SourceKind::Screen, SourceKind::Window] {
            assert_eq!(source_kind_of(share_tab_of(kind)), kind);
        }
    }

    #[test]
    fn the_connection_is_put_into_words() {
        assert_eq!(delay_in_words(0.42), "0.4 ms");
        assert_eq!(delay_in_words(9.96), "10.0 ms");
        assert_eq!(delay_in_words(23.4), "23 ms");
        assert_eq!(loss_in_words(0.0), "0 %");
        assert_eq!(loss_in_words(0.0004), "< 0.1 %");
        assert_eq!(loss_in_words(0.0237), "2.4 %");
        assert_eq!(loss_in_words(1.0), "100.0 %");
    }

    #[test]
    fn volumes_and_levels_are_put_into_words_and_bars() {
        assert_eq!(loudness(3.0), "+3 dB");
        assert_eq!(loudness(-0.2), "+0 dB");
        assert_eq!(loudness(-30.0), "off");
        assert_eq!(along_the_meter(-60.0), 0.0);
        assert_eq!(along_the_meter(0.0), 1.0);
        assert_eq!(along_the_meter(-30.0), 0.5);
    }

    #[test]
    fn a_list_in_the_window_follows_the_one_it_shows() {
        let model = VecModel::from(vec![1, 2, 3]);
        sync(&model, vec![1, 5]);
        assert_eq!(model.iter().collect::<Vec<_>>(), [1, 5]);
        sync(&model, vec![1, 5, 8, 9]);
        assert_eq!(model.iter().collect::<Vec<_>>(), [1, 5, 8, 9]);
        sync(&model, Vec::new());
        assert_eq!(model.row_count(), 0);
    }

    #[test]
    fn line_zero_of_a_device_list_is_the_system_default() {
        let devices = ["Headset".to_owned(), "Webcam".to_owned()];
        assert_eq!(device_at(&devices, 0), None);
        assert_eq!(device_at(&devices, 2), Some("Webcam".to_owned()));
        assert_eq!(device_at(&devices, 9), None);
        assert_eq!(device_at(&devices, -1), None);
    }
}
