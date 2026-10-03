use whisperia::{audio::fake::FakeBackend, gui::Harness};
use whisperia_e2e::{
    gui::*,
    net,
    relay::Relay,
    signal::{is_silent, is_tone, last, played, since},
    TestServer,
};

#[test]
fn deafened_nobody_is_heard() {
    let server = TestServer::start("DESKTOP-AV83");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _raccoon = runtime.block_on(
        net::joining(server.addr(), "Raccoon")
            .audio(FakeBackend::quiet(48_000, Some((880.0, 0.3))))
            .join(),
    );
    let (mut panda, speaker) = window_heard(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    let hears_raccoon = |_: &mut Harness| is_tone(&last(&speaker, 9_600), 48_000, 880.0);
    wait_until(&mut panda, "Raccoon heard", hears_raccoon);

    press(&mut panda, "Deafen");
    wait_until(&mut panda, "the room going quiet", |_| {
        is_silent(&last(&speaker, 4_800))
    });
    let from = played(&speaker);
    wait_until(&mut panda, "half a second deafened", |_| {
        played(&speaker) > from + 24_000
    });
    assert!(is_silent(&since(&speaker, from)), "Raccoon came through");

    press(&mut panda, "Undeafen");
    wait_until(&mut panda, "Raccoon heard again", hears_raccoon);
}

#[test]
fn muted_one_is_heard_by_nobody() {
    let server = TestServer::start("DESKTOP-AV83");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let raccoon = runtime.block_on(net::join(server.addr(), "Raccoon"));
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    let raccoon_hears = |_: &mut Harness| is_tone(&last(&raccoon.audio, 9_600), 48_000, 440.0);
    wait_until(&mut panda, "Raccoon hearing Panda", raccoon_hears);

    press(&mut panda, "Mute microphone");
    wait_until(&mut panda, "Raccoon hearing nothing", |_| {
        is_silent(&last(&raccoon.audio, 4_800))
    });

    press(&mut panda, "Unmute microphone");
    wait_until(&mut panda, "Raccoon hearing Panda again", raccoon_hears);
}

#[test]
fn away_from_a_room_the_window_sleeps() {
    let mut panda = window(named("Panda"));
    wait_for_text(&mut panda, "Servers");
    panda.tick();
    let next = panda.next_tick_in();
    assert!(
        next.is_none_or(|next| next > std::time::Duration::from_secs(1)),
        "woken every {next:?}"
    );
}

#[test]
fn in_a_room_the_window_follows_the_sound_live() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    panda.tick();
    let next = panda.next_tick_in();
    assert!(
        next.is_some_and(|next| next <= whisperia::app::LIVE),
        "woken every {next:?}"
    );
}

#[test]
fn alone_in_a_room_the_connection_is_good_and_sits_by_the_buttons() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    reach(&mut panda, &server.address());
    press(&mut panda, "Open");
    wait_for(&mut panda, "Panda (you)");
    assert!(panda.has_text("General"), "{}", panda.screen());
    assert!(panda.has_text("DESKTOP-AV83"), "{}", panda.screen());
    assert!(!panda.screen().contains("Nobody"));
    wait_for(&mut panda, "Good connection");
    let (x, y) = middle_of(&panda, "Connection details");
    let (_, buttons) = middle_of(&panda, "Mute microphone");
    assert!(
        x < LEFT_SIDE && y < buttons && y > buttons - 120.0,
        "the signal belongs next to the buttons at the bottom left, not at ({x}, {y})"
    );
    picture(&panda, "04-room-alone");
}

#[test]
fn mute_and_deafen_are_shown_and_explained_on_hover() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");

    press(&mut panda, "Mute microphone");
    wait_until(&mut panda, "the microphone muted", |ui| {
        ui.has("Unmute microphone")
    });
    assert!(panda.screen().contains("Panda is muted"));
    press(&mut panda, "Deafen");
    wait_for(&mut panda, "Panda is deafened");
    assert!(panda.screen().contains("Panda is muted"));
    picture(&panda, "18-muted-and-deafened");

    assert!(!panda.has_text("Undeafen"));
    hover(&mut panda, "Undeafen");
    wait_for_text(&mut panda, "Undeafen");
    picture(&panda, "19-tip");
    panda.mouse_leaves();
    wait_until_text_gone(&mut panda, "Undeafen");

    press(&mut panda, "Undeafen");
    press(&mut panda, "Unmute microphone");
    wait_until(&mut panda, "the microphone back on", |ui| {
        ui.has("Mute microphone")
    });
    assert!(!panda.screen().contains("Panda is muted"));
}

