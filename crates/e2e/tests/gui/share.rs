use std::sync::Arc;

use whisperia::{
    desktop::DesktopEvent,
    gui::Harness,
    screen::{
        fake::{FakeScreen, ICON_COLOUR},
        SourceKind, SourceNews,
    },
};
use whisperia_e2e::{gui::*, TestServer};

fn paint(screen: &FakeScreen, label: &str, colour: [u8; 3]) {
    let mut painted = screen.painted.lock().unwrap();
    painted.retain(|(painted, _)| painted != label);
    painted.push((label.into(), colour));
}

fn list_window(screen: &FakeScreen, label: &str) {
    screen
        .listed
        .lock()
        .unwrap()
        .push((SourceKind::Window, label.into(), "program".into()));
}

fn sharer(server: &TestServer) -> (Window, Arc<FakeScreen>) {
    let screen = Arc::new(FakeScreen::new());
    let mut panda = opening(named("Panda")).screen(screen.clone()).open();
    enter_general(&mut panda, &server.address(), "Panda");
    (panda, screen)
}

#[test]
fn the_picker_shows_screens_and_windows_with_previews() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, screen) = sharer(&server);
    open_the_picker(&mut panda);
    assert!(panda.screen().contains("Share your screen"));
    assert_eq!(panda.is_on("What to share: Screens"), Some(true));
    assert_eq!(panda.is_on("What to share: Windows"), Some(false));
    assert!(panda.screen().contains("Choose a screen."));
    assert!(!panda.has("whisperia - Visual Studio Code"));

    paint(&screen, "Screen 1", [40, 60, 220]);
    wait_for_colour(&mut panda, "Screen 1", |(r, g, b)| {
        b > r + 100 && b > g + 100
    });
    paint(&screen, "Screen 1", [40, 200, 60]);
    wait_for_colour(&mut panda, "Screen 1", |(r, g, b)| {
        g > r + 100 && g > b + 100
    });
    picture(&panda, "14-what-to-share-screens");

    press(&mut panda, "What to share: Windows");
    wait_until(&mut panda, "the windows to choose from", |ui| {
        ui.is_on("whisperia - Visual Studio Code") == Some(false)
    });
    assert!(panda.screen().contains("Choose a window."));
    assert!(!panda.has("Screen 2"));
    paint(&screen, "Containers - Docker Desktop", [120, 120, 120]);
    let [r, g, b] = ICON_COLOUR.map(i32::from);
    wait_for_colour(&mut panda, "Containers - Docker Desktop", |(r2, g2, b2)| {
        (r - r2).abs() < 30 && (g - g2).abs() < 30 && (b - b2).abs() < 30
    });
}

#[test]
fn cancelling_the_picker_shares_nothing() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, screen) = sharer(&server);
    open_the_picker(&mut panda);
    press(&mut panda, "Share");
    press(&mut panda, "Cancel");
    wait_until_gone(&mut panda, "Share your screen");
    assert_eq!(screen.shares_started(), 0);
    assert!(panda.has("Share my screen"));
}

#[test]
fn a_new_window_comes_first_and_a_closed_one_is_unchosen() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, screen) = sharer(&server);
    open_the_picker(&mut panda);
    press(&mut panda, "What to share: Windows");
    wait_until(&mut panda, "the windows to choose from", |ui| {
        ui.has("GitHub - File Explorer")
    });

    list_window(&screen, "Untitled - Paint");
    wait_until(&mut panda, "Paint to choose from", |ui| {
        ui.is_on("Untitled - Paint") == Some(false)
    });
    wait_until(&mut panda, "the new window first", |ui| {
        let (paint_x, paint_y) = middle_of(ui, "Untitled - Paint");
        let (explorer_x, explorer_y) = middle_of(ui, "GitHub - File Explorer");
        (paint_y, paint_x) < (explorer_y, explorer_x)
    });
    press(&mut panda, "Untitled - Paint");
    wait_until(&mut panda, "Paint chosen", |ui| {
        ui.is_on("Untitled - Paint") == Some(true)
    });
    screen
        .listed
        .lock()
        .unwrap()
        .retain(|(_, label, _)| label != "Untitled - Paint");
    wait_until(&mut panda, "Paint gone", |ui| !ui.has("Untitled - Paint"));
    wait_for(&mut panda, "Choose a window.");
}

