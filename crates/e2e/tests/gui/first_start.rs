use whisperia::config::Config;
use whisperia_e2e::{gui::*, TempDir};

const PANDA: &str = "\u{1f43c}";

#[test]
fn the_first_start_asks_for_a_name_and_a_face() {
    let mut panda = window(Config::default());
    wait_for(&mut panda, "Set up your profile");
    assert!(panda.has("Your name"), "{}", panda.screen());
    assert_eq!(panda.is_on("No animal"), Some(true));
    assert_eq!(panda.is_on("Panda"), Some(false));
    picture(&panda, "01-first-start");

    type_into(&mut panda, "Your name", "");
    panda.enter();
    panda.tick();
    assert!(
        panda.screen().contains("Set up your profile"),
        "an empty name is not saved and the question stays"
    );

    type_into(&mut panda, "Your name", "  Panda  ");
    wait_for_text(&mut panda, "Panda");
    assert!(panda.has_text("This is what the others see."));
    assert!(panda.has_text("P"), "the letter of the new name");
    press(&mut panda, "Panda");
    wait_for_text(&mut panda, PANDA);
    assert!(!panda.has_text("P"), "the panda took the letter's place");

    press(&mut panda, "Save");
    wait_until_gone(&mut panda, "Set up your profile");
    wait_for_text(&mut panda, "Panda");
    assert!(panda.has_text(PANDA), "the panda came along");
    assert!(panda.has_text("Servers"), "{}", panda.screen());
    assert!(panda.has_text("Whisperia"), "{}", panda.screen());
    let saved = panda.saved_settings();
    assert_eq!(saved.name, "Panda");
    assert_eq!(saved.avatar.key(), "panda");
    picture(&panda, "02-servers");
}

#[test]
fn the_first_start_does_not_ask_again_after_a_restart() {
    let dir = TempDir::new("again");
    let saved = {
        let mut panda = opening(Config::default())
            .in_dir(TempDir::new("first"))
            .open();
        wait_for(&mut panda, "Set up your profile");
        type_into(&mut panda, "Your name", "Panda");
        press(&mut panda, "Save");
        wait_until_gone(&mut panda, "Set up your profile");
        panda.saved_settings()
    };
    let mut again = opening(saved).in_dir(dir).open();
    wait_for_text(&mut again, "Servers");
    assert!(
        !again.screen().contains("Set up your profile"),
        "asked a second time:\n{}",
        again.screen()
    );
    assert!(again.has_text("Panda"), "{}", again.screen());
}

#[test]
fn the_first_start_sheet_fits_a_small_window() {
    let mut panda = window(Config::default());
    wait_for(&mut panda, "Set up your profile");
    panda.resize(720.0, 520.0);
    panda.tick();
    for label in ["Save", "Bee", "Your name"] {
        let (x, y) = middle_of(&panda, label);
        assert!(
            (0.0..720.0).contains(&x) && (0.0..520.0).contains(&y),
            "{label} is outside the window at ({x}, {y})"
        );
    }
    picture(&panda, "01b-first-start-small");
}

#[test]
fn the_face_at_the_foot_opens_the_profile() {
    let mut panda = window(named("Panda"));
    wait_for_text(&mut panda, "Servers");
    press(&mut panda, "Change your name and face");
    wait_for_switch(&mut panda, "Profile", true);
    assert_eq!(panda.value_of("Your name").as_deref(), Some("Panda"));
    assert!(!panda.has("Save"), "the profile keeps itself");
    type_into(&mut panda, "Your name", "  Codependency ");
    press(&mut panda, "Your name");
    wait_for_value(&mut panda, "Your name", "Codependency");
    picture(&panda, "28-profile");
    press(&mut panda, "Close");
    wait_until_gone(&mut panda, "[button] Profile");
    assert!(panda.has_text("Codependency"), "{}", panda.screen());
}
