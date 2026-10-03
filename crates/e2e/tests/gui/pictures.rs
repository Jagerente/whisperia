use whisperia::{audio::fake::FakeBackend, config::Config};
use whisperia_e2e::{gui::*, TestServer};
use whisperia_proto::control::Avatar;

fn pictured() -> bool {
    std::env::var_os("WW_GUI_PICTURES").is_some()
}

#[test]
fn the_first_connect_is_pictured() {
    if !pictured() {
        return;
    }
    let nobody = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut panda = window(named("Panda"));
    type_into(
        &mut panda,
        "Server address",
        &nobody.local_addr().unwrap().to_string(),
    );
    press(&mut panda, "Connect");
    wait_for_text(&mut panda, "Connecting");
    picture(&panda, "07b-connecting");
    press(&mut panda, "Cancel");
    wait_for_text(&mut panda, "Servers");
}

#[test]
fn long_names_are_pictured() {
    if !pictured() {
        return;
    }
    let server = TestServer::start("PANDA-RASPBERRYPI-4B");
    let address = server.address();
    let mut host = quiet(Config {
        avatar: Avatar::Panda,
        ..named("Panda the Wonderful!")
    });
    reach(&mut host, &address);
    type_into(
        &mut host,
        "Name of the new room",
        "Movie night with friends",
    );
    press(&mut host, "Open");
    wait_for(&mut host, "Panda the Wonderful! (you)");
    let mut guest = quiet(Config {
        avatar: Avatar::Mouse,
        ..named("MouseAndFriends")
    });
    reach(&mut guest, &address);
    press(&mut guest, "Enter Movie night with friends");
    wait_for(&mut guest, "MouseAndFriends (you)");
    press(&mut guest, "Mute microphone");
    press(&mut guest, "Deafen");
    share(&mut host, "Screen 1");
    wait_for(&mut guest, "Panda the Wonderful! is the host");
    wait_for(&mut guest, "[button] Watch Panda the Wonderful!'s screen");
    picture(&guest, "41c-long-names");

    press(&mut guest, "Leave the room");
    wait_for(&mut guest, "[button] Disconnect");
    press(&mut guest, "Disconnect");
    wait_for(&mut guest, "Connect to PANDA-RASPBERRYPI-4B");
    wait_until_gone(&mut guest, "Left the room");
    picture(&guest, "39b-a-long-server-name");
}

#[test]
fn notifications_switched_off_are_pictured() {
    if !pictured() {
        return;
    }
    let mut panda = window(named("Panda"));
    press(&mut panda, "Settings");
    press(&mut panda, "Notifications");
    press(&mut panda, "Notification sounds");
    wait_for_switch(&mut panda, "Notification sounds", false);
    picture(&panda, "23b-notifications-off");
}

fn quiet(config: Config) -> Window {
    opening(config)
        .backend(FakeBackend::quiet(48_000, None))
        .open()
}

#[test]
fn a_full_room_is_pictured() {
    if !pictured() {
        return;
    }
    let server = TestServer::start("DESKTOP-6TNC");
    let address = server.address();
    let mut panda = quiet(named("Panda"));
    let mut raccoon = window(named("Raccoon"));
    let mut fox = quiet(named("Fox"));
    let mut mouse = quiet(named("Mouse"));
    let mut penguin = quiet(named("Penguin"));
    enter_general(&mut panda, &address, "Panda");
    for (ui, me) in [
        (&mut raccoon, "Raccoon"),
        (&mut fox, "Fox"),
        (&mut mouse, "Mouse"),
        (&mut penguin, "Penguin"),
    ] {
        enter_general(ui, &address, me);
        wait_until(&mut panda, &format!("{me} in the room"), |ui| ui.has(me));
    }
    for (ui, said) in [
        (&mut raccoon, "Film tonight?"),
        (&mut fox, "Yes, at eight"),
        (&mut penguin, "I will show the trailer"),
    ] {
        say(ui, said);
        wait_for_text(&mut panda, said);
    }
    say(&mut panda, "Popcorn is on me");
    press(&mut fox, "Mute microphone");
    press(&mut mouse, "Deafen");
    share(&mut penguin, "Screen 1");
    wait_for(&mut panda, "[button] Watch what Penguin shows");
    wait_for(&mut panda, "Mouse is deafened");
    wait_for(&mut panda, "Fox is muted");
    picture(&panda, "41-full-room");

    panda.resize(720.0, 520.0);
    wait_until(&mut panda, "the offer in a small window", |ui| {
        ui.has("Watch what Penguin shows")
    });
    picture(&panda, "42-full-room-small");
    panda.resize(1180.0, 760.0);

    let mut octopus = window(named("Octopus"));
    reach(&mut octopus, &address);
    wait_for_text(&mut octopus, "5 people");
    picture(&octopus, "43-full-room-from-outside");
}