#[test]
fn the_sound_caption_follows_what_is_chosen() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, _screen) = sharer(&server);
    open_the_picker(&mut panda);
    press(&mut panda, "What to share: Windows");
    press(&mut panda, "GitHub - File Explorer");
    wait_for(&mut panda, "With the sound of this program.");
    assert_eq!(panda.is_on("GitHub - File Explorer"), Some(true));

    press(&mut panda, "What to share: Screens");
    wait_for(&mut panda, "Choose a screen.");
    press(&mut panda, "Screen 2");
    wait_for(&mut panda, "With everything this PC plays");
    press(&mut panda, "What to share: Windows");
    wait_until(&mut panda, "the window still chosen", |ui| {
        ui.is_on("GitHub - File Explorer") == Some(true)
    });
}

#[test]
fn resolution_and_frame_rate_are_asked_in_the_picker_and_used() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, screen) = sharer(&server);
    open_the_picker(&mut panda);
    press(&mut panda, "What to share: Windows");
    press(&mut panda, "GitHub - File Explorer");
    press(&mut panda, "Resolution: 720p");
    press(&mut panda, "Frames per second: 15");
    wait_for_switch(&mut panda, "Frames per second: 15", true);
    assert_eq!(panda.is_on("Resolution: 720p"), Some(true));
    picture(&panda, "14-what-to-share");
    press(&mut panda, "Share");
    wait_for(&mut panda, "You are sharing GitHub - File Explorer");
    assert_eq!(screen.labels_shown(), ["GitHub - File Explorer"]);
    let settings = screen.settings_seen();
    assert_eq!((settings[0].max_height, settings[0].fps), (720, 15));
    wait_for_text(&mut panda, "You are sharing");
    assert!(
        panda
            .screen()
            .contains("GitHub - File Explorer, 1280 x 720, 15 fps"),
        "{}",
        panda.screen()
    );
    picture(&panda, "32-my-share");
}

#[track_caller]
fn open_the_change(ui: &mut Harness, shared: &str) {
    press(ui, "Change what is shared");
    wait_for(ui, "Change what you share");
    wait_until(ui, &format!("{shared} chosen"), |ui| {
        ui.is_on(shared) == Some(true)
    });
}

#[test]
fn the_card_says_what_is_really_sent() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, _screen) = sharer(&server);
    share_window(&mut panda, "GitHub - File Explorer");
    wait_for(&mut panda, "GitHub - File Explorer, 1920 x 1080, 60 fps");
}

#[test]
fn the_quality_changes_while_sharing_without_starting_again() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, screen) = sharer(&server);
    share_window(&mut panda, "GitHub - File Explorer");
    open_the_change(&mut panda, "GitHub - File Explorer");
    press(&mut panda, "Resolution: 720p");
    press(&mut panda, "Frames per second: 30");
    wait_for_switch(&mut panda, "Frames per second: 30", true);
    picture(&panda, "52-change-what-is-shared");
    press(&mut panda, "Apply");
    wait_until_gone(&mut panda, "Change what you share");
    wait_for(&mut panda, "GitHub - File Explorer, 1280 x 720, 30 fps");
    assert_eq!(screen.shares_started(), 1);
    let settings = screen.settings_seen();
    assert_eq!(settings.len(), 2, "{settings:?}");
    assert_eq!((settings[1].max_height, settings[1].fps), (720, 30));
}

#[test]
fn a_minimized_window_is_offered_and_opened_when_shared() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, screen) = sharer(&server);
    screen
        .minimized
        .lock()
        .unwrap()
        .push("GitHub - File Explorer".into());
    open_the_picker(&mut panda);
    press(&mut panda, "What to share: Windows");
    wait_for_text(&mut panda, "Minimized. Sharing opens it.");
    assert_eq!(
        panda.texts_within("GitHub - File Explorer"),
        ["Minimized. Sharing opens it.", "explorer"]
    );
    picture(&panda, "14c-minimized-window");
    press(&mut panda, "Cancel");
    wait_until_gone(&mut panda, "Share your screen");

    share_window(&mut panda, "GitHub - File Explorer");
    assert!(!screen.is_minimized("GitHub - File Explorer"));
}

