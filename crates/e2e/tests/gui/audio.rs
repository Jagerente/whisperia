use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use whisperia::{audio::fake::FakeBackend, gui::Harness};
use whisperia_e2e::{
    gui::*,
    net,
    signal::{is_tone, last, peak_after, played},
    TestServer,
};

fn opened(count: &std::sync::atomic::AtomicUsize) -> usize {
    count.load(Ordering::Relaxed)
}

#[test]
fn new_default_devices_are_taken_without_a_word_and_the_voice_goes_on() {
    let server = TestServer::start("DESKTOP-AV83");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let raccoon = runtime.block_on(net::join(server.addr(), "Raccoon"));
    let backend = FakeBackend::quiet(48_000, Some((440.0, 0.3)));
    let mut panda = opening(named("Panda")).backend(backend.clone()).open();
    enter_general(&mut panda, &server.address(), "Panda");
    let raccoon_hears = |_: &mut Harness| is_tone(&last(&raccoon.audio, 9_600), 48_000, 440.0);
    wait_until(&mut panda, "Raccoon hearing Panda", raccoon_hears);

    let (microphones, speakers) = (
        opened(&backend.microphones_opened),
        opened(&backend.speakers_opened),
    );
    backend.move_default_devices();
    wait_until(&mut panda, "both devices opened again", |_| {
        opened(&backend.microphones_opened) > microphones
            && opened(&backend.speakers_opened) > speakers
    });
    let screen = panda.screen();
    assert!(!screen.contains("not working"), "{screen}");
    let heard_until = raccoon.audio.played.lock().unwrap().len();
    wait_until(&mut panda, "Raccoon still hearing Panda", |_| {
        raccoon.audio.played.lock().unwrap().len() > heard_until + 9_600
            && is_tone(&last(&raccoon.audio, 9_600), 48_000, 440.0)
    });
}

#[test]
fn a_device_pulled_out_is_followed_to_the_next_one_without_a_word() {
    let server = TestServer::start("DESKTOP-AV83");
    let backend = FakeBackend::quiet(48_000, None);
    let mut panda = opening(named("Panda")).backend(backend.clone()).open();
    enter_general(&mut panda, &server.address(), "Panda");
    wait_until(&mut panda, "both devices open", |_| {
        opened(&backend.microphones_opened) > 0 && opened(&backend.speakers_opened) > 0
    });
    let (microphones, speakers) = (
        opened(&backend.microphones_opened),
        opened(&backend.speakers_opened),
    );
    backend.unplug_devices();
    wait_until(&mut panda, "both devices opened again", |_| {
        opened(&backend.microphones_opened) > microphones
            && opened(&backend.speakers_opened) > speakers
    });
    let screen = panda.screen();
    assert!(!screen.contains("not working"), "{screen}");
}

#[test]
fn dead_speakers_leave_the_microphone_heard() {
    let server = TestServer::start("DESKTOP-AV83");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let raccoon = runtime.block_on(net::join(server.addr(), "Raccoon"));
    let backend = FakeBackend::quiet(48_000, Some((440.0, 0.3)));
    backend.speakers_broken.store(true, Ordering::Relaxed);
    let mut panda = opening(named("Panda")).backend(backend).open();
    enter_general(&mut panda, &server.address(), "Panda");
    wait_for(&mut panda, "Speakers not working");
    wait_until(&mut panda, "Raccoon hearing Panda", |_| {
        is_tone(&last(&raccoon.audio, 9_600), 48_000, 440.0)
    });
}

#[test]
fn a_broken_microphone_and_broken_speakers_are_both_named() {
    let server = TestServer::start("DESKTOP-9FJD");
    let backend = FakeBackend::quiet(48_000, None);
    backend.microphone_broken.store(true, Ordering::Relaxed);
    backend.speakers_broken.store(true, Ordering::Relaxed);
    let mut panda = opening(named("Panda")).backend(backend).open();
    enter_general(&mut panda, &server.address(), "Panda");
    wait_for(&mut panda, "Microphone not working");
    wait_for(&mut panda, "Speakers not working");
    assert!(panda.has("Choose another microphone"));
    assert!(panda.has("Choose other speakers"));
    picture(&panda, "41c-both-not-working");
}

