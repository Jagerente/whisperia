use whisperia::{
    audio::fake::FakeBackend,
    config::{Appearance, Config},
    desktop::DesktopLink,
};
use whisperia_e2e::gui::*;

#[test]
fn the_colours_are_chosen_in_the_settings_and_kept() {
    let mut panda = window(named("Panda"));
    wait_for_text(&mut panda, "Servers");
    press(&mut panda, "Settings");
    wait_for_switch(&mut panda, "Appearance: Like Windows", true);
    assert_eq!(panda.is_on("Appearance: Light"), Some(false));
    assert_eq!(panda.is_on("Appearance: Dark"), Some(false));

    press(&mut panda, "Appearance: Dark");
    wait_for_switch(&mut panda, "Appearance: Dark", true);
    assert!(panda.in_dark_colours());
    assert_eq!(panda.saved_settings().appearance, Appearance::Dark);
    picture(&panda, "37-appearance-dark");

    press(&mut panda, "Appearance: Light");
    wait_for_switch(&mut panda, "Appearance: Light", true);
    assert!(!panda.in_dark_colours());
    assert_eq!(panda.saved_settings().appearance, Appearance::Light);
    picture(&panda, "38-appearance-light");

    press(&mut panda, "Appearance: Like Windows");
    wait_for_switch(&mut panda, "Appearance: Like Windows", true);
    assert_eq!(panda.saved_settings().appearance, Appearance::System);

    press(&mut panda, "Appearance: Dark");
    wait_for_switch(&mut panda, "Appearance: Dark", true);
    let saved = panda.saved_settings();
    drop(panda);
    let mut again = window(saved);
    again.tick();
    assert!(again.in_dark_colours(), "the choice outlives the window");
}

fn audio_settings() -> Window {
    let mut panda = window(named("Panda"));
    wait_for_text(&mut panda, "Servers");
    press(&mut panda, "Settings");
    wait_for_switch(&mut panda, "General", true);
    press(&mut panda, "Audio");
    wait_for_switch(&mut panda, "Audio", true);
    panda
}

#[test]
fn a_microphone_plugged_in_joins_the_open_list() {
    let backend = FakeBackend::quiet(48_000, None);
    let mut panda = opening(named("Panda")).backend(backend.clone()).open();
    wait_for_text(&mut panda, "Servers");
    press(&mut panda, "Settings");
    press(&mut panda, "Audio");
    wait_for_switch(&mut panda, "Audio", true);

    backend
        .listed
        .lock()
        .unwrap()
        .inputs
        .push("Yeti Nano".into());
    backend.devices_changed();
    press(&mut panda, "Microphone");
    wait_until(&mut panda, "the new microphone offered", |ui| {
        ui.has("Yeti Nano")
    });
}

#[test]
fn the_audio_tab_puts_the_microphone_first() {
    let mut panda = audio_settings();
    assert!(panda.has("Automatic gain control"));
    assert!(!panda.has("Notification sounds"));
    assert!(
        !panda.screen().contains("Resolution"),
        "what to share is asked when sharing"
    );

    let (_, off) = middle_of(&panda, "Noise suppression: Off");
    let (_, deep) = middle_of(&panda, "Noise suppression: Strong");
    assert!(
        (off - deep).abs() < 1.0,
        "the kinds of noise suppression stand in a row: {off} and {deep}"
    );
    let (meter_x, meter_y) = middle_of(&panda, "Microphone level");
    let (gain_x, gain_y) = middle_of(&panda, "Microphone gain");
    let (_, auto_y) = middle_of(&panda, "Automatic gain control");
    assert!(
        (meter_x - gain_x).abs() < 1.0 && meter_y < gain_y && (gain_y - auto_y).abs() < 1.0,
        "the level stands over the gain, auto next to it: ({meter_x}, {meter_y}), ({gain_x}, {gain_y}), {auto_y}"
    );
    let test = middle_of(&panda, "Test the microphone");
    assert!(
        (test.1 - meter_y).abs() < 1.0 && test.0 > meter_x,
        "the test is next to the level: {test:?} and ({meter_x}, {meter_y})"
    );
    let (_, speaker_volume) = middle_of(&panda, "Speaker volume");
    assert!(
        gain_y < off && off < speaker_volume,
        "the microphone comes before the speakers: {gain_y}, {off}, {speaker_volume}"
    );
    picture(&panda, "05-settings");

    slide(&mut panda, "Speaker volume", 150.0);
    wait_for_value(&mut panda, "Speaker volume", "150");
    assert!(panda.has_text("150 %"));

    press(&mut panda, "Test the microphone");
    wait_until(&mut panda, "the test running", |ui| {
        ui.has("Stop the microphone test")
    });
    assert_eq!(
        panda.middle_of("Stop the microphone test"),
        Some(test),
        "the test stops where it started"
    );
    assert_eq!(
        panda.middle_of("Noise suppression: Off").map(|(_, y)| y),
        Some(off),
        "nothing moves while testing"
    );
    picture(&panda, "05b-settings-testing");
}

