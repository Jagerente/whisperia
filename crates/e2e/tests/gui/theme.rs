use whisperia::gui::Token::{self, *};
use whisperia_e2e::gui::*;

const TEXT: f64 = 4.5;
const MARK: f64 = 3.0;

type Rgb = (u8, u8, u8);

fn luminance((red, green, blue): Rgb) -> f64 {
    let linear = |channel: u8| {
        let c = f64::from(channel) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

fn contrast(one: Rgb, other: Rgb) -> f64 {
    let (a, b) = (luminance(one), luminance(other));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

const READ: [(Token, &[Token], f64); 12] = [
    (
        Text,
        &[Window, Surface, Raised, RaisedHover, Menu, MenuHover],
        TEXT,
    ),
    (TextDim, &[Window, Surface, Raised, RaisedHover], TEXT),
    (TextFaint, &[Window, Surface, Raised], MARK),
    (Bad, &[Window, Surface, Raised], TEXT),
    (Warn, &[Surface], TEXT),
    (Warn, &[Window, Raised], MARK),
    (Good, &[Window, Surface, Raised], MARK),
    (Gold, &[Window, Surface, Raised], MARK),
    (OnAccent, &[BadFill, Toast], TEXT),
    (OnAccent, &[GoodFill], MARK),
    (BadFill, &[Surface], MARK),
    (GoodFill, &[Surface], MARK),
];

#[test]
fn text_and_marks_read_well_in_both_themes() {
    let mut ui = window(named("Panda"));
    for dark in [false, true] {
        ui.use_dark_colours(dark);
        ui.tick();
        let theme = if dark { "dark" } else { "light" };
        for (fore, backs, least) in READ {
            for back in backs {
                let ratio = contrast(ui.colour(fore), ui.colour(*back));
                assert!(
                    ratio >= least,
                    "{fore:?} on {back:?} in {theme} is {ratio:.2}, below {least}"
                );
            }
        }
        for (index, avatar) in ui.avatar_colours().into_iter().enumerate() {
            let ratio = contrast(ui.colour(OnAccent), avatar);
            assert!(
                ratio >= TEXT,
                "initials on avatar colour {index} in {theme} are {ratio:.2}"
            );
        }
    }
}