#[test]
fn a_microphone_plugged_in_is_taken_when_windows_tells() {
    let server = TestServer::start("DESKTOP-9FJD");
    let backend = FakeBackend::quiet(48_000, None);
    backend.microphone_missing.store(true, Ordering::Relaxed);
    let mut panda = opening(named("Panda")).backend(backend.clone()).open();
    enter_general(&mut panda, &server.address(), "Panda");
    wait_for(&mut panda, "No microphone");

    backend.microphone_missing.store(false, Ordering::Relaxed);
    backend.devices_changed();
    wait_until_gone(&mut panda, "No microphone");
    assert!(opened(&backend.microphones_opened) > 0);
}

const CUE_PEAK: std::ops::Range<f32> = 0.03..0.2;

#[test]
fn the_quick_audio_panel_sets_the_volume_and_tests_the_microphone() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    let (_, buttons) = middle_of(&panda, "Mute microphone");

    press(&mut panda, "Quick audio settings");
    wait_for_text(&mut panda, "Audio");
    assert_eq!(panda.is_on("Automatic gain control"), Some(true));
    assert_eq!(panda.is_on("Noise suppression: Off"), Some(true));
    assert!(panda.screen().contains("automatic"));
    slide(&mut panda, "Speaker volume", 120.0);
    wait_for_value(&mut panda, "Speaker volume", "120");
    assert!(panda.has_text("120 %"));
    let (x, y) = middle_of(&panda, "Speaker volume");
    assert!(
        x < LEFT_SIDE && y < buttons,
        "the panel belongs over the buttons at the bottom left, not at ({x}, {y})"
    );

    assert!(panda.has_text("Microphone test"));
    let (_, test_line) = middle_of(&panda, "Test the microphone");
    let (_, gain_line) = middle_of(&panda, "Microphone gain");
    let (_, noise_line) = middle_of(&panda, "Noise suppression: Off");
    assert!(
        test_line < gain_line && gain_line < noise_line && noise_line < y,
        "the panel keeps the order of the settings: test, gain, noise, volume"
    );
    let (button, line) = middle_of(&panda, "Test the microphone");
    let (meter, meter_line) = middle_of(&panda, "Microphone level");
    assert!(
        (line - meter_line).abs() < 1.0 && button < meter && meter < LEFT_SIDE,
        "the level belongs opposite the button: {button}, {line} and {meter}, {meter_line}"
    );
    press(&mut panda, "Test the microphone");
    wait_until(&mut panda, "the test running", |ui| {
        ui.has("Stop the microphone test")
    });
    assert_eq!(
        panda.middle_of("Stop the microphone test"),
        Some((button, line)),
        "the test stops where it started"
    );
    wait_until(&mut panda, "the level moving", |ui| {
        ui.value_of("Microphone level").as_deref() != Some("0")
    });
    picture(&panda, "26-quick-audio");

    press(&mut panda, "Quick audio settings");
    wait_until_text_gone(&mut panda, "Audio");
    press(&mut panda, "Quick audio settings");
    wait_until(&mut panda, "the test stopped by closing", |ui| {
        ui.has("Test the microphone")
    });
    press(&mut panda, "Quick audio settings");
    wait_until_text_gone(&mut panda, "Audio");
}

