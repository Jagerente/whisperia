use std::net::SocketAddr;

use whisperia::config::{Config, RecentServer};
use whisperia_e2e::{fakes, gui::*, net, TestServer};
use whisperia_proto::control::Name;

const LOOKS_LIKE: &str = "An address looks like 192.168.1.52 or host-pc:47820.";

#[test]
fn a_typed_address_that_is_not_one_is_explained_under_the_field() {
    let mut panda = window(named("Panda"));
    type_into(&mut panda, "Server address", "not an address !");
    press(&mut panda, "Connect");
    wait_for_text(&mut panda, LOOKS_LIKE);
    assert!(panda.notice().is_none(), "no toast for it");
    let (_, field) = middle_of(&panda, "Server address");
    let (_, told) = middle_of(&panda, LOOKS_LIKE);
    assert!(told > field, "the explanation sits under the field");
    wait_for_colour(&mut panda, "Server address", is_red);
    picture(&panda, "02b-not-an-address");

    type_into(&mut panda, "Server address", "192.168.1.52");
    wait_until_text_gone(&mut panda, LOOKS_LIKE);
}

#[test]
fn an_address_nobody_answers_is_explained_under_the_field() {
    let nobody = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = nobody.local_addr().unwrap().to_string();
    let said = format!("Could not connect: server not reachable ({address}: no answer).");
    let mut panda = window(named("Panda"));
    type_into(&mut panda, "Server address", &address);
    press(&mut panda, "Connect");
    wait_for_text(&mut panda, &said);
    assert!(panda.notice().is_none(), "{:?}", panda.notice());
    let (_, field) = middle_of(&panda, "Server address");
    let (_, told) = middle_of(&panda, &said);
    assert!(told > field, "the explanation sits under the field");
    wait_for_colour(&mut panda, "Server address", is_red);
    picture(&panda, "02c-nobody-answers");

    type_into(&mut panda, "Server address", "192.168.1.52");
    wait_until_text_gone(&mut panda, &said);
}

#[test]
fn a_first_visit_says_nothing() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    type_into(&mut panda, "Server address", &server.address());
    press(&mut panda, "Connect");
    wait_for(&mut panda, "[button] New room");
    assert!(panda.notice().is_none(), "{:?}", panda.notice());
}

#[test]
fn a_typed_address_connects_is_remembered_and_forgotten() {
    let server = TestServer::start("DESKTOP-AV83");
    let address = server.address();
    let mut panda = window(named("Panda"));
    type_into(&mut panda, "Server address", &address);
    press(&mut panda, "Connect");
    wait_for(&mut panda, "Nobody is in a room yet");
    assert!(panda.has_text("DESKTOP-AV83"), "{}", panda.screen());
    assert!(panda.has_text(&address), "{}", panda.screen());
    assert!(
        !panda.has_text("Whisperia"),
        "the rooms page is the server's"
    );

    press(&mut panda, "Disconnect");
    wait_for(&mut panda, "Connect to DESKTOP-AV83");
    assert!(
        panda
            .texts_within("Connect to DESKTOP-AV83")
            .contains(&address),
        "{}",
        panda.screen()
    );
    assert_eq!(
        panda.saved_settings().recent[0].address.to_string(),
        address
    );

    press(&mut panda, "Connect to DESKTOP-AV83");
    wait_for(&mut panda, "[button] New room");
    press(&mut panda, "Disconnect");
    wait_for(&mut panda, "Connect to DESKTOP-AV83");
    press(&mut panda, "Forget DESKTOP-AV83");
    wait_until_gone(&mut panda, "Connect to DESKTOP-AV83");
    assert!(panda.saved_settings().recent.is_empty());
}

