use std::sync::Arc;

use whisperia::preview::{fake::FakePreviews, LinkPreview, Picture};
use whisperia_e2e::{gui::*, TestServer};

const LINK: &str = "https://github.com/Jagerente/whisperia";
const MORE_LINES_THAN_FIT: usize = 40;

fn two_in_a_room() -> (TestServer, Window, Window) {
    let server = TestServer::start("DESKTOP-6TNC");
    let mut room = room_of(&server, &["Panda", "Raccoon"]);
    let raccoon = room.pop().expect("two were asked for");
    let panda = room.pop().expect("two were asked for");
    (server, panda, raccoon)
}

fn two_in_a_room_with_a_preview() -> (TestServer, Window, Window) {
    let server = TestServer::start("DESKTOP-6TNC");
    let previews = FakePreviews::default().with(
        LINK,
        LinkPreview {
            site: "GitHub".into(),
            title: Some(
                "GitHub - Jagerente/whisperia: Self hosted voice chat with screen sharing, on a server of your own"
                    .into(),
            ),
            description: Some("Voice chat with screen sharing, on a server of your own.".into()),
            picture: Some(Picture::plain(400, 210, [30, 36, 51, 255])),
            colour: Some(0x1e2433),
        },
    );
    let mut panda = opening(named("Panda")).previews(Arc::new(previews)).open();
    enter_general(&mut panda, &server.address(), "Panda");
    let mut raccoon = window(named("Raccoon"));
    enter_general(&mut raccoon, &server.address(), "Raccoon");
    wait_until(&mut panda, "Raccoon in the room", |ui| ui.has("Raccoon"));
    (server, panda, raccoon)
}

fn is_a_time(text: &str) -> bool {
    let digits = text.chars().filter(char::is_ascii_digit).count();
    text.len() == 5 && text.as_bytes()[2] == b':' && digits == 4
}

#[test]
fn consecutive_messages_are_grouped_under_one_name() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    let before = panda.count_text("Raccoon");

    say(&mut raccoon, "Hello World!");
    wait_for_text(&mut panda, "Hello World!");
    say(&mut raccoon, "ave 83 \\o/");
    wait_for_text(&mut panda, "ave 83 \\o/");
    say(&mut panda, "IKEA");
    say(&mut panda, "Nice, www.rust-lang.org too");
    wait_for_text(&mut panda, "Nice, www.rust-lang.org too");
    assert_eq!(
        panda.count_text("Raccoon"),
        before + 1,
        "two messages in a row stand under one name\n{}",
        panda.screen()
    );
    picture(&panda, "11-chat");
}

#[test]
fn ones_own_messages_stand_under_ones_name_in_line_with_the_others() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    let names = panda.count_text("Panda");
    say(&mut raccoon, "Hello World!");
    wait_for_text(&mut panda, "Hello World!");
    say(&mut panda, "IKEA");
    wait_for_text(&mut panda, "IKEA");
    let (theirs, _) = middle_of(&panda, "Hello World!");
    let (mine, _) = middle_of(&panda, "IKEA");
    assert_eq!(mine, theirs);
    assert_eq!(panda.count_text("Panda"), names + 1);
}

#[test]
fn a_message_and_a_note_say_when_they_were_written() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    say(&mut raccoon, "Hello World!");
    wait_for_text(&mut panda, "Hello World!");
    let times: Vec<String> = panda
        .texts()
        .into_iter()
        .filter(|text| is_a_time(text))
        .collect();
    assert_eq!(
        times.len(),
        2,
        "Raccoon joined, then said\n{}",
        panda.screen()
    );
}

#[test]
fn a_later_message_under_the_same_name_tells_its_time_when_pointed_at() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    say(&mut raccoon, "Hello World!");
    say(&mut raccoon, "IKEA");
    wait_for_text(&mut panda, "IKEA");
    let times = |ui: &Window| ui.texts().iter().filter(|text| is_a_time(text)).count();
    let before = times(&panda);
    hover(&mut panda, "IKEA");
    assert_eq!(times(&panda), before + 1, "{}", panda.screen());
}

#[test]
fn pointing_at_a_message_leaves_its_line_as_it_was() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    panda.use_dark_colours(false);
    say(&mut raccoon, "Hello World!");
    wait_for_text(&mut panda, "Hello World!");
    panda.tick();
    let unpointed = panda
        .brightest_in("Hello World!")
        .expect("the message is shown");
    hover(&mut panda, "Hello World!");
    let pointed = panda
        .brightest_in("Hello World!")
        .expect("the message is shown");
    assert_eq!(pointed, unpointed);
}

#[test]
fn the_scroll_bar_of_the_chat_keeps_clear_of_the_messages() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    for n in 1..=MORE_LINES_THAN_FIT {
        say(&mut raccoon, &format!("line {n}"));
    }
    let last = format!("line {MORE_LINES_THAN_FIT}");
    wait_for_text(&mut panda, &last);
    assert_eq!(panda.value_of("Scroll the chat").as_deref(), Some("100"));
    let (bar, ..) = panda.bounds("Scroll the chat").expect("the chat scrolls");
    let (x, _, width, _) = panda.bounds(&last).expect("the last line is shown");
    assert!(
        x + width <= bar,
        "the line ends at {}, the bar starts at {bar}",
        x + width
    );
    picture(&panda, "11e-chat-scrolled");
}

#[test]
fn a_long_word_wraps_inside_the_chat() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    let word = "Whisperia".repeat(40);
    let link = format!("{LINK}/{}", "tree/main/".repeat(30));
    say(&mut raccoon, &word);
    say(&mut raccoon, &link);
    wait_for_text(&mut panda, &link);
    let (bar, ..) = panda.bounds("Scroll the chat").expect("the chat scrolls");
    for said in [&word, &link] {
        let (x, _, width, height) = panda.bounds(said).expect("the message is shown");
        assert!(
            x + width <= bar,
            "the message ends at {}, the bar starts at {bar}",
            x + width
        );
        assert!(height > 40.0, "the message did not wrap: {height} high");
    }
    picture(&panda, "11g-long-words");
}

