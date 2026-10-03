use whisperia_e2e::{gui::*, TestServer};

#[test]
fn the_rooms_page_puts_the_list_left_and_the_new_room_right() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    reach(&mut panda, &server.address());
    wait_for(&mut panda, "Nobody is in a room yet");
    assert_eq!(panda.is_on("New room"), Some(true));
    for on_the_left in [
        "New room",
        "Change your name and face",
        "Settings",
        "Disconnect",
    ] {
        let (x, _) = middle_of(&panda, on_the_left);
        assert!(x < LEFT_SIDE, "{on_the_left} at {x}");
    }
    for on_the_right in ["Name of the new room", "Open"] {
        let (x, _) = middle_of(&panda, on_the_right);
        assert!(x > LEFT_SIDE, "{on_the_right} at {x}");
    }
    let (name, _) = middle_of(&panda, "Name of the new room");
    let (password, _) = middle_of(&panda, "Password of the new room");
    assert!(
        (name - password).abs() < 1.0,
        "the name and the password line up: {name} and {password}"
    );
    picture(&panda, "03-rooms");
}

#[test]
fn a_room_from_outside_shows_who_is_in_it() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");

    let mut mouse = window(named("Mouse"));
    reach(&mut mouse, &server.address());
    wait_for(&mut mouse, "[button] Enter General");
    assert_eq!(mouse.is_on("Room General"), Some(true));
    assert_eq!(mouse.is_on("New room"), Some(false));
    assert!(mouse.has_text("1 person"), "{}", mouse.screen());
    assert!(mouse.has_text("Panda"), "{}", mouse.screen());
    assert!(!mouse.has("Name of the new room"));
    let (list, _) = middle_of(&mouse, "Room General");
    let (way_in, _) = middle_of(&mouse, "Enter General");
    assert!(
        list < LEFT_SIDE && way_in > LEFT_SIDE,
        "{list} and {way_in}"
    );
    picture(&mouse, "29-a-room-from-outside");
    press(&mut mouse, "Enter General");
    wait_for(&mut mouse, "Mouse (you)");
    wait_until(&mut panda, "Mouse in the room", |ui| ui.has("Mouse"));
}

#[test]
fn a_new_room_is_opened_by_name() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut mouse = window(named("Mouse"));
    enter_general(&mut mouse, &server.address(), "Mouse");
    let mut panda = window(named("Panda"));
    reach(&mut panda, &server.address());
    wait_for(&mut panda, "[button] Enter General");

    press(&mut panda, "New room");
    wait_until(&mut panda, "the new room chosen", |ui| {
        ui.is_on("New room") == Some(true)
    });
    type_into(&mut panda, "Name of the new room", "Backroom");
    picture(&panda, "30-a-new-room");
    press(&mut panda, "Open");
    wait_for(&mut panda, "Panda (you)");
    assert!(panda.has_text("Backroom"), "{}", panda.screen());
}

#[test]
fn two_rooms_each_show_their_own_people() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    let mut mouse = window(named("Mouse"));
    reach(&mut mouse, &server.address());
    press(&mut mouse, "New room");
    type_into(&mut mouse, "Name of the new room", "Backroom");
    press(&mut mouse, "Open");
    wait_for(&mut mouse, "Mouse (you)");

    let mut fox = window(named("Fox"));
    reach(&mut fox, &server.address());
    wait_for(&mut fox, "[button] Enter Backroom");
    assert_eq!(fox.is_on("Room General"), Some(false));
    assert!(fox.has_text("Mouse") && !fox.has_text("Panda"));
    press(&mut fox, "Room General");
    wait_for(&mut fox, "[button] Enter General");
    assert!(fox.has_text("Panda") && !fox.has_text("Mouse"));
    assert!(!fox.has("Enter Backroom"));
    picture(&fox, "31-two-rooms");

    press(&mut panda, "Leave the room");
    wait_for(&mut panda, "[button] Enter Backroom");
    assert!(!panda.has("Enter General"), "an empty room is gone");
}