#[test]
fn a_microphone_is_chosen_from_the_picker_and_kept() {
    let mut panda = audio_settings();
    assert_eq!(
        panda.value_of("Microphone").as_deref(),
        Some("System default")
    );
    press(&mut panda, "Microphone");
    wait_until(&mut panda, "the list of microphones", |ui| {
        ui.has("Fake microphone")
    });
    assert_eq!(panda.is_on("System default"), Some(true));
    picture(&panda, "05c-choosing-a-microphone");
    press(&mut panda, "Fake microphone");
    wait_for_value(&mut panda, "Microphone", "Fake microphone");
    assert_eq!(
        panda.saved_settings().input_device.as_deref(),
        Some("Fake microphone")
    );

    type_into(&mut panda, "Speakers", "Fake speaker");
    wait_for_value(&mut panda, "Speakers", "Fake speaker");
    assert_eq!(
        panda.saved_settings().output_device.as_deref(),
        Some("Fake speaker")
    );
}

#[test]
fn noise_suppression_names_what_powers_it() {
    let mut panda = audio_settings();
    assert!(!panda.has_text("Powered by"));
    press(&mut panda, "Noise suppression: Light");
    wait_for_switch(&mut panda, "Noise suppression: Light", true);
    assert!(panda.has_text("Powered by"));
    assert!(panda.has("Powered by RNNoise"), "{}", panda.screen());
    press(&mut panda, "Noise suppression: Strong");
    wait_until(&mut panda, "DeepFilterNet named", |ui| {
        ui.has("Powered by DeepFilterNet")
    });
    press(&mut panda, "Noise suppression: Off");
    wait_until_text_gone(&mut panda, "Powered by");
}

#[test]
fn notification_switches_are_kept() {
    let mut panda = window(named("Panda"));
    wait_for_text(&mut panda, "Servers");
    press(&mut panda, "Settings");
    press(&mut panda, "Notifications");
    wait_for_switch(&mut panda, "Notification sounds", true);
    assert!(panda.has_text("People") && panda.has_text("You"));
    assert_eq!(panda.is_on("Sound: A message arrives"), Some(true));
    assert!(panda.has("Play: Somebody joins the room"));
    assert!(!panda.has("Noise suppression: Off"));

    press(&mut panda, "Sound: A message arrives");
    wait_for_switch(&mut panda, "Sound: A message arrives", false);
    picture(&panda, "23-notifications");
    press(&mut panda, "Notification sounds");
    wait_for_switch(&mut panda, "Notification sounds", false);
    assert_eq!(panda.is_on("Sound: A message arrives"), Some(false));
    assert_eq!(panda.is_on("Sound: Somebody joins the room"), Some(true));
    assert!(panda.scroll("Sound: Deafened", 600.0));
    wait_for_switch(&mut panda, "Sound: The connection is lost", true);
    let saved = panda.saved_settings();
    assert!(!saved.sounds && !saved.notifications.message);
    assert!(saved.notifications.lost);
}

#[test]
fn automatic_gain_takes_over_the_gain_slider() {
    let mut panda = audio_settings();
    assert_eq!(panda.is_on("Automatic gain control"), Some(true));
    slide(&mut panda, "Microphone gain", 6.0);
    press(&mut panda, "Automatic gain control");
    wait_for_switch(&mut panda, "Automatic gain control", false);
    wait_for_value(&mut panda, "Microphone gain", "0");
    slide(&mut panda, "Microphone gain", 6.0);
    wait_for_value(&mut panda, "Microphone gain", "6");
}

