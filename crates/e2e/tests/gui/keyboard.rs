use whisperia_e2e::{gui::*, TestServer};

#[test]
fn tab_reaches_the_buttons_enter_presses_them_and_escape_closes_the_sheet() {
    let mut panda = window(named("Panda"));
    wait_for_text(&mut panda, "Servers");
    for _ in 0..2 {
        panda.tab();
    }
    panda.enter();
    wait_for(&mut panda, "[button] Close");
    assert_eq!(panda.is_on("General"), Some(true));

    for _ in 0..3 {
        panda.tab();
    }
    picture(&panda, "52-focus-ring");
    panda.space();
    wait_for_switch(&mut panda, "Profile", true);

    panda.escape();
    wait_until_gone(&mut panda, "[button] Close");
    assert!(panda.has_text("Servers"));
}

#[test]
fn an_address_is_typed_straight_away_when_no_server_is_found() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    wait_for_text(&mut panda, "Servers");
    panda.type_text(&server.address());
    panda.enter();
    wait_for(&mut panda, "[button] New room");
}

#[test]
fn enter_goes_into_the_room_in_view() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut mouse = window(named("Mouse"));
    enter_general(&mut mouse, &server.address(), "Mouse");
    let mut panda = window(named("Panda"));
    reach(&mut panda, &server.address());
    wait_for(&mut panda, "[button] Enter General");
    panda.enter();
    wait_for(&mut panda, "Panda (you)");
}

#[test]
fn the_first_name_is_typed_straight_away_and_saved_with_enter() {
    let mut panda = window(whisperia::config::Config::default());
    wait_for(&mut panda, "Set up your profile");
    panda.type_text("Panda");
    panda.enter();
    wait_until_gone(&mut panda, "Set up your profile");
    assert_eq!(panda.saved_settings().name, "Panda");
}

#[test]
fn escape_puts_a_popover_away() {
    let server = TestServer::start("DESKTOP-AV83");
    let mut panda = window(named("Panda"));
    enter_general(&mut panda, &server.address(), "Panda");
    press(&mut panda, "Quick audio settings");
    wait_for(&mut panda, "Automatic gain control");
    panda.escape();
    wait_until_gone(&mut panda, "Automatic gain control");
}
