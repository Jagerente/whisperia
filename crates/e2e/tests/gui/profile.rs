use whisperia::config::Config;
use whisperia_e2e::{gui::*, TestServer};
use whisperia_proto::control::Avatar;

const FOX: &str = "\u{1f98a}";
const PANDA: &str = "\u{1f43c}";
const RACCOON: &str = "\u{1f99d}";
const EMPTY: &str = "Type a name: it cannot be empty.";

#[test]
fn an_animal_picked_in_the_profile_is_shown_and_kept() {
    let mut panda = window(Config {
        avatar: Avatar::None,
        ..named("Panda")
    });
    press(&mut panda, "Change your name and face");
    wait_for_switch(&mut panda, "Profile", true);
    assert_eq!(panda.is_on("No animal"), Some(true));
    assert_eq!(panda.is_on("Panda"), Some(false));
    assert!(panda.has_text("The first letter of your name"));
    picture(&panda, "28b-profile-no-animal");

    press(&mut panda, "Panda");
    wait_for_switch(&mut panda, "Panda", true);
    assert_eq!(panda.is_on("No animal"), Some(false));
    assert!(panda.has_text(PANDA), "the big face is still a letter");
    assert!(!panda.has_text("The first letter of your name"));
    picture(&panda, "28c-profile-panda");
    press(&mut panda, "Close");

    wait_for_text(&mut panda, PANDA);
    assert!(panda.has_text("Panda"), "{}", panda.screen());
    assert_eq!(panda.saved_settings().avatar.key(), "panda");
}

#[test]
fn the_others_see_a_new_face_everywhere_and_it_is_not_news() {
    let server = TestServer::start("DESKTOP-3MRX");
    let mut panda = window(named("Panda"));
    press(&mut panda, "Change your name and face");
    press(&mut panda, "Fox");
    press(&mut panda, "Close");
    enter_general(&mut panda, &server.address(), "Panda");
    let mut raccoon = window(named("Raccoon"));
    enter_general(&mut raccoon, &server.address(), "Raccoon");
    wait_until(&mut panda, "Raccoon in the room", |ui| ui.has("Raccoon"));

    wait_for_text(&mut raccoon, FOX);
    say(&mut panda, "Hello World!");
    wait_for_text(&mut raccoon, "Hello World!");
    assert_eq!(raccoon.count_text(FOX), 2, "line and message");

    press(&mut panda, "Settings");
    press(&mut panda, "Profile");
    press(&mut panda, "Panda");
    press(&mut panda, "Close");
    wait_for_text(&mut raccoon, PANDA);
    wait_until_text_gone(&mut raccoon, FOX);
    assert_eq!(raccoon.count_text(PANDA), 2, "{}", raccoon.screen());
    assert!(
        !raccoon.screen().contains("Panda is now"),
        "a face is not news\n{}",
        raccoon.screen()
    );
    assert!(raccoon.has_text(RACCOON), "Raccoon keeps their raccoon");
    picture(&raccoon, "11b-room-with-animals");

    press(&mut raccoon, "Leave the room");
    wait_for(&mut raccoon, "[button] Enter General");
    assert!(raccoon.has_text(PANDA) && raccoon.has_text("Panda"));

    press(&mut panda, "Settings");
    press(&mut panda, "Profile");
    press(&mut panda, "No animal");
    press(&mut panda, "Close");
    wait_for_text(&mut panda, "P");
    assert!(!panda.has_text(PANDA));
    assert_eq!(panda.saved_settings().avatar.key(), "none");
    wait_until_text_gone(&mut raccoon, PANDA);
}

#[test]
fn the_profile_keeps_a_name_by_itself_and_refuses_an_empty_one() {
    let mut panda = window(Config {
        avatar: Avatar::None,
        ..named("Panda")
    });
    wait_for_text(&mut panda, "Servers");
    press(&mut panda, "Change your name and face");
    wait_for_switch(&mut panda, "Profile", true);
    assert!(!panda.has("Save"));
    assert!(!panda.has("Enter"));
    assert!(panda.has_text("This is what the others see."));
    wait_for_colour(&mut panda, "No animal", is_colourful);

    assert!(panda.tap("Your name"));
    panda.tick();
    assert!(!panda.has("Enter"), "nothing to save yet");

    type_into(&mut panda, "Your name", "Codependency");
    wait_until(&mut panda, "a way to save the name", |ui| ui.has("Enter"));
    assert!(panda.has_text("This is what the others see."));
    picture(&panda, "28d-profile-name-held");

    panda.escape();
    wait_for_value(&mut panda, "Your name", "Panda");
    assert!(!panda.has("Enter"), "the field is let go");

    panda.tap("Your name");
    type_into(&mut panda, "Your name", "");
    press(&mut panda, "Your name");
    wait_for_text(&mut panda, EMPTY);
    assert!(
        panda.count_text("P") >= 2,
        "the preview keeps the saved name's letter instead of a blank disc"
    );
    assert!(
        !panda.has_text("This is what the others see."),
        "the remark makes room for the complaint"
    );
    assert!(!panda.has("Enter"), "nothing to save");
    picture(&panda, "28e-profile-name-empty");

    panda.tab();
    wait_for_value(&mut panda, "Your name", "Panda");
    assert!(!panda.has_text(EMPTY));

    panda.tap("Your name");
    type_into(&mut panda, "Your name", "  Penguin ");
    wait_until(&mut panda, "a way to save the name", |ui| ui.has("Enter"));
    press(&mut panda, "Enter");
    wait_for_value(&mut panda, "Your name", "Penguin");
    assert!(!panda.has("Enter"), "saving lets the field go");
    assert_eq!(panda.saved_settings().name, "Penguin");

    panda.tap("Your name");
    type_into(&mut panda, "Your name", "Codependency");
    panda.tab();
    wait_for_value(&mut panda, "Your name", "Codependency");
    assert_eq!(panda.saved_settings().name, "Codependency");

    panda.resize(720.0, 520.0);
    panda.tick();
    assert!(panda.has("Bee"));
    picture(&panda, "28f-profile-small");
    panda.resize(1180.0, 760.0);
    panda.tick();

    press(&mut panda, "Close");
    wait_for_text(&mut panda, "Codependency");
}
