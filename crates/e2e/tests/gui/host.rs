use std::time::{Duration, Instant};

use whisperia_e2e::{gui::*, TestServer};

const NO_LIMIT: &str = "No limit";
const FULL_NOW: &str = "Full right now. Try again when somebody leaves.";
const FULL: &str = "Full: 3 of 3 people";
const KICK_FOX: &str = "Kick Fox? They can come back unless the room asks for a password.";

fn host_of_backroom(server: &TestServer, password: Option<&str>, size: f32) -> Window {
    let mut panda = window(named("Panda"));
    reach(&mut panda, &server.address());
    type_into(&mut panda, "Name of the new room", "Backroom");
    if let Some(password) = password {
        type_into(&mut panda, "Password of the new room", password);
    }
    slide(&mut panda, "Size of the new room", size);
    press(&mut panda, "Open");
    wait_for(&mut panda, "Panda (you)");
    panda
}

fn guest_of_backroom(server: &TestServer, name: &str, password: Option<&str>) -> Window {
    let mut guest = window(named(name));
    reach(&mut guest, &server.address());
    press(&mut guest, "Room Backroom");
    if let Some(password) = password {
        type_into(&mut guest, "Password of Backroom", password);
    }
    press(&mut guest, "Enter Backroom");
    wait_for(&mut guest, &format!("{name} (you)"));
    guest
}

#[test]
fn a_new_room_can_have_a_password_and_a_size() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    reach(&mut panda, &server.address());
    type_into(&mut panda, "Name of the new room", "Backroom");
    let (lock_x, lock_y) = middle_of(&panda, "The new room has no password");
    let (field_x, field_y) = middle_of(&panda, "Password of the new room");
    assert!(
        lock_x < field_x && (lock_y - field_y).abs() < 1.0,
        "an open lock stands before the empty password: ({lock_x}, {lock_y}) and ({field_x}, {field_y})"
    );
    wait_for_colour(&mut panda, "The new room has no password", is_grey);

    type_into(&mut panda, "Password of the new room", "owl");
    wait_for_text(&mut panda, "The new room asks for a password");
    wait_for_colour(&mut panda, "The new room asks for a password", is_red);
    assert!(!panda.screen().contains("owl"), "a password is not shown");

    slide(&mut panda, "Size of the new room", 2.0);
    wait_for_value(&mut panda, "Size of the new room", "2");
    assert!(panda.has_text("2 people"));
    slide(&mut panda, "Size of the new room", 26.0);
    wait_for_value(&mut panda, "Size of the new room", "26");
    assert!(panda.has_text(NO_LIMIT), "past 25 there is no limit");
    slide(&mut panda, "Size of the new room", 2.0);
    wait_for_value(&mut panda, "Size of the new room", "2");
    picture(&panda, "44-a-new-room-with-a-password");

    press(&mut panda, "Open");
    wait_for(&mut panda, "Panda (you)");
    assert!(panda.screen().contains("Panda is the host"));
    let (x, y) = middle_of(&panda, "Room settings");
    let (name_x, name_y) = middle_of(&panda, "Panda (you)");
    assert!(
        x > 200.0 && x < LEFT_SIDE && y < name_y && name_x < x,
        "the room's own settings sit at the top right of the left side, opposite its name: ({x}, {y})"
    );
}

#[test]
fn a_locked_room_hides_who_is_inside_and_asks_for_its_password() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = host_of_backroom(&server, Some("owl"), 2.0);
    let mut mouse = window(named("Mouse"));
    reach(&mut mouse, &server.address());
    press(&mut mouse, "Room Backroom");
    wait_until(&mut mouse, "a field for the password", |ui| {
        ui.has("Password of Backroom")
    });
    let outside = mouse.screen();
    assert!(
        outside.contains("Backroom asks for a password"),
        "{outside}"
    );
    assert!(outside.contains("Asks for a password"), "{outside}");
    for hidden in ["Panda", "1 person", "1 of 2 people", "1/2"] {
        assert!(!outside.contains(hidden), "{hidden}: {outside}");
    }
    wait_for_colour(&mut mouse, "Backroom asks for a password", is_red);
    picture(&mouse, "45-a-locked-room-from-outside");

    press(&mut mouse, "Enter Backroom");
    assert!(
        !mouse.screen().contains("Entering..."),
        "without a password nothing is tried"
    );
    type_into(&mut mouse, "Password of Backroom", "cat");
    press(&mut mouse, "Enter Backroom");
    wait_for(&mut mouse, "That was not the password.");
    picture(&mouse, "46-a-wrong-password");
    type_into(&mut mouse, "Password of Backroom", "owl");
    press(&mut mouse, "Enter Backroom");
    wait_for(&mut mouse, "Mouse (you)");
    wait_for(&mut mouse, "Panda is the host");
    assert!(!mouse.has("Room settings"), "only the host sets the room");
    wait_until(&mut panda, "Mouse in the room", |ui| ui.has("Mouse"));
}

