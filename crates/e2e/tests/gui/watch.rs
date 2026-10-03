use std::time::{Duration, Instant};

use whisperia::{desktop::DesktopEvent, screen::fake::FakeScreen};
use whisperia_e2e::{gui::*, TestServer};

#[track_caller]
fn pictures_shown(watcher: &mut Window, other: &mut Window, screen: &FakeScreen, at_least: usize) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        watcher.tick();
        other.tick();
        let shown = screen
            .viewers()
            .first()
            .map(|viewer| viewer.shown().len())
            .unwrap_or_default();
        if shown >= at_least {
            return;
        }
        assert!(Instant::now() < deadline, "only {shown} pictures");
        std::thread::sleep(Duration::from_millis(20));
    }
}

const STOP: &str = "Stop watching Panda";
const VOLUME: &str = "Volume of Panda's screen";

#[test]
fn watching_starts_on_a_keyframe_and_the_sharer_sees_who_watches() {
    let mut two = room_with_a_share();
    assert!(!two.panda.screen().contains("watches your screen"));
    let screen = two.panda_screen.clone();
    wait_until(&mut two.panda, "the share under way", |_| {
        screen.pictures_sent() > 20
    });
    press(&mut two.raccoon, "Watch what Panda shows");
    pictures_shown(&mut two.raccoon, &mut two.panda, &two.raccoon_screen, 20);
    let shown = two.raccoon_screen.viewers()[0].shown();
    assert!(shown[0].0, "the first picture is a keyframe: {shown:?}");
    assert!(
        shown.windows(2).all(|w| w[1].1 == w[0].1 + 1),
        "no picture is missing: {shown:?}"
    );

    wait_for(&mut two.panda, "Raccoon watches your screen");
    assert!(!two.panda.screen().contains("watching"));
    assert!(!two.raccoon.screen().contains("watches your screen"));
    wait_for_colour(
        &mut two.panda,
        "Raccoon watches your screen",
        |(r, g, b)| r > g + 80 && (g - b).abs() < 40,
    );
    picture(&two.panda, "15-watched");

    wait_for(&mut two.raccoon, &format!("[button] {STOP}"));
    let room = two.raccoon.screen();
    assert!(room.contains("Waiting for Panda's picture"), "{room}");
    assert!(two.raccoon.has(VOLUME));
    assert!(two.raccoon.has_text("LIVE"));
    assert!(!two.raccoon.has("Watch Panda's screen"));
    picture(&two.raccoon, "10-watching");
}

#[test]
fn viewer_controls_show_only_under_the_mouse() {
    let mut two = room_with_a_share();
    watch(&mut two.raccoon, "Panda");
    let raccoon = &mut two.raccoon;
    raccoon.mouse_leaves();
    wait_for_brightness(raccoon, STOP, |bright| bright < 60);
    hover(raccoon, "Full screen");
    wait_for_brightness(raccoon, STOP, |bright| bright > 200);
    hover(raccoon, VOLUME);
    wait_for_brightness(raccoon, STOP, |bright| bright > 200);
    raccoon.mouse_leaves();
    wait_for_brightness(raccoon, STOP, |bright| bright < 60);

    let line = |ui: &Window, label: &str| middle_of(ui, label).1;
    let slider = line(raccoon, VOLUME);
    for button in ["Mute Panda's screen", "Full screen", STOP] {
        assert!(
            (line(raccoon, button) - slider).abs() < 1.0,
            "{button} stands in the row of the volume"
        );
    }
}

#[test]
fn screen_volume_mutes_and_comes_back() {
    let mut two = room_with_a_share();
    watch(&mut two.raccoon, "Panda");
    let raccoon = &mut two.raccoon;
    slide(raccoon, VOLUME, -6.0);
    wait_for_value(raccoon, VOLUME, "-6");
    press(raccoon, "Mute Panda's screen");
    wait_until(raccoon, "the screen muted", |ui| {
        ui.has("Unmute Panda's screen")
    });
    assert_eq!(raccoon.value_of(VOLUME).as_deref(), Some("-30"));
    hover(raccoon, "Unmute Panda's screen");
    picture(raccoon, "17-screen-muted");
    raccoon.mouse_leaves();
    press(raccoon, "Unmute Panda's screen");
    wait_for_value(raccoon, VOLUME, "-6");
}

#[test]
fn a_persons_line_turns_their_voice_and_their_screen() {
    let mut two = room_with_a_share();
    watch(&mut two.raccoon, "Panda");
    let raccoon = &mut two.raccoon;
    slide(raccoon, VOLUME, -6.0);
    wait_for_value(raccoon, VOLUME, "-6");
    press(raccoon, "Panda");
    wait_until(raccoon, "Panda's line open", |ui| ui.has("Volume of Panda"));
    assert_eq!(
        raccoon.values_of(VOLUME),
        ["-6", "-6"],
        "the line and the picture turn the same screen"
    );
    slide(raccoon, "Volume of Panda", 4.0);
    wait_for_text(raccoon, "+4 dB");
    picture(raccoon, "12-person-open");
    press(raccoon, "Panda");
    wait_until(raccoon, "Panda's line closed", |ui| {
        !ui.has("Volume of Panda")
    });
    let before = two
        .raccoon_screen
        .viewers()
        .first()
        .map_or(0, |viewer| viewer.shown().len());
    pictures_shown(
        &mut two.raccoon,
        &mut two.panda,
        &two.raccoon_screen,
        before + 10,
    );
}