#[test]
fn a_link_in_a_message_opens_in_the_browser() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    say(&mut raccoon, LINK);
    wait_for_text(&mut panda, LINK);
    assert!(panda.tap_start(LINK));
    wait_until(&mut panda, "the link opened", |ui| ui.opened() == [LINK]);
}

#[test]
fn a_link_gets_a_preview_with_its_title_description_and_picture() {
    let (_server, mut panda, mut raccoon) = two_in_a_room_with_a_preview();
    say(&mut raccoon, &format!("Look at {LINK} now"));
    wait_for(&mut panda, &format!("Open {LINK}"));
    assert!(panda.has_text("GitHub"));
    assert!(panda.has_text("Voice chat with screen sharing, on a server of your own."));
    assert!(panda.hover(&format!("Open {LINK}")));
    picture(&panda, "11b-link-preview");
}

#[test]
fn the_title_of_a_preview_opens_its_link() {
    let (_server, mut panda, mut raccoon) = two_in_a_room_with_a_preview();
    say(&mut raccoon, LINK);
    wait_for(&mut panda, &format!("Open {LINK}"));
    press(&mut panda, &format!("Open {LINK}"));
    wait_until(&mut panda, "the link opened", |ui| ui.opened() == [LINK]);
}

#[test]
fn a_preview_is_hidden_with_its_close_button() {
    let (_server, mut panda, mut raccoon) = two_in_a_room_with_a_preview();
    say(&mut raccoon, LINK);
    wait_for(&mut panda, &format!("Open {LINK}"));
    press(&mut panda, &format!("Hide the preview of {LINK}"));
    wait_until_gone(&mut panda, &format!("Open {LINK}"));
    assert!(panda.has_text(LINK));
}

#[test]
fn a_click_on_a_message_takes_the_typing_away_from_the_message_field() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    say(&mut raccoon, "Hello World!");
    wait_for_text(&mut panda, "Hello World!");
    assert!(panda.tap("Message"));
    panda.type_text("IKEA");
    wait_for_value(&mut panda, "Message", "IKEA");
    assert!(panda.tap_start("Hello World!"));
    panda.tick();
    panda.type_text("!");
    panda.tick();
    assert_eq!(panda.value_of("Message").as_deref(), Some("IKEA"));
}

#[test]
fn sending_with_the_button_keeps_typing_in_the_message_field() {
    let (_server, mut panda, _raccoon) = two_in_a_room();
    assert!(panda.tap("Message"));
    panda.type_text("one");
    wait_for_value(&mut panda, "Message", "one");
    assert!(panda.tap("Send"));
    wait_for_text(&mut panda, "one");
    panda.type_text("two");
    wait_for_value(&mut panda, "Message", "two");
}

#[test]
fn escape_takes_the_typing_away_from_the_message_field_and_keeps_the_draft() {
    let (_server, mut panda, _raccoon) = two_in_a_room();
    assert!(panda.tap("Message"));
    panda.type_text("IKEA");
    wait_for_value(&mut panda, "Message", "IKEA");
    panda.escape();
    panda.type_text("!");
    panda.tick();
    assert_eq!(panda.value_of("Message").as_deref(), Some("IKEA"));
}

#[test]
fn the_copy_button_of_a_message_copies_all_of_it() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    let said = format!("Look at {LINK} now");
    say(&mut raccoon, &said);
    wait_for_text(&mut panda, &said);
    let button = format!("Copy the message: {said}");
    assert!(panda.hover(&button));
    picture(&panda, "11c-message-copy");
    press(&mut panda, &button);
    wait_until(&mut panda, "the message copied", |ui| {
        ui.copied() == [said.as_str()]
    });
}

#[test]
fn messages_that_come_while_reading_older_ones_wait_below() {
    let (_server, mut panda, mut raccoon) = two_in_a_room();
    for n in 1..=MORE_LINES_THAN_FIT {
        say(&mut raccoon, &format!("line {n}"));
    }
    let last = format!("line {MORE_LINES_THAN_FIT}");
    wait_for_text(&mut panda, &last);
    panda.scroll(&last, -2000.0);
    panda.tick();

    say(&mut raccoon, "one more");
    say(&mut raccoon, "and another");
    wait_for(&mut panda, "[button] 2 new messages");
    picture(&panda, "11d-new-messages-below");
    press(&mut panda, "2 new messages");
    wait_until_gone(&mut panda, "new messages");
}

#[test]
fn messages_for_a_hidden_chat_are_counted_on_its_button() {
    let mut two = room_with_a_share();
    watch(&mut two.raccoon, "Panda");
    press(&mut two.raccoon, "Hide the chat");
    say(&mut two.panda, "Hello World!");
    say(&mut two.panda, "IKEA");
    wait_for_text(&mut two.raccoon, "2 unread messages");
    picture(&two.raccoon, "36b-unread-messages");
    press(&mut two.raccoon, "Show the chat");
    wait_for_text(&mut two.raccoon, "IKEA");
    assert!(!two.raccoon.has_text("2 unread messages"));
}

#[test]
fn the_last_characters_a_message_may_have_are_counted() {
    let (_server, mut panda, _raccoon) = two_in_a_room();
    type_into(&mut panda, "Message", &"a".repeat(360));
    wait_for_text(&mut panda, "40 characters left");
    type_into(&mut panda, "Message", "a short one");
    wait_until_text_gone(&mut panda, "40 characters left");
}