#[test]
fn unmute_while_deafened_lets_one_talk_and_hear_again() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    press(&mut panda, "Deafen");
    wait_for(&mut panda, "Panda is deafened");
    assert!(panda.has("Unmute microphone"), "deafened means muted too");
    picture(&panda, "18b-deafened-not-muted");

    press(&mut panda, "Unmute microphone");
    wait_until(&mut panda, "the microphone and the sound back", |ui| {
        ui.has("Mute microphone") && ui.has("Deafen")
    });
    assert!(!panda.screen().contains("Panda is muted"));
    assert!(!panda.screen().contains("Panda is deafened"));
}

#[test]
fn ones_own_line_is_not_a_button() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    hover(&mut panda, "Panda (you)");
    assert!(!panda.has("Change your name and face"));
    panda.press("Panda (you)");
    panda.tick();
    assert!(!panda.has("Your name"), "{}", panda.screen());
}

#[test]
fn a_person_can_be_turned_down() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut room = room_of(&server, &["Panda", "Mouse"]);
    let panda = &mut room[0];
    press(panda, "Mouse");
    slide(panda, "Volume of Mouse", -8.0);
    wait_for_value(panda, "Volume of Mouse", "-8");
    assert!(panda.has_text("-8 dB"), "{}", panda.screen());
}

#[test]
fn a_rename_in_a_room_is_told_to_the_others_but_the_same_name_is_not() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut room = room_of(&server, &["Panda", "Mouse"]);
    let [panda, mouse] = &mut room[..] else {
        unreachable!("two were asked for");
    };
    press(mouse, "Panda");
    wait_until(mouse, "Panda's line open", |ui| ui.has("Volume of Panda"));
    press(panda, "Settings");
    press(panda, "Profile");
    wait_for_value(panda, "Your name", "Panda");
    type_into(panda, "Your name", "Panda");
    press(panda, "Your name");
    assert!(
        !panda.screen().contains("You are now"),
        "the same name is no rename\n{}",
        panda.screen()
    );

    type_into(panda, "Your name", "Codependency");
    press(panda, "Your name");
    press(panda, "Close");
    wait_for(panda, "Codependency (you)");
    wait_for(mouse, "Panda is now Codependency");
    assert_eq!(
        mouse.value_of("Volume of Codependency").as_deref(),
        Some("0"),
        "the open line follows the new name"
    );
}

#[test]
fn connection_details_show_round_trip_loss_and_certificate() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    press(&mut panda, "Connection details");
    wait_for(&mut panda, "Round trip, last 30 seconds");
    let screen = panda.screen();
    assert!(screen.contains("Round trip"), "{screen}");
    assert!(screen.contains("Lost in the last 30 seconds"), "{screen}");
    assert!(
        screen.contains(&format!("DESKTOP-AV83, {}, certificate ", server.address())),
        "{screen}"
    );
    let texts = panda.texts();
    let marks = texts.iter().filter(|text| text.ends_with(" ms")).count();
    assert!(marks >= 3, "the graph has its marks: {texts:?}");
    assert!(panda.has_text("0"), "nothing lost: {texts:?}");
    picture(&panda, "06-numbers");
    panda.escape();
    wait_until_gone(&mut panda, "Round trip, last 30 seconds");
}

#[test]
fn what_gets_lost_on_the_way_out_is_called_so() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _in_runtime = runtime.enter();
    let server = TestServer::start("DESKTOP-AV83");
    let relay = Relay::to(server.addr());
    relay.lose_every_nth_on_the_way_out(5);

    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &relay.addr().to_string(), "Panda");
    press(&mut panda, "Connection details");
    wait_for(&mut panda, "Lost on the way out");
    let screen = panda.screen();
    assert!(!screen.contains("Lost in the last 30 seconds"), "{screen}");
    assert!(!screen.contains("Good"), "{screen}");
    picture(&panda, "06-numbers-lossy");
}
