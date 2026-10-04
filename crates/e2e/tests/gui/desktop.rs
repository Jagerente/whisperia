use std::{
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

use whisperia::{
    config::Paths,
    desktop::{DesktopCommand, DesktopEvent, DesktopLink, DesktopStatus, Presence},
    instance::{self, Claim},
    wake::Waker,
};
use whisperia_e2e::{gui::*, TempDir, TestServer};

#[track_caller]
fn the_tray_shows(commands: &Receiver<DesktopCommand>, wanted: Presence) -> DesktopStatus {
    let deadline = Instant::now() + PATIENCE;
    let mut last = None;
    while Instant::now() < deadline {
        last = statuses(commands).pop().or(last);
        if let Some(status) = last.as_ref().filter(|s| s.presence == wanted) {
            return status.clone();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the tray never showed {wanted:?}, last: {last:?}");
}

const SOONER_THAN_A_TIMEOUT: Duration = Duration::from_secs(4);

#[test]
fn quitting_from_the_tray_leaves_the_room_at_once() {
    let mut two = two_with_screens();
    two.desktop.send(DesktopEvent::Quit).unwrap();
    wait_until(&mut two.panda, "the window quitting", |ui| {
        ui.wants_to_quit()
    });
    wait_until_within(
        &mut two.raccoon,
        "Panda leaving",
        SOONER_THAN_A_TIMEOUT,
        |ui| ui.has_text("Panda left"),
    );
}

#[test]
fn quitting_lets_the_tray_go() {
    let (link, desktop, commands) = DesktopLink::channels(&Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();
    desktop.send(DesktopEvent::Quit).unwrap();
    wait_until(&mut panda, "the window quitting", |ui| ui.wants_to_quit());
    while commands.try_recv().is_ok() {}
    assert_eq!(
        commands.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    );
}

#[test]
fn the_tray_and_hotkeys_mute_and_deafen_and_the_tray_shows_it() {
    let server = TestServer::start("DESKTOP-R8ZE");
    let (link, desktop, commands) = DesktopLink::channels(&Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();

    desktop.send(DesktopEvent::ToggleMute).unwrap();
    panda.tick();
    the_tray_shows(&commands, Presence::Away);
    enter_general(&mut panda, &server.address(), "Panda");
    assert!(
        panda.has("Mute microphone"),
        "a mute before the room does not carry into it"
    );
    the_tray_shows(&commands, Presence::Live);

    desktop.send(DesktopEvent::ToggleMute).unwrap();
    wait_until(&mut panda, "the microphone muted", |ui| {
        ui.has("Unmute microphone")
    });
    the_tray_shows(&commands, Presence::Muted);
    desktop.send(DesktopEvent::ToggleDeafen).unwrap();
    wait_for(&mut panda, "Panda is deafened");
    the_tray_shows(&commands, Presence::Deafened);
    desktop.send(DesktopEvent::ToggleDeafen).unwrap();
    desktop.send(DesktopEvent::ToggleMute).unwrap();
    wait_until(&mut panda, "the microphone back on", |ui| {
        ui.has("Mute microphone")
    });
    the_tray_shows(&commands, Presence::Live);
}

#[test]
fn the_tray_shows_a_microphone_test_as_deafened_and_the_mute_key_ends_it() {
    let server = TestServer::start("DESKTOP-R8ZE");
    let (link, desktop, commands) = DesktopLink::channels(&Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();
    enter_general(&mut panda, &server.address(), "Panda");
    the_tray_shows(&commands, Presence::Live);

    press(&mut panda, "Quick audio settings");
    press(&mut panda, "Test the microphone");
    the_tray_shows(&commands, Presence::Deafened);
    desktop.send(DesktopEvent::ToggleMute).unwrap();
    wait_until(&mut panda, "the test stopped", |ui| {
        ui.has("Test the microphone")
    });
    assert!(panda.has("Mute microphone"), "not muted after all");
    the_tray_shows(&commands, Presence::Live);
}

#[test]
fn a_hotkey_problem_is_shown() {
    let (link, desktop, _commands) = DesktopLink::channels(&Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();
    desktop
        .send(DesktopEvent::Problem("Ctrl+Alt+M is taken".into()))
        .unwrap();
    wait_for(&mut panda, "Ctrl+Alt+M is taken");
}

#[test]
fn the_tray_opens_the_settings_shows_the_window_and_quits() {
    let (link, desktop, _commands) = DesktopLink::channels(&Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();

    desktop.send(DesktopEvent::OpenSettings).unwrap();
    wait_for_switch(&mut panda, "General", true);
    assert!(
        panda
            .screen()
            .contains("Minimize Whisperia to the system tray"),
        "{}",
        panda.screen()
    );
    press(&mut panda, "Close");
    wait_until_gone(&mut panda, "[button] General");

    panda.hide();
    panda.tick();
    desktop.send(DesktopEvent::ShowWindow).unwrap();
    wait_until(&mut panda, "the window shown again", |ui| ui.is_shown());
    assert!(!panda.wants_to_quit());
    desktop.send(DesktopEvent::Quit).unwrap();
    wait_until(&mut panda, "the program quitting", |ui| ui.wants_to_quit());
}

#[test]
fn settings_from_the_tray_cover_the_connection_details_and_give_them_back() {
    let server = TestServer::start("DESKTOP-R8ZE");
    let (link, desktop, _commands) = DesktopLink::channels(&Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();
    enter_general(&mut panda, &server.address(), "Panda");
    wait_for(&mut panda, "[button] Connection details");
    press(&mut panda, "Connection details");
    wait_for(&mut panda, "Round trip, last 30 seconds");

    desktop.send(DesktopEvent::OpenSettings).unwrap();
    wait_for_switch(&mut panda, "General", true);
    assert!(
        !panda.screen().contains("Round trip, last 30 seconds"),
        "one sheet at a time: {}",
        panda.screen()
    );
    press(&mut panda, "Close");
    wait_until_gone(&mut panda, "[button] General");
    wait_for(&mut panda, "Round trip, last 30 seconds");
}

#[test]
fn settings_from_the_tray_put_the_quick_audio_panel_aside_while_they_are_open() {
    let server = TestServer::start("DESKTOP-R8ZE");
    let (link, desktop, _commands) = DesktopLink::channels(&Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();
    enter_general(&mut panda, &server.address(), "Panda");
    press(&mut panda, "Quick audio settings");
    wait_until(&mut panda, "the quick audio panel", |ui| {
        ui.has("Test the microphone")
    });

    desktop.send(DesktopEvent::OpenSettings).unwrap();
    wait_for_switch(&mut panda, "General", true);
    press(&mut panda, "Audio");
    wait_for_switch(&mut panda, "Audio", true);
    assert!(!panda.has_text("Microphone test"), "{}", panda.screen());
    assert!(
        panda.has("Test the microphone"),
        "one test button, the settings' own"
    );
    slide(&mut panda, "Speaker volume", 150.0);
    wait_for_value(&mut panda, "Speaker volume", "150");

    press(&mut panda, "Close");
    wait_until(&mut panda, "the quick audio panel back", |ui| {
        ui.has("Test the microphone")
    });
    assert_eq!(panda.value_of("Speaker volume").as_deref(), Some("150"));
}

#[test]
fn starting_it_again_brings_up_the_window_already_running() {
    let dir = TempDir::new("again-panda");
    let paths = Paths::resolve(Some(dir.path().to_owned())).unwrap();
    let Ok(Claim::First(first)) = instance::claim(&paths.config, &Waker::default()) else {
        panic!("the first start does not get the settings");
    };
    let mut panda = opening(named("Panda")).instance(first).in_dir(dir).open();
    panda.hide();
    panda.tick();
    assert!(!panda.is_shown());

    let elsewhere = TempDir::new("again-penguin");
    let elsewhere = Paths::resolve(Some(elsewhere.path().to_owned())).unwrap();
    let Ok(Claim::First(_penguin)) = instance::claim(&elsewhere.config, &Waker::default()) else {
        panic!("other settings are another copy's to take");
    };
    panda.tick();
    assert!(!panda.is_shown());

    assert!(matches!(
        instance::claim(&paths.config, &Waker::default()),
        Ok(Claim::Elsewhere)
    ));
    wait_until(&mut panda, "the window coming up", |ui| ui.is_shown());
    assert!(panda.has_text("Panda"), "{}", panda.screen());
}