#[test]
fn a_full_room_refuses_and_says_so() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = host_of_backroom(&server, Some("owl"), 2.0);
    let _mouse = guest_of_backroom(&server, "Mouse", Some("owl"));

    let mut fox = window(named("Fox"));
    reach(&mut fox, &server.address());
    press(&mut fox, "Room Backroom");
    type_into(&mut fox, "Password of Backroom", "owl");
    press(&mut fox, "Enter Backroom");
    wait_for_text(&mut fox, FULL_NOW);
    assert!(fox.has("Enter Backroom"), "still before the room");
    assert!(
        !fox.has_text("Full: 2 of 2 people"),
        "a locked room hides its people"
    );
    picture(&fox, "48b-a-locked-room-turns-out-full");

    press(&mut panda, "Room settings");
    press(&mut panda, "Remove the password");
    wait_for(&mut fox, "Full, 2 of 2 people");
    let screen = fox.screen();
    assert!(screen.contains("Full: 2 of 2 people"), "{screen}");
    assert!(screen.contains("Panda is the host"), "{screen}");
    assert!(!screen.contains("Backroom asks for a password"), "{screen}");
    wait_for_colour(&mut fox, "Full, 2 of 2 people", is_red);
    picture(&fox, "48-a-full-room-from-outside");
    press(&mut fox, "Enter Backroom");
    assert!(
        !fox.screen().contains("Entering..."),
        "a full room is not tried"
    );
}

#[test]
fn the_host_changes_and_removes_the_password() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = host_of_backroom(&server, Some("owl"), 2.0);
    let mut mouse = guest_of_backroom(&server, "Mouse", Some("owl"));

    press(&mut panda, "Room settings");
    wait_until(&mut panda, "the room settings", |ui| {
        ui.has("Remove the password")
    });
    assert!(panda.has_text("The room asks for a password"));
    assert!(panda.has_text("New password"), "{}", panda.screen());
    assert_eq!(panda.value_of("Room size").as_deref(), Some("2"));
    wait_for_colour(&mut panda, "The room asks for a password", is_red);
    picture(&panda, "47-the-room-settings");

    assert!(panda.tap("Room password"));
    type_into(&mut panda, "Room password", "fox");
    wait_until(&mut panda, "a way to set the password", |ui| {
        ui.has("Enter")
    });
    picture(&panda, "47c-a-new-password-typed");
    press(&mut panda, "Enter");
    wait_for_text(&mut panda, "The room asks for a new password now");
    assert!(
        !panda
            .notice()
            .is_some_and(|notice| notice.contains("password")),
        "told in the chat, not in a toast"
    );
    assert!(!panda.has("Enter"), "the field lets go once it is set");
    assert!(panda.has_text("New password"));
    type_into(&mut panda, "Room password", "x");
    assert!(
        !panda.has("Enter"),
        "the field let go once the password was set"
    );
    type_into(&mut panda, "Room password", "");

    assert!(panda.has_text("Password"), "the row says what it is");
    assert!(panda.has_text("Room size"), "the row says what it is");
    let (remove, remove_y) = middle_of(&panda, "Remove the password");
    let (field, field_y) = middle_of(&panda, "Room password");
    assert!(
        remove > field && remove_y < field_y,
        "the way to remove the password is written above the field, on the right: ({remove}, {remove_y}) and ({field}, {field_y})"
    );
    picture(&panda, "47d-the-password-can-be-removed");

    assert!(panda.tap("Room password"));
    type_into(&mut panda, "Room password", "y");
    wait_until(&mut panda, "a way to set the password", |ui| {
        ui.has("Enter")
    });
    press(&mut panda, "Remove the password");
    wait_for_text(&mut panda, "The room has no password");
    assert!(panda.has_text("No password"), "{}", panda.screen());
    assert!(!panda.has("Remove the password"));
    assert!(!panda.has("Enter"), "what was typed is let go");
    wait_for_colour(&mut panda, "The room has no password", is_grey);
    picture(&panda, "47b-the-room-settings-open");
    wait_for(&mut mouse, "The room no longer asks for a password");
}