#[test]
fn full_screen_by_button_esc_and_f11() {
    let mut two = room_with_a_share();
    watch(&mut two.raccoon, "Panda");
    let raccoon = &mut two.raccoon;
    press(raccoon, "Full screen");
    wait_until(raccoon, "full screen", |ui| ui.has("Leave full screen"));
    raccoon.escape();
    wait_until(raccoon, "full screen left by esc", |ui| {
        ui.has("Full screen")
    });
    raccoon.f11();
    wait_until(raccoon, "full screen by f11", |ui| {
        ui.has("Leave full screen")
    });
    assert!(
        !raccoon.has("Leave the room"),
        "the picture has the window to itself"
    );
    raccoon.f11();
    wait_until(raccoon, "full screen left by f11", |ui| {
        ui.has("Leave the room")
    });
}

#[test]
fn hiding_what_is_shared_gives_the_chat_its_room() {
    let mut two = room_with_a_share();
    let raccoon = &mut two.raccoon;
    let (narrow, _) = middle_of(raccoon, "Message");
    press(raccoon, "Hide what is shared");
    wait_until(raccoon, "the offer hidden", |ui| {
        !ui.has("Watch what Panda shows")
    });
    let (wide, _) = middle_of(raccoon, "Message");
    assert!(
        wide < narrow - 100.0,
        "the chat did not get its room back: {wide} against {narrow}"
    );
    picture(raccoon, "36-chat-alone");
    press(raccoon, "Show what is shared");
    wait_for(raccoon, "[button] Watch what Panda shows");
    press(raccoon, "Hide what is shared");
    wait_until(raccoon, "the offer hidden", |ui| {
        !ui.has("Watch what Panda shows")
    });
    press(raccoon, "Watch Panda's screen");
    wait_for(raccoon, &format!("[button] {STOP}"));
    wait_for(&mut two.panda, "Raccoon watches your screen");
}

#[test]
fn stopping_watching_is_told_to_the_sharer() {
    let mut two = room_with_a_share();
    watch(&mut two.raccoon, "Panda");
    wait_for(&mut two.panda, "Raccoon watches your screen");
    press(&mut two.raccoon, STOP);
    wait_until_gone(&mut two.panda, "Raccoon watches your screen");
    wait_for(&mut two.raccoon, "[button] Watch what Panda shows");
}

#[test]
fn a_share_that_stops_under_a_full_screen_watcher_says_so() {
    let mut two = room_with_a_share();
    watch(&mut two.raccoon, "Panda");
    press(&mut two.raccoon, "Full screen");
    wait_until(&mut two.raccoon, "full screen", |ui| {
        ui.has("Leave full screen")
    });
    two.desktop.send(DesktopEvent::ToggleShare).unwrap();
    wait_until(&mut two.panda, "the share stopped", |ui| {
        !ui.has("Stop sharing my screen")
    });
    wait_for_text(&mut two.raccoon, "Panda stopped sharing");
    assert!(two.raccoon.has("Leave the room"), "full screen is left");
    wait_until(&mut two.raccoon, "the picture gone", |ui| !ui.has(STOP));
    wait_until_text_gone(&mut two.raccoon, "Panda stopped sharing");
    wait_for(&mut two.raccoon, "Panda stopped sharing their screen");
}

#[test]
fn only_one_screen_is_watched_at_a_time() {
    let server = TestServer::start("DESKTOP-HB21");
    let mut room = room_of(&server, &["Panda", "Raccoon", "Fox"]);
    let [panda, raccoon, fox] = &mut room[..] else {
        unreachable!("three were asked for");
    };
    share(panda, "Screen 1");
    share(fox, "Screen 1");

    wait_for(raccoon, "[button] Watch what Fox shows");
    wait_for(raccoon, "[button] Watch what Panda shows");
    let big = middle_of(raccoon, "Watch what Fox shows").1;
    picture(raccoon, "33-two-offers");

    press(raccoon, "Watch what Panda shows");
    wait_for(raccoon, "[button] Stop watching Panda");
    wait_for(panda, "Raccoon watches your screen");
    let small = middle_of(raccoon, "Watch what Fox shows").1;
    assert!(
        small > big + 200.0,
        "Fox's offer stayed beside the picture: {small} against {big}"
    );
    picture(raccoon, "34-one-watched");

    share(raccoon, "Screen 1");
    assert!(
        raccoon.has("Stop watching Panda"),
        "sharing does not stop watching"
    );
    assert_eq!(middle_of(raccoon, "Watch what Fox shows").1, small);

    press(raccoon, "Watch what Fox shows");
    wait_for(raccoon, "[button] Stop watching Fox");
    assert!(!raccoon.has("Stop watching Panda"));
    assert!(raccoon.has("Watch what Panda shows"));
    wait_until_gone(panda, "Raccoon watches your screen");
    wait_for(fox, "Raccoon watches your screen");
    picture(raccoon, "35-switched");

    press(raccoon, "Stop watching Fox");
    wait_until_gone(fox, "Raccoon watches your screen");
    wait_for(raccoon, "[button] Watch what Fox shows");
    assert!(middle_of(raccoon, "Watch what Fox shows").1 < small - 200.0);
}

#[test]
fn a_screen_that_cannot_be_shown_says_why() {
    let mut two = room_with_a_share();
    two.raccoon_screen
        .cannot_view
        .store(true, std::sync::atomic::Ordering::Relaxed);
    press(&mut two.raccoon, "Watch what Panda shows");
    wait_for_text(&mut two.raccoon, "Cannot watch: this PC has no Direct3D 12");
    assert!(two.raccoon.has("Watch what Panda shows"));
    assert!(!two.raccoon.has(STOP));
}
