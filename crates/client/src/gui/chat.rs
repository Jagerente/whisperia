use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, StyledText, VecModel};
use whisperia_proto::PeerId;

use super::{face, ui, MY_ID};
use crate::{
    app::{About, Chat, ChatLine, Line, Message, Piece, Said, Url},
    preview::{LinkPreview, Picture},
};

const ONE_GROUP_WITHIN_SECONDS: i64 = 5 * 60;
const WIDE_PICTURE: u32 = 300;

type Shown = Vec<(Url, *const LinkPreview)>;

struct Row {
    styled: StyledText,
    shown: Shown,
    embeds: ModelRc<ui::Embed>,
}

#[derive(Default)]
pub(super) struct Lines(HashMap<u64, Row>);

impl Lines {
    pub(super) fn rows<'a>(
        &mut self,
        chat: &Chat,
        here: impl Fn(PeerId) -> bool,
        preview: impl Fn(&Url) -> Option<&'a Arc<LinkPreview>>,
    ) -> Vec<ui::ChatLine> {
        let lines = chat.lines();
        let kept: HashSet<u64> = lines.iter().map(|line| line.number).collect();
        self.0.retain(|number, _| kept.contains(number));
        lines
            .iter()
            .zip(heads(lines))
            .map(|(line, head)| self.row(chat, line, head, &here, &preview))
            .collect()
    }

    fn row<'a>(
        &mut self,
        chat: &Chat,
        line: &ChatLine,
        head: bool,
        here: &impl Fn(PeerId) -> bool,
        preview: &impl Fn(&Url) -> Option<&'a Arc<LinkPreview>>,
    ) -> ui::ChatLine {
        let time = line.at.format("%H:%M").to_string().into();
        let number = i32::try_from(line.number).unwrap_or(i32::MAX);
        match &line.what {
            Line::Said(said) => {
                let sender = sender_here(said, here);
                let shown: Vec<(&Url, &Arc<LinkPreview>)> = said
                    .message
                    .links()
                    .filter(|url| chat.shows_preview(line.number, url))
                    .filter_map(|url| preview(url).map(|found| (url, found)))
                    .collect();
                let identity: Shown = shown
                    .iter()
                    .map(|(url, found)| ((*url).clone(), Arc::as_ptr(found)))
                    .collect();
                let row = self.0.entry(line.number).or_insert_with(|| Row {
                    styled: styled(&said.message),
                    shown: Vec::new(),
                    embeds: ModelRc::default(),
                });
                if row.shown != identity {
                    let embeds: Vec<ui::Embed> =
                        shown.iter().map(|(url, found)| embed(url, found)).collect();
                    row.embeds = ModelRc::from(Rc::new(VecModel::from(embeds)));
                    row.shown = identity;
                }
                ui::ChatLine {
                    number,
                    about: ui::About::Said,
                    face: face(&said.who, said.avatar),
                    sender: sender.unwrap_or_default(),
                    here: sender.is_some(),
                    text: said.message.text().into(),
                    styled: row.styled.clone(),
                    embeds: row.embeds.clone(),
                    time,
                    head,
                }
            }
            Line::Note { about, text } => ui::ChatLine {
                number,
                about: about_in_ui(*about),
                text: text.as_str().into(),
                time,
                ..Default::default()
            },
        }
    }
}

fn sender_here(said: &Said, here: &impl Fn(PeerId) -> bool) -> Option<i32> {
    match said.mine {
        true => Some(MY_ID),
        false => said.from.filter(|id| here(*id)).map(i32::from),
    }
}

fn embed(url: &Url, preview: &LinkPreview) -> ui::Embed {
    let picture = preview.picture.as_ref();
    ui::Embed {
        url: url.as_str().into(),
        site: preview.site.as_str().into(),
        title: preview.title.as_deref().unwrap_or_default().into(),
        description: preview.description.as_deref().unwrap_or_default().into(),
        picture: picture.map(image_of).unwrap_or_default(),
        has_picture: picture.is_some(),
        wide: picture.is_some_and(|p| p.width() >= WIDE_PICTURE),
        colour: preview
            .colour
            .map(|rgb| slint::Color::from_argb_encoded(0xff00_0000 | rgb))
            .unwrap_or_default(),
        has_colour: preview.colour.is_some(),
    }
}

fn image_of(picture: &Picture) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        picture.rgba(),
        picture.width(),
        picture.height(),
    ))
}