#[test]
fn a_password_is_asked_for_only_by_a_server_that_wants_one() {
    let server = TestServer::named("DESKTOP-P4LW")
        .password("correct_pass")
        .start();
    let mut panda = window(named("Panda"));
    assert!(!panda.screen().contains("Password"));
    type_into(&mut panda, "Server address", &server.address());
    press(&mut panda, "Connect");
    wait_for(&mut panda, "asks for a password");
    picture(&panda, "08-password");

    type_into(&mut panda, "Password", "wrong_pass");
    assert!(
        !panda.screen().contains("wrong_pass"),
        "a password is not shown"
    );
    press(&mut panda, "Connect with this password");
    wait_for(&mut panda, "That was not the password");
    wait_for_colour(&mut panda, "Password", is_red);
    picture(&panda, "08b-wrong-password");
    type_into(&mut panda, "Password", "correct_pass");
    press(&mut panda, "Connect with this password");
    wait_for(&mut panda, "Nobody is in a room yet");

    let mut raccoon = window(named("Raccoon"));
    type_into(&mut raccoon, "Server address", &server.address());
    press(&mut raccoon, "Connect");
    wait_for(&mut raccoon, "asks for a password");
    raccoon.escape();
    wait_for(&mut raccoon, "Not connected.");
    assert!(!raccoon.wants_to_quit());
}

#[test]
fn a_server_that_shows_another_certificate_is_refused_until_the_user_says_yes() {
    let mut first = TestServer::start("DESKTOP-9FJD");
    let port = first.port();
    let mut panda = window(named("Panda"));
    type_into(&mut panda, "Server address", &first.address());
    press(&mut panda, "Connect");
    wait_for(&mut panda, "[button] New room");
    press(&mut panda, "Disconnect");
    wait_for(&mut panda, "Connect to DESKTOP-9FJD");

    first.stop();
    let _second = TestServer::named("DESKTOP-9FJD").on_port(port).start();
    press(&mut panda, "Connect to DESKTOP-9FJD");
    wait_for(&mut panda, "This server looks different");
    assert!(panda
        .screen()
        .contains("\"DESKTOP-9FJD\" shows a different certificate from last time"));
    let (trust, _) = middle_of(&panda, "Trust the new certificate");
    let (refuse, _) = middle_of(&panda, "Do not connect");
    assert!(
        refuse > trust,
        "the safe answer sits where the eye confirms"
    );
    picture(&panda, "13-another-certificate");
    panda.enter();
    wait_for(&mut panda, "The remembered certificate was kept.");
    assert!(!panda.screen().contains("This server looks different"));

    press(&mut panda, "Connect to DESKTOP-9FJD");
    wait_for(&mut panda, "This server looks different");
    press(&mut panda, "Trust the new certificate");
    wait_for(&mut panda, "New room");
}

fn recent(name: &str, addr: SocketAddr) -> RecentServer {
    RecentServer {
        name: Name::new(name).unwrap(),
        address: addr.into(),
        id: None,
    }
}

struct Scene {
    _up: TestServer,
    _penguin: (whisperia::net::Session, net::Events),
    _older: quinn::Endpoint,
    _nobody: std::net::UdpSocket,
    _runtime: tokio::runtime::Runtime,
}

fn remembered_servers() -> (Window, Scene) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let up = TestServer::start("DESKTOP-9FJD");
    let (penguin, _, penguin_hears) = runtime.block_on(net::connect(up.addr(), "Penguin"));
    net::enter(&penguin, "Room 83");
    let deadline = std::time::Instant::now() + PATIENCE;
    while up.occupancy() != (1, 1) {
        assert!(std::time::Instant::now() < deadline, "{:?}", up.occupancy());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let (older, older_at) = {
        let _in_runtime = runtime.enter();
        fakes::server_from_before_peeks()
    };
    let nobody = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();

    let panda = window(Config {
        recent: vec![
            recent("DESKTOP-9FJD", up.addr()),
            recent("DESKTOP-P4LW", older_at),
            recent("DESKTOP-3MRX", nobody.local_addr().unwrap()),
        ],
        ..named("Panda")
    });
    (
        panda,
        Scene {
            _up: up,
            _penguin: (penguin, penguin_hears),
            _older: older,
            _nobody: nobody,
            _runtime: runtime,
        },
    )
}

const ONLINE: &str = "Online";
const OTHER: &str = "Other version";
const OFFLINE: &str = "Offline";

#[track_caller]
fn wait_for_status(ui: &mut Window, server: &str, status: &str) {
    let line = format!("Connect to {server}");
    wait_until(ui, &format!("{server} {status}"), |ui| {
        ui.texts_within(&line).iter().any(|text| text == status)
    });
}