#[test]
fn a_sound_is_listened_to_from_the_first_page_and_the_speaker_is_let_go_after() {
    let (mut panda, speaker) = window_heard(named("Panda"));
    wait_for_text(&mut panda, "Servers");
    press(&mut panda, "Settings");
    press(&mut panda, "Notifications");
    wait_for_switch(&mut panda, "Notification sounds", true);
    assert!(panda.has("Play: Somebody joins the room"));

    let quiet = played(&speaker);
    press(&mut panda, "Play: Somebody joins the room");
    let peak = peak_after(&speaker, quiet, PATIENCE);
    assert!(CUE_PEAK.contains(&peak), "nothing was heard: peak {peak}");
    assert_eq!(speaker.microphones_opened.load(Ordering::Relaxed), 0);

    press(&mut panda, "Sound: Somebody joins the room");
    wait_for_switch(&mut panda, "Sound: Somebody joins the room", false);
    let quiet = played(&speaker);
    press(&mut panda, "Play: Somebody joins the room");
    let peak = peak_after(&speaker, quiet, PATIENCE);
    assert!(
        CUE_PEAK.contains(&peak),
        "a sound switched off can still be listened to: peak {peak}"
    );

    let deadline = Instant::now() + Duration::from_secs(15);
    let released = loop {
        panda.tick();
        let before = played(&speaker);
        std::thread::sleep(Duration::from_millis(150));
        if played(&speaker) == before {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
    };
    assert!(released, "the speaker was never let go");
    assert_eq!(speaker.microphones_opened.load(Ordering::Relaxed), 0);
}

#[test]
fn a_broken_microphone_is_named_leads_to_the_settings_and_goes_away_by_itself() {
    let server = TestServer::start("DESKTOP-9FJD");
    let backend = FakeBackend::quiet(48_000, Some((440.0, 0.3)));
    let mut panda = opening(named("Panda")).backend(backend.clone()).open();
    panda.resize(292.0, 720.0);
    enter_general(&mut panda, &server.address(), "Panda");
    wait_for(&mut panda, "[button] Connection details");
    assert!(!panda.screen().contains("not working"));

    backend.microphone_broken.store(true, Ordering::Relaxed);
    wait_for(&mut panda, "Microphone not working");
    let screen = panda.screen();
    assert!(
        screen.contains("It would not open. Trying again..."),
        "{screen}"
    );
    assert!(panda.has("Choose another microphone"), "{screen}");
    assert!(!screen.contains("fake microphone"), "{screen}");
    picture(&panda, "41-microphone-not-working");

    press(&mut panda, "Choose another microphone");
    wait_for_switch(&mut panda, "Audio", true);
    assert_eq!(panda.is_on("General"), Some(false));
    press(&mut panda, "Close");
    wait_for(&mut panda, "Microphone not working");

    backend.microphone_broken.store(false, Ordering::Relaxed);
    wait_until_gone(&mut panda, "Microphone not working");
    assert!(!panda.has("Choose another microphone"));
}

#[test]
fn broken_speakers_are_named_and_go_away_by_themselves() {
    let server = TestServer::start("DESKTOP-9FJD");
    let backend = FakeBackend::quiet(48_000, None);
    let mut panda = opening(named("Panda")).backend(backend.clone()).open();
    enter_general(&mut panda, &server.address(), "Panda");
    wait_for(&mut panda, "[button] Connection details");

    backend.speakers_broken.store(true, Ordering::Relaxed);
    wait_for(&mut panda, "Speakers not working");
    assert!(panda
        .screen()
        .contains("They would not open. Trying again..."));
    assert!(panda.has("Choose other speakers"));
    picture(&panda, "41b-speakers-not-working");

    backend.speakers_broken.store(false, Ordering::Relaxed);
    wait_until_gone(&mut panda, "Speakers not working");
}

#[track_caller]
fn start_the_test(ui: &mut Harness) {
    press(ui, "Quick audio settings");
    press(ui, "Test the microphone");
    wait_until(ui, "the test running", |ui| {
        ui.has("Stop the microphone test")
    });
}

#[test]
fn the_microphone_test_lights_mute_and_deafen() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    start_the_test(&mut panda);
    wait_for_colour(&mut panda, "Unmute microphone", is_red);
    wait_for_colour(&mut panda, "Undeafen", is_red);
    assert!(panda.screen().contains("Panda is deafened"));
    assert!(!panda.screen().contains("do not hear you"));
    picture(&panda, "26b-quick-audio-testing");
}

#[test]
fn the_others_see_a_microphone_test_as_muted_and_deafened() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut people = room_of(&server, &["Panda", "Raccoon"]);
    let (panda, raccoon) = people.split_at_mut(1);
    let (panda, raccoon) = (&mut panda[0], &mut raccoon[0]);
    start_the_test(panda);
    wait_for(raccoon, "Panda is deafened");
    assert!(raccoon.screen().contains("Panda is muted"));

    press(panda, "Stop the microphone test");
    wait_until_gone(raccoon, "Panda is deafened");
    assert!(!raccoon.screen().contains("Panda is muted"));
}

#[test]
fn a_lit_button_ends_the_microphone_test() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    start_the_test(&mut panda);
    press(&mut panda, "Unmute microphone");
    wait_until(&mut panda, "the test stopped", |ui| {
        ui.has("Test the microphone")
    });
    assert!(panda.has("Mute microphone"), "not muted after all");
    assert!(panda.has("Deafen"));
}