fn styled(message: &Message) -> StyledText {
    let mut markdown = String::new();
    for piece in message.pieces() {
        match piece {
            Piece::Plain(text) => escape_into(&mut markdown, text),
            Piece::Link { shown, url } => {
                markdown.push('[');
                escape_into(&mut markdown, shown);
                markdown.push_str("](<");
                escape_into(&mut markdown, url.as_str());
                markdown.push_str(">)");
            }
        }
    }
    StyledText::from_markdown(&markdown)
        .unwrap_or_else(|_| StyledText::from_plain_text(message.text()))
}

fn escape_into(markdown: &mut String, text: &str) {
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            markdown.push('\\');
        }
        markdown.push(c);
    }
}

fn about_in_ui(about: About) -> ui::About {
    match about {
        About::Joined => ui::About::Joined,
        About::Left => ui::About::Left,
        About::Kicked => ui::About::Kicked,
        About::Host => ui::About::Host,
        About::Locked => ui::About::Locked,
        About::Unlocked => ui::About::Unlocked,
        About::Size => ui::About::Size,
        About::Sharing => ui::About::Sharing,
        About::StoppedSharing => ui::About::StoppedSharing,
        About::Renamed => ui::About::Renamed,
    }
}

fn one_group(before: &ChatLine, after: &ChatLine) -> bool {
    match (&before.what, &after.what) {
        (Line::Said(first), Line::Said(next)) => {
            first.who == next.who
                && first.mine == next.mine
                && (after.at - before.at).num_seconds() <= ONE_GROUP_WITHIN_SECONDS
        }
        _ => false,
    }
}

fn heads(lines: &[ChatLine]) -> Vec<bool> {
    (0..lines.len())
        .map(|at| at == 0 || !one_group(&lines[at - 1], &lines[at]))
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeDelta};
    use whisperia_proto::control::{Avatar, ChatText, Name};

    use super::*;

    fn message(text: &str) -> Message {
        Message::from(&ChatText::new(text).unwrap())
    }

    fn line(number: u64, minute: i64, what: Line) -> ChatLine {
        ChatLine {
            number,
            at: Local::now() + TimeDelta::minutes(minute),
            what,
        }
    }

    fn said_by(from: Option<PeerId>, who: &str, mine: bool) -> Said {
        Said {
            from,
            who: Name::new(who).unwrap(),
            avatar: Avatar::None,
            message: message("hi"),
            mine,
        }
    }

    fn said(who: &str, mine: bool) -> Line {
        Line::Said(said_by(None, who, mine))
    }

    #[test]
    fn a_message_belongs_to_its_sender_while_they_are_in_the_room() {
        let fox = said_by(Some(7), "Fox", false);
        assert_eq!(sender_here(&fox, &|id| id == 7), Some(7));
        assert_eq!(sender_here(&fox, &|_| false), None);
    }

    #[test]
    fn ones_own_message_belongs_to_oneself() {
        let mine = said_by(Some(3), "Panda", true);
        assert_eq!(sender_here(&mine, &|_| false), Some(MY_ID));
    }

    #[test]
    fn lines_of_one_person_close_in_time_make_one_group() {
        let note = Line::Note {
            about: About::Joined,
            text: "Octopus joined".into(),
        };
        let lines = [
            line(0, 0, said("Fox", false)),
            line(1, 1, said("Fox", false)),
            line(2, 2, said("Panda", true)),
            line(3, 3, note),
            line(4, 3, said("Panda", true)),
            line(5, 20, said("Panda", true)),
            line(6, 21, said("Panda", true)),
        ];
        assert_eq!(heads(&lines), [true, false, true, true, true, true, false]);
    }

    #[test]
    fn ordinary_text_is_shown_exactly_as_written() {
        for text in [
            "*not bold* _nor this_ ~~nor this~~",
            "# not a heading",
            "1. not a list",
            "- nor this one",
            "> not a quote",
            "`no code` and [no](link) & &amp; <b>no tags</b>",
            "back\\slash, 100% {braces} \u{e541}",
            "spaces  stay   as   they are",
            "\u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}, \u{1f600}",
        ] {
            assert_eq!(
                styled(&message(text)),
                StyledText::from_plain_text(text),
                "{text}"
            );
        }
    }

    #[test]
    fn links_are_styled_as_links() {
        for text in [
            "see https://example.org/a_b?c=(1)&d=*e*",
            "www.rust-lang.org",
            "two: https://a.org and https://b.org/[x]",
        ] {
            assert_ne!(
                styled(&message(text)),
                StyledText::from_plain_text(text),
                "{text}"
            );
        }
    }
}