#[test]
fn remembered_servers_say_whether_they_are_up_asleep_or_older() {
    let (mut panda, _scene) = remembered_servers();
    for server in ["DESKTOP-9FJD", "DESKTOP-P4LW", "DESKTOP-3MRX"] {
        let line = panda.texts_within(&format!("Connect to {server}"));
        assert!(
            line.iter().any(|text| text == "Checking"),
            "{server}: {line:?}"
        );
    }
    assert!(!panda.screen().contains("connected before"));

    wait_for_status(&mut panda, "DESKTOP-9FJD", ONLINE);
    wait_for_status(&mut panda, "DESKTOP-P4LW", OTHER);
    wait_for_status(&mut panda, "DESKTOP-3MRX", OFFLINE);
    let up = panda.texts_within("Connect to DESKTOP-9FJD");
    for said in ["DESKTOP-9FJD", "1 room", "1 person"] {
        assert!(up.iter().any(|text| text == said), "{said}: {up:?}");
    }
    let asleep = panda.texts_within("Connect to DESKTOP-3MRX");
    assert!(!asleep.iter().any(|text| text == "1 person"), "{asleep:?}");

    wait_for_colour(&mut panda, "Connect to DESKTOP-9FJD", |(r, g, b)| {
        g > r + 60 && g > b + 60
    });
    wait_for_colour(&mut panda, "Connect to DESKTOP-P4LW", |(r, g, b)| {
        r > g + 40 && g > b + 60
    });
    wait_for_colour(&mut panda, "Connect to DESKTOP-3MRX", is_grey);
}

#[test]
fn a_server_that_is_not_up_says_so_in_its_row_without_pointing() {
    let (mut panda, _scene) = remembered_servers();
    wait_for_status(&mut panda, "DESKTOP-9FJD", ONLINE);
    wait_for_status(&mut panda, "DESKTOP-P4LW", OTHER);
    wait_for_status(&mut panda, "DESKTOP-3MRX", OFFLINE);
    for (server, status) in [("DESKTOP-P4LW", OTHER), ("DESKTOP-3MRX", OFFLINE)] {
        assert_eq!(panda.count_text(status), 1, "{}", panda.screen());
        let (said, _) = middle_of(&panda, status);
        let (row, _) = middle_of(&panda, &format!("Connect to {server}"));
        assert!(said > row, "{status} sits on the right of the row");
    }
    hover(&mut panda, "Connect to DESKTOP-3MRX");
    assert_eq!(panda.count_text(OFFLINE), 1, "no tip on top of it");
    picture(&panda, "39-servers-in-every-state");
}

#[test]
fn a_server_that_hangs_up_instead_of_answering_is_offline() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (_closing, closing_at) = {
        let _in_runtime = runtime.enter();
        fakes::server_that_hangs_up_when_asked()
    };
    let mut panda = window(Config {
        recent: vec![recent("DESKTOP-9FJD", closing_at)],
        ..named("Panda")
    });
    wait_for_status(&mut panda, "DESKTOP-9FJD", OFFLINE);
}

#[test]
fn a_server_of_another_version_says_which_one_to_update() {
    let (mut panda, _scene) = remembered_servers();
    wait_for_status(&mut panda, "DESKTOP-P4LW", OTHER);
    press(&mut panda, "Connect to DESKTOP-P4LW");
    wait_for_text(
        &mut panda,
        "\"DESKTOP-P4LW\" has another Whisperia version. Update the older one.",
    );
    assert!(panda.notice().is_none(), "{:?}", panda.notice());
    picture(&panda, "40b-another-version");
    panda.enter();
    wait_until_text_gone(
        &mut panda,
        "\"DESKTOP-P4LW\" has another Whisperia version. Update the older one.",
    );
}

#[test]
fn a_full_server_says_so() {
    let server = TestServer::named("DESKTOP-AV83").max_peers(1).start();
    let mut panda = window(named("Panda"));
    reach(&mut panda, &server.address());
    let mut raccoon = window(named("Raccoon"));
    type_into(&mut raccoon, "Server address", &server.address());
    press(&mut raccoon, "Connect");
    wait_for_text(
        &mut raccoon,
        &format!(
            "{} is full. Try again when somebody leaves.",
            server.address()
        ),
    );
    assert!(!raccoon.has("New room"));
}