#[test]
fn the_host_changes_the_size_and_the_others_are_told() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = host_of_backroom(&server, None, 2.0);
    let mut mouse = guest_of_backroom(&server, "Mouse", None);
    let mut fox = window(named("Fox"));
    reach(&mut fox, &server.address());
    wait_for(&mut fox, "Full, 2 of 2 people");

    press(&mut panda, "Room settings");
    assert_eq!(panda.value_of("Room size").as_deref(), Some("2"));
    assert!(panda.tap("Room password"));
    type_into(&mut panda, "Room password", "z");
    wait_until(&mut panda, "a way to set the password", |ui| {
        ui.has("Enter")
    });
    slide(&mut panda, "Room size", 26.0);
    assert!(
        !panda.has("Enter"),
        "touching the slider lets go of the password"
    );
    assert!(panda.has_text("No password"));
    assert_eq!(
        panda.value_of("Room size").as_deref(),
        Some("26"),
        "the slider stays where it was let go while the server has not answered yet"
    );
    assert!(panda.has_text(NO_LIMIT));
    wait_for(&mut panda, "Any number of people can be in the room now");
    assert_eq!(panda.value_of("Room size").as_deref(), Some("26"));
    wait_for(&mut mouse, "Any number of people can be in the room now");
    wait_for_text(&mut fox, "2 people");
    assert!(!fox.screen().contains("Full"), "{}", fox.screen());

    slide(&mut panda, "Room size", 3.0);
    wait_for(&mut mouse, "At most 3 people can be in the room now");
    wait_for(&mut fox, "2 of 3 people");
}

#[test]
fn the_crown_is_handed_over_and_comes_back_when_the_heir_leaves() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = host_of_backroom(&server, None, 26.0);
    let mut mouse = guest_of_backroom(&server, "Mouse", None);
    let mut fox = guest_of_backroom(&server, "Fox", None);
    wait_until(&mut panda, "Mouse in the room", |ui| ui.has("Mouse"));

    press(&mut panda, "Mouse");
    press(&mut panda, "Make Mouse the host");
    wait_for_text(
        &mut panda,
        "Make Mouse the host? Only they can hand it back.",
    );
    assert!(
        !mouse.screen().contains("You are the host now"),
        "nothing is handed over before the answer"
    );
    picture(&panda, "49b-the-crown-asked-about");
    press(&mut panda, "Keep the crown");
    wait_until_text_gone(
        &mut panda,
        "Make Mouse the host? Only they can hand it back.",
    );
    press(&mut panda, "Make Mouse the host");
    press(&mut panda, "Hand the crown to Mouse");
    wait_for(&mut mouse, "You are the host now");
    assert!(mouse.screen().contains("Mouse is the host"));
    wait_until(&mut mouse, "the room settings for the new host", |ui| {
        ui.has("Room settings")
    });
    wait_until(&mut panda, "the room settings gone", |ui| {
        !ui.has("Room settings")
    });
    wait_for(&mut fox, "Mouse is the host now");
    picture(&mouse, "49-the-crown-handed-over");

    press(&mut mouse, "Leave the room");
    wait_for(&mut panda, "You are the host now");
    wait_until(&mut panda, "the room settings back", |ui| {
        ui.has("Room settings")
    });
    assert!(
        !panda.has("Room size"),
        "the crown coming back does not open the panel by itself"
    );
    wait_for(&mut fox, "Panda is the host now");
}

