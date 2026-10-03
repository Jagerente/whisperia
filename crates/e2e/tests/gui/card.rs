use whisperia_e2e::{gui::*, TestServer};

const SIDEBAR: f32 = 292.0;
const KICK_RACCOON: &str = "Kick Raccoon? They can come back unless the room asks for a password.";
const GONE: &str = "Not in the room any more";

fn raccoon_said_hello() -> (TestServer, Window, Window) {
    let server = TestServer::start("DESKTOP-6TNC");
    let mut room = room_of(&server, &["Panda", "Raccoon"]);
    let mut raccoon = room.pop().expect("two were asked for");
    let mut panda = room.pop().expect("two were asked for");
    say(&mut raccoon, "Hello World!");
    wait_for_text(&mut panda, "Hello World!");
    (server, panda, raccoon)
}

#[track_caller]
fn picture_of(ui: &Window, name: &str) -> String {
    let mut found = ui.labels_starting_with(&format!("{name}, "));
    assert_eq!(
        found.len(),
        1,
        "one group of messages under {name}\n{}",
        ui.screen()
    );
    found.pop().expect("one was found")
}

#[track_caller]
fn open_card(ui: &mut Window, name: &str) {
    let picture = picture_of(ui, name);
    press(ui, &picture);
}

#[test]
fn the_picture_of_a_sender_opens_their_card_beside_the_chat() {
    let (_server, mut panda, _raccoon) = raccoon_said_hello();
    open_card(&mut panda, "Raccoon");
    wait_until(&mut panda, "Raccoon's card", |ui| {
        ui.has("Volume of Raccoon")
    });
    let (x, _) = middle_of(&panda, "Volume of Raccoon");
    assert!(
        x > SIDEBAR,
        "the card opens by the message, not in the sidebar"
    );
    picture(&panda, "11f-person-card");
}

#[test]
fn the_name_of_a_sender_opens_their_card_like_their_picture() {
    let (_server, mut panda, _raccoon) = raccoon_said_hello();
    let (x, _, width, _) = panda
        .bounds(&picture_of(&panda, "Raccoon"))
        .expect("the picture is shown");
    let (name_x, name_y, name_width, name_height) = panda
        .text_bounds("Raccoon")
        .into_iter()
        .find(|(name_x, ..)| *name_x > x + width)
        .expect("the name stands beside the picture");
    panda.tap_at(name_x + name_width / 2.0, name_y + name_height / 2.0);
    wait_until(&mut panda, "Raccoon's card", |ui| {
        ui.has("Volume of Raccoon")
    });
}

#[test]
fn escape_closes_the_card() {
    let (_server, mut panda, _raccoon) = raccoon_said_hello();
    open_card(&mut panda, "Raccoon");
    wait_until(&mut panda, "Raccoon's card", |ui| {
        ui.has("Volume of Raccoon")
    });
    panda.escape();
    wait_until(&mut panda, "the card closed", |ui| {
        !ui.has("Volume of Raccoon")
    });
}

#[test]
fn the_volume_set_in_a_card_is_the_one_in_the_sidebar() {
    let (_server, mut panda, _raccoon) = raccoon_said_hello();
    open_card(&mut panda, "Raccoon");
    slide(&mut panda, "Volume of Raccoon", -8.0);
    wait_for_value(&mut panda, "Volume of Raccoon", "-8");
    panda.escape();
    wait_until(&mut panda, "the card closed", |ui| {
        !ui.has("Volume of Raccoon")
    });
    press(&mut panda, "Raccoon");
    wait_for_value(&mut panda, "Volume of Raccoon", "-8");
}

#[test]
fn opening_a_card_folds_the_sidebar_line_of_its_person() {
    let (_server, mut panda, _raccoon) = raccoon_said_hello();
    press(&mut panda, "Raccoon");
    wait_until(&mut panda, "Raccoon's line open", |ui| {
        ui.has("Volume of Raccoon")
    });
    open_card(&mut panda, "Raccoon");
    wait_until(&mut panda, "only the card open", |ui| {
        ui.values_of("Volume of Raccoon").len() == 1
            && ui
                .middle_of("Volume of Raccoon")
                .is_some_and(|(x, _)| x > SIDEBAR)
    });
}

#[test]
fn the_host_kicks_from_a_card_after_asking() {
    let (_server, mut panda, mut raccoon) = raccoon_said_hello();
    open_card(&mut panda, "Raccoon");
    press(&mut panda, "Kick Raccoon from the room");
    wait_for_text(&mut panda, KICK_RACCOON);
    press(&mut panda, "Kick Raccoon");
    wait_for(&mut raccoon, "Panda kicked you from \"General\".");
    wait_for_text(&mut panda, GONE);
}

#[test]
fn the_card_of_someone_who_left_says_so_and_offers_nothing() {
    let (_server, mut panda, mut raccoon) = raccoon_said_hello();
    press(&mut raccoon, "Leave the room");
    wait_until(&mut panda, "Raccoon gone", |ui| !ui.has("Raccoon"));
    open_card(&mut panda, "Raccoon");
    wait_for_text(&mut panda, GONE);
    assert!(!panda.has("Volume of Raccoon"));
    picture(&panda, "11g-card-of-someone-who-left");
}

#[test]
fn ones_own_card_leads_to_the_profile() {
    let (_server, mut panda, _raccoon) = raccoon_said_hello();
    say(&mut panda, "IKEA");
    wait_for_text(&mut panda, "IKEA");
    open_card(&mut panda, "Panda");
    press(&mut panda, "Edit profile");
    wait_for_switch(&mut panda, "Profile", true);
}

#[test]
fn the_card_of_someone_sharing_starts_watching_their_screen() {
    let mut two = room_with_a_share();
    say(&mut two.panda, "Look");
    wait_for_text(&mut two.raccoon, "Look");
    open_card(&mut two.raccoon, "Panda");
    press(&mut two.raccoon, "Watch the screen of Panda");
    wait_for(&mut two.raccoon, "[button] Stop watching Panda");
}
