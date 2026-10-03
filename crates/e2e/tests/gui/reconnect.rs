use std::time::{Duration, Instant};

use whisperia::{
    desktop::{DesktopEvent, DesktopLink, Presence},
    wake::Waker,
};
use whisperia_e2e::{
    gui::*,
    net,
    relay::Relay,
    signal::{peak_after, played},
    TestServer,
};

const LOSS_NOTICED: Duration = Duration::from_secs(20);

#[test]
fn cancelling_a_reconnect_goes_back_to_the_servers() {
    let mut server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    server.stop();
    wait_for_within(&mut panda, "Reconnecting", LOSS_NOTICED);
    assert!(
        panda
            .screen()
            .contains("Reconnecting to DESKTOP-AV83 - attempt 1"),
        "the server is called as the room called it:\n{}",
        panda.screen()
    );
    picture(&panda, "07-reconnecting");
    press(&mut panda, "Cancel");
    wait_for(&mut panda, "Connect to DESKTOP-AV83");
    assert!(panda.has_text("Servers"));
    assert!(!panda.wants_to_quit());
}

#[test]
fn the_sound_of_a_lost_connection_outlives_the_room_it_was_played_in() {
    let mut server = TestServer::start("DESKTOP-9FJD");
    let (mut panda, speaker) = window_heard(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    wait_until(&mut panda, "the speaker running", |_| {
        played(&speaker) > 48_000
    });

    let quiet = played(&speaker);
    server.stop();
    wait_for_within(&mut panda, "Reconnecting", LOSS_NOTICED);
    let peak = peak_after(&speaker, quiet, PATIENCE);
    assert!(
        (0.03..0.2).contains(&peak),
        "the loss was not heard: peak {peak}"
    );
}

#[test]
fn a_reconnect_keeps_mute_deafen_and_the_chat() {
    let mut server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    say(&mut panda, "ave 83");
    wait_for_text(&mut panda, "ave 83");
    press(&mut panda, "Mute microphone");
    press(&mut panda, "Deafen");
    wait_for(&mut panda, "Panda is deafened");

    server.stop();
    wait_for_within(&mut panda, "Reconnecting", LOSS_NOTICED);
    server.start_again();
    wait_until_gone_within(&mut panda, "Reconnecting", LOSS_NOTICED);
    wait_for(&mut panda, "Panda (you)");
    assert!(panda.has("Unmute microphone"));
    assert!(panda.has("Undeafen"));
    assert!(panda.has_text("ave 83"), "{}", panda.screen());

    let mut raccoon = window(named("Raccoon"));
    enter_general(&mut raccoon, &server.address(), "Raccoon");
    wait_for(&mut raccoon, "Panda is deafened");
    assert!(raccoon.screen().contains("Panda is muted"));
}

#[test]
fn a_name_held_by_a_lost_connection_is_given_back() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _in_runtime = runtime.enter();
    let server = TestServer::start("DESKTOP-AV83");
    let relay = Relay::to(server.addr());
    let _lost = runtime.block_on(net::join(relay.addr(), "Panda"));
    relay.cut();

    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda (2)");
    wait_for_within(&mut panda, "Panda (you)", LOSS_NOTICED);
}

#[test]
fn the_mute_key_works_while_reconnecting() {
    let mut server = TestServer::start("DESKTOP-AV83");
    let (link, desktop, commands) = DesktopLink::channels(&Waker::default());
    let mut panda = opening(named("Panda")).desktop(link).open();
    enter_general(&mut panda, &server.address(), "Panda");

    server.stop();
    wait_for_within(&mut panda, "Reconnecting", LOSS_NOTICED);
    desktop.send(DesktopEvent::ToggleMute).unwrap();
    wait_for_text(&mut panda, "You come back muted.");
    picture(&panda, "53-reconnecting-muted");
    wait_until(&mut panda, "the tray showing the mute", |_| {
        statuses(&commands)
            .last()
            .is_some_and(|status| status.presence == Presence::Muted)
    });

    server.start_again();
    wait_until_gone_within(&mut panda, "Reconnecting", LOSS_NOTICED);
    wait_for(&mut panda, "Panda (you)");
    assert!(panda.has("Unmute microphone"));
    let mut raccoon = window(named("Raccoon"));
    enter_general(&mut raccoon, &server.address(), "Raccoon");
    wait_for(&mut raccoon, "Panda is muted");
}