#[test]
fn the_room_settings_close_when_the_crown_is_handed_over() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = host_of_backroom(&server, None, 26.0);
    let mut mouse = guest_of_backroom(&server, "Mouse", None);
    wait_until(&mut panda, "Mouse in the room", |ui| ui.has("Mouse"));
    press(&mut panda, "Room settings");
    wait_until(&mut panda, "the room settings", |ui| ui.has("Room size"));

    press(&mut panda, "Mouse");
    press(&mut panda, "Make Mouse the host");
    press(&mut panda, "Hand the crown to Mouse");
    wait_until(&mut panda, "the room settings gone", |ui| {
        !ui.has("Room settings")
    });
    assert!(!panda.has("Room size"), "{}", panda.screen());

    press(&mut mouse, "Leave the room");
    wait_until(&mut panda, "the crown back", |ui| ui.has("Room settings"));
    assert!(
        !panda.has("Room size"),
        "the panel stays closed when the crown comes back"
    );
}

#[test]
fn two_racing_for_the_last_seat_one_gets_it() {
    let server = TestServer::start("DESKTOP-AV83");
    let _panda = host_of_backroom(&server, None, 3.0);
    let _fox = guest_of_backroom(&server, "Fox", None);
    let mut octopus = window(named("Octopus"));
    reach(&mut octopus, &server.address());
    let mut mouse = window(named("Mouse"));
    reach(&mut mouse, &server.address());
    wait_for(&mut mouse, "2 of 3 people");
    wait_for(&mut octopus, "2 of 3 people");

    assert!(octopus.press("Enter Backroom") && mouse.press("Enter Backroom"));
    let deadline = Instant::now() + PATIENCE;
    let outside = loop {
        octopus.tick();
        mouse.tick();
        let (y, b) = (octopus.screen(), mouse.screen());
        match (y.contains("Octopus (you)"), b.contains("Mouse (you)")) {
            (true, false) if b.contains(FULL) && !b.contains("Entering...") => break b,
            (false, true) if y.contains(FULL) && !y.contains("Entering...") => break y,
            (true, true) => panic!("both got the last seat:\n{y}\n{b}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "{y}\n{b}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        outside.contains("[button] Enter Backroom") && outside.contains(FULL),
        "the one left outside is back at the list, not stuck entering:\n{outside}"
    );
    assert!(!outside.contains("Entering..."), "{outside}");
}

#[test]
fn the_host_kicks_someone_after_asking_and_they_may_come_back() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut room = room_of(&server, &["Panda", "Mouse", "Fox"]);
    let [panda, mouse, fox] = &mut room[..] else {
        unreachable!("three were asked for");
    };

    press(mouse, "Fox");
    wait_until(mouse, "Fox's line open", |ui| ui.has("Volume of Fox"));
    assert!(!mouse.has("Kick Fox from the room"), "only the host kicks");

    press(panda, "Fox");
    press(panda, "Kick Fox from the room");
    wait_for_text(panda, KICK_FOX);
    let (asked, _) = middle_of(panda, "Kick Fox");
    assert!(asked < LEFT_SIDE, "asked in Fox's line, not in a sheet");
    assert!(!panda.has("Close"), "no sheet over the room");
    picture(panda, "50-kick-someone");
    press(panda, "Keep Fox in the room");
    wait_until_text_gone(panda, KICK_FOX);
    assert!(fox.screen().contains("Fox (you)"));

    press(panda, "Kick Fox from the room");
    wait_for_text(panda, KICK_FOX);
    press(panda, "Kick Fox");
    wait_for(fox, "Panda kicked you from \"General\".");
    assert!(fox.has("Enter General"));
    assert!(!fox.screen().contains("Fox (you)"));
    picture(fox, "51-kicked");
    wait_for(mouse, "Panda kicked Fox");
    wait_for(panda, "You kicked Fox");
    assert!(!panda.has("Fox"));
    assert!(!panda.has_text(KICK_FOX));

    press(fox, "Enter General");
    wait_for(fox, "Fox (you)");
    wait_for(panda, "Fox joined");

    if !panda.has("Kick Fox from the room") {
        press(panda, "Fox");
    }
    press(panda, "Kick Fox from the room");
    wait_for_text(panda, KICK_FOX);
    press(fox, "Leave the room");
    wait_until_text_gone(panda, KICK_FOX);
}