#[test]
fn the_microphone_gain_goes_up_to_thirty_db() {
    let mut panda = audio_settings();
    press(&mut panda, "Automatic gain control");
    wait_for_switch(&mut panda, "Automatic gain control", false);
    slide(&mut panda, "Microphone gain", 30.0);
    wait_for_value(&mut panda, "Microphone gain", "30");
}

#[test]
fn automatic_gain_shows_its_own_gain_without_the_manual_one() {
    let mut panda = audio_settings();
    press(&mut panda, "Automatic gain control");
    wait_for_switch(&mut panda, "Automatic gain control", false);
    slide(&mut panda, "Microphone gain", 6.0);
    wait_for_value(&mut panda, "Microphone gain", "6");
    press(&mut panda, "Automatic gain control");
    wait_for_switch(&mut panda, "Automatic gain control", true);
    wait_for_value(&mut panda, "Microphone gain", "0");
}

#[test]
fn the_general_tab_tells_what_closing_does() {
    let mut panda = window(named("Panda"));
    press(&mut panda, "Settings");
    wait_for_switch(&mut panda, "General", true);
    assert!(
        panda.screen().contains("Clicking X quits Whisperia"),
        "without a tray the window says so: {}",
        panda.screen()
    );
    picture(&panda, "24-general");
}

fn about() -> Window {
    let mut panda = window(named("Panda"));
    press(&mut panda, "Settings");
    press(&mut panda, "About");
    wait_for_switch(&mut panda, "About", true);
    panda
}

#[test]
fn the_about_tab_names_the_program_and_its_version() {
    let panda = about();
    assert!(panda.has_text("Whisperia"));
    assert!(
        panda.has_text(&format!("Version {}", env!("CARGO_PKG_VERSION"))),
        "{}",
        panda.screen()
    );
    picture(&panda, "24c-about");
}

#[test]
fn the_about_tab_opens_the_source_code_on_github() {
    let mut panda = about();
    press(&mut panda, "Source code on GitHub");
    wait_until(&mut panda, "the source code opened", |ui| {
        ui.opened() == ["https://github.com/jagerente/whisperia"]
    });
}

#[test]
fn the_fine_print_opens_the_licence_and_slint() {
    let mut panda = about();
    press(
        &mut panda,
        "Free software under the GNU GPL 3.0 or later, with no warranty",
    );
    press(&mut panda, "Made with Slint");
    wait_until(&mut panda, "the licence and Slint opened", |ui| {
        ui.opened()
            == [
                "https://www.gnu.org/licenses/gpl-3.0.html",
                "https://slint.dev",
            ]
    });
}

#[test]
fn closing_the_window_keeps_it_in_the_tray_or_quits() {
    let (link, _desktop, _commands) = DesktopLink::channels(&whisperia::wake::Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();
    press(&mut panda, "Settings");
    wait_for_switch(&mut panda, "Minimize Whisperia to the system tray", true);
    assert!(
        panda.has_text("Clicking X minimizes Whisperia instead of closing it completely."),
        "{}",
        panda.screen()
    );
    press(&mut panda, "Minimize Whisperia to the system tray");
    wait_for_switch(&mut panda, "Minimize Whisperia to the system tray", false);
    picture(&panda, "27-general-window");
    press(&mut panda, "Close");
    assert!(!panda.saved_settings().close_to_tray);
}

#[test]
fn closing_the_window_hides_it_in_the_tray() {
    let (link, _desktop, _commands) = DesktopLink::channels(&whisperia::wake::Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();
    assert!(panda.is_shown());
    panda.close();
    panda.tick();
    assert!(!panda.is_shown());
    assert!(!panda.wants_to_quit());
}

#[test]
fn closing_the_window_quits_when_asked_to() {
    let (link, _desktop, _commands) = DesktopLink::channels(&whisperia::wake::Waker::default());
    let mut panda = opening(Config {
        close_to_tray: false,
        ..named("Panda")
    })
    .desktop(link)
    .open();
    panda.close();
    wait_until(&mut panda, "the program quitting", |ui| ui.wants_to_quit());
}

#[test]
fn closing_the_window_quits_when_there_is_no_tray() {
    let mut panda = window(named("Panda"));
    panda.close();
    wait_until(&mut panda, "the program quitting", |ui| ui.wants_to_quit());
}