#[test]
fn a_full_server_is_tried_again_while_reconnecting() {
    let mut server = TestServer::named("DESKTOP-AV83").max_peers(1).start();
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");

    server.stop();
    wait_for_within(&mut panda, "attempt 2", LOSS_NOTICED);
    server.start_again();
    let mut octopus = window(named("Octopus"));
    reach(&mut octopus, &server.address());

    wait_for_within(&mut panda, "attempt 3", LOSS_NOTICED);
    assert!(panda.screen().contains("full"), "{}", panda.screen());
    octopus.close();
    octopus.tick();
    assert!(octopus.wants_to_quit());
    wait_for_within(&mut panda, "Panda (you)", LOSS_NOTICED);
}

#[test]
fn a_reconnect_does_not_ask_for_the_server_password_again() {
    let mut server = TestServer::named("DESKTOP-P4LW")
        .password("correct_pass")
        .start();
    let mut panda = window(named("Panda"));
    type_into(&mut panda, "Server address", &server.address());
    press(&mut panda, "Connect");
    wait_for(&mut panda, "asks for a password");
    type_into(&mut panda, "Password", "correct_pass");
    press(&mut panda, "Connect with this password");
    wait_for(&mut panda, "[button] New room");
    press(&mut panda, "Open");
    wait_for(&mut panda, "Panda (you)");

    server.stop();
    wait_for_within(&mut panda, "Reconnecting", LOSS_NOTICED);
    server.start_again();
    wait_until_gone_within(&mut panda, "Reconnecting", LOSS_NOTICED);
    wait_for(&mut panda, "Panda (you)");
    assert!(!panda.screen().contains("asks for a password"));
}

#[test]
fn a_locked_room_comes_back_locked_after_the_server_restarts() {
    let mut server = TestServer::start("DESKTOP-6TNC");
    let address = server.address();
    let mut panda = window(named("Panda"));
    reach(&mut panda, &address);
    type_into(&mut panda, "Name of the new room", "Backroom");
    type_into(&mut panda, "Password of the new room", "owl");
    slide(&mut panda, "Size of the new room", 3.0);
    press(&mut panda, "Open");
    wait_for(&mut panda, "Panda (you)");
    let mut raccoon = window(named("Raccoon"));
    reach(&mut raccoon, &address);
    type_into(&mut raccoon, "Password of Backroom", "owl");
    press(&mut raccoon, "Password of Backroom");
    wait_for(&mut raccoon, "Raccoon (you)");

    server.stop();
    wait_for_within(&mut panda, "Reconnecting", LOSS_NOTICED);
    server.start_again();
    let deadline = Instant::now() + LOSS_NOTICED;
    loop {
        panda.tick();
        raccoon.tick();
        if panda.has("Raccoon") && raccoon.has("Panda") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "not back together:\n{}\n{}",
            panda.screen(),
            raccoon.screen()
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let mut octopus = window(named("Octopus"));
    reach(&mut octopus, &address);
    wait_for(&mut octopus, "Backroom asks for a password");
    assert!(!octopus.screen().contains("Panda"), "{}", octopus.screen());
    type_into(&mut octopus, "Password of Backroom", "owl");
    press(&mut octopus, "Enter Backroom");
    wait_for(&mut octopus, "Octopus (you)");
    assert_eq!(
        octopus.screen().matches(" is the host").count(),
        1,
        "{}",
        octopus.screen()
    );

    let mut penguin = window(named("Penguin"));
    reach(&mut penguin, &address);
    type_into(&mut penguin, "Password of Backroom", "owl");
    press(&mut penguin, "Enter Backroom");
    wait_for_text(
        &mut penguin,
        "Full right now. Try again when somebody leaves.",
    );
}