#[test]
fn the_stream_follows_the_size_of_the_shared_window() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, screen) = sharer(&server);
    share_window(&mut panda, "GitHub - File Explorer");
    wait_for(&mut panda, "GitHub - File Explorer, 1920 x 1080, 60 fps");
    screen.tell_what_is_shared(SourceNews::Resized((1200, 900)));
    wait_for(&mut panda, "GitHub - File Explorer, 1200 x 900, 60 fps");
    screen.tell_what_is_shared(SourceNews::Resized((3000, 2000)));
    wait_for(&mut panda, "GitHub - File Explorer, 2160 x 1440, 60 fps");
    assert_eq!(screen.shares_started(), 1);
}

#[test]
fn a_minimized_share_is_told_to_the_sharer_and_the_watchers() {
    let mut two = room_with_a_share();
    watch(&mut two.raccoon, "Panda");
    let said = "Panda minimized the window. The picture comes back when it is open.";

    two.panda_screen.tell_what_is_shared(SourceNews::Minimized);
    wait_for_text(
        &mut two.panda,
        "The window is minimized: the others see its last picture until you open it.",
    );
    wait_for_text(&mut two.raccoon, said);
    assert!(two.raccoon.has("Stop watching Panda"));
    picture(&two.raccoon, "54-a-minimized-share");

    two.panda_screen.tell_what_is_shared(SourceNews::Restored);
    wait_until_text_gone(&mut two.raccoon, said);
    assert!(!two
        .panda
        .has_text("The window is minimized: the others see its last picture until you open it."));
}

#[test]
fn the_share_moves_to_another_window_and_the_watcher_keeps_watching() {
    let mut two = room_with_a_share();
    watch(&mut two.raccoon, "Panda");
    let viewer = two.raccoon_screen.viewers().pop().expect("a viewer");
    open_the_change(&mut two.panda, "Screen 1");
    press(&mut two.panda, "What to share: Windows");
    press(&mut two.panda, "whisperia - Visual Studio Code");
    wait_until(&mut two.panda, "the window chosen", |ui| {
        ui.is_on("whisperia - Visual Studio Code") == Some(true)
    });
    let before = viewer.shown().len();
    press(&mut two.panda, "Apply");
    wait_for(
        &mut two.panda,
        "whisperia - Visual Studio Code, 1920 x 1080, 60 fps",
    );
    assert_eq!(
        two.panda_screen.labels_shown(),
        ["Screen 1", "whisperia - Visual Studio Code"]
    );

    wait_until(&mut two.raccoon, "the first picture of the window", |_| {
        viewer.shown()[before..]
            .iter()
            .any(|&(keyframe, number, _)| keyframe && number == 0)
    });
    assert!(two.raccoon.has("Stop watching Panda"));
    assert!(
        !two.raccoon.has_text("Panda stopped sharing their screen"),
        "{}",
        two.raccoon.screen()
    );
}

#[test]
fn cancelling_a_change_keeps_the_share_as_it_was() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, screen) = sharer(&server);
    share_window(&mut panda, "GitHub - File Explorer");
    open_the_change(&mut panda, "GitHub - File Explorer");
    press(&mut panda, "What to share: Screens");
    press(&mut panda, "Screen 2");
    press(&mut panda, "Resolution: 720p");
    press(&mut panda, "Cancel");
    wait_until_gone(&mut panda, "Change what you share");
    wait_for(&mut panda, "GitHub - File Explorer, 1920 x 1080, 60 fps");
    assert_eq!(screen.labels_shown(), ["GitHub - File Explorer"]);
    assert_eq!(screen.settings_seen().len(), 1);
}

#[test]
fn sharing_shows_live_in_green_and_tells_the_tray() {
    let mut two = two_with_screens();
    assert!(
        !two.raccoon.screen().contains("Watch"),
        "nothing to watch before a share"
    );
    share(&mut two.panda, "Screen 1");
    let panda = &mut two.panda;
    assert!(panda.has("Stop sharing my screen"));
    assert!(panda.has_text("LIVE"));
    assert!(!panda.screen().contains("Share your screen"));
    assert!(!panda.screen().contains("watching"), "nobody watches yet");
    let stop = panda.count_text("Stop sharing");
    hover(panda, "Stop sharing my screen");
    wait_until(panda, "the tip naming it as the stage does", |ui| {
        ui.count_text("Stop sharing") == stop + 1
    });
    picture(panda, "20-live");
    panda.mouse_leaves();
    wait_for_colour(panda, "Stop sharing my screen", |(r, g, b)| {
        g > r + 80 && g > b + 50
    });
    wait_until(panda, "the tray hearing of the share", |_| {
        statuses(&two.commands).iter().any(|status| status.sharing)
    });

    let raccoon = &mut two.raccoon;
    wait_for(raccoon, "[button] Watch what Panda shows");
    assert!(raccoon.has("Watch Panda's screen"));
    let (offer, _) = middle_of(raccoon, "Watch what Panda shows");
    let (chat, _) = middle_of(raccoon, "Message");
    assert!(
        offer > LEFT_SIDE && offer < chat,
        "the offer belongs on the stage, not over the chat: {offer} and {chat}"
    );
    assert_eq!(
        settled_colour(&mut two.panda, "LIVE"),
        settled_colour(&mut two.raccoon, "LIVE"),
        "one's own mark and somebody else's are the same"
    );
    hover(&mut two.raccoon, "Watch Panda's screen");
    wait_for_text(&mut two.raccoon, "Watch");
    two.raccoon.mouse_leaves();
    picture(&two.raccoon, "09-somebody-shows");
}

#[test]
fn the_share_hotkey_opens_and_closes_the_picker() {
    let mut two = two_with_screens();
    two.desktop.send(DesktopEvent::ToggleShare).unwrap();
    wait_for(&mut two.panda, "Share your screen");
    two.desktop.send(DesktopEvent::ToggleShare).unwrap();
    wait_until_gone(&mut two.panda, "Share your screen");
    assert_eq!(two.panda_screen.shares_started(), 0);
}

#[test]
fn the_last_kind_shared_is_offered_first_and_a_long_list_scrolls_to_its_end() {
    let server = TestServer::start("DESKTOP-6TNC");
    let (mut panda, screen) = sharer(&server);
    share_window(&mut panda, "GitHub - File Explorer");
    press(&mut panda, "Stop sharing my screen");
    wait_until(&mut panda, "the share stopped", |ui| {
        ui.has("Share my screen")
    });
    for extra in 0..9 {
        list_window(&screen, &format!("Window {extra}"));
    }
    press(&mut panda, "Share my screen");
    wait_until(&mut panda, "the windows to choose from", |ui| {
        ui.is_on("GitHub - File Explorer") == Some(false)
    });
    assert_eq!(
        panda.is_on("What to share: Windows"),
        Some(true),
        "a window was shared last"
    );
    wait_until_gone(&mut panda, "Finding screens and windows");
    assert!(!panda.has("Window 8"), "not in sight yet");
    assert!(panda.scroll("GitHub - File Explorer", 900.0));
    wait_until(&mut panda, "the list scrolled to its end", |ui| {
        ui.is_on("Window 8") == Some(false)
    });
    picture(&panda, "16-many-windows");
    press(&mut panda, "Window 8");
    press(&mut panda, "Share");
    wait_for(&mut panda, "You are sharing Window 8");
    assert_eq!(screen.shares_started(), 2);
}

#[test]
fn leaving_the_room_ends_the_share_for_the_others() {
    let mut two = room_with_a_share();
    press(&mut two.panda, "Leave the room");
    wait_for(&mut two.panda, "Left the room.");
    wait_until(&mut two.raccoon, "the offer gone", |ui| {
        !ui.has("Watch Panda's screen")
    });
}

#[test]
fn a_share_whose_window_is_closed_says_so_and_ends_for_the_others() {
    let mut two = room_with_a_share();
    two.panda_screen.close_what_is_shared();
    wait_for_text(&mut two.panda, "Stopped sharing: what was shown is gone.");
    assert!(two.panda.has("Share my screen"));
    wait_until(&mut two.raccoon, "the offer gone", |ui| {
        !ui.has("Watch what Panda shows")
    });
}
