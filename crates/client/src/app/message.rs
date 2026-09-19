use std::fmt;

use whisperia_proto::control::ChatText;

const NOT_IN_URLS: &str = "<>\"`\\";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Url(String);

impl Url {
    pub fn parse(text: &str) -> Option<Self> {
        let rest = strip_scheme(text)?;
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let fine = |c: char| !c.is_whitespace() && !c.is_control() && !NOT_IN_URLS.contains(c);
        (!host.is_empty() && text.chars().all(fine)).then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn site(&self) -> &str {
        let rest = strip_scheme(&self.0).unwrap_or(&self.0);
        let place = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = place.rsplit('@').next().unwrap_or(place);
        match host.get(..4) {
            Some(www) if www.eq_ignore_ascii_case("www.") => &host[4..],
            _ => host,
        }
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Plain(String),
    Link { shown: String, url: Url },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    text: String,
    pieces: Vec<Piece>,
}

impl Message {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    pub fn links(&self) -> impl Iterator<Item = &Url> {
        self.pieces.iter().filter_map(|piece| match piece {
            Piece::Link { url, .. } => Some(url),
            Piece::Plain(_) => None,
        })
    }
}

impl From<&ChatText> for Message {
    fn from(text: &ChatText) -> Self {
        let text = text.to_string();
        Self {
            pieces: pieces(&text),
            text,
        }
    }
}

fn strip_scheme(text: &str) -> Option<&str> {
    ["https://", "http://"].iter().find_map(|scheme| {
        let head = text.get(..scheme.len())?;
        head.eq_ignore_ascii_case(scheme)
            .then(|| &text[scheme.len()..])
    })
}

fn pieces(text: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let at_word_start = plain
            .chars()
            .last()
            .is_none_or(|before| !before.is_alphanumeric());
        match at_word_start.then(|| link_at(rest)).flatten() {
            Some((shown, url)) => {
                if !plain.is_empty() {
                    pieces.push(Piece::Plain(std::mem::take(&mut plain)));
                }
                rest = &rest[shown.len()..];
                pieces.push(Piece::Link {
                    shown: shown.to_owned(),
                    url,
                });
            }
            None => {
                let next = rest.chars().next().map_or(1, char::len_utf8);
                plain.push_str(&rest[..next]);
                rest = &rest[next..];
            }
        }
    }
    if !plain.is_empty() {
        pieces.push(Piece::Plain(plain));
    }
    pieces
}

fn link_at(text: &str) -> Option<(&str, Url)> {
    let www = text
        .get(..4)
        .is_some_and(|head| head.eq_ignore_ascii_case("www."));
    if strip_scheme(text).is_none() && !www {
        return None;
    }
    let end = text
        .find(|c: char| c.is_whitespace() || NOT_IN_URLS.contains(c))
        .unwrap_or(text.len());
    let shown = trim_end(&text[..end]);
    let url = match www {
        true if shown.len() > "www.".len() => Url::parse(&format!("https://{shown}"))?,
        true => return None,
        false => Url::parse(shown)?,
    };
    let host = strip_scheme(url.as_str()).unwrap_or_default();
    host.contains(|c: char| c.is_alphanumeric())
        .then_some((shown, url))
}

fn trim_end(link: &str) -> &str {
    let mut link = link;
    loop {
        let Some(last) = link.chars().last() else {
            return link;
        };
        let unmatched = |open: char, close: char| {
            last == close && link.matches(close).count() > link.matches(open).count()
        };
        if ".,;:!?'*".contains(last)
            || unmatched('(', ')')
            || unmatched('[', ']')
            || unmatched('{', '}')
        {
            link = &link[..link.len() - last.len_utf8()];
        } else {
            return link;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Vec<(bool, String)> {
        Message::from(&ChatText::new(text).unwrap())
            .pieces
            .into_iter()
            .map(|piece| match piece {
                Piece::Plain(text) => (false, text),
                Piece::Link { shown, url } => {
                    assert_eq!(
                        url.as_str().trim_start_matches("https://"),
                        shown.trim_start_matches("https://")
                    );
                    (true, shown)
                }
            })
            .collect()
    }

    fn link(text: &str) -> (bool, String) {
        (true, text.to_owned())
    }

    fn plain(text: &str) -> (bool, String) {
        (false, text.to_owned())
    }

    #[test]
    fn links_are_found_where_they_are_written() {
        assert_eq!(
            read("see https://github.com/Jagerente/whisperia now"),
            [
                plain("see "),
                link("https://github.com/Jagerente/whisperia"),
                plain(" now")
            ]
        );
        assert_eq!(
            read("http://192.168.1.52:8080"),
            [link("http://192.168.1.52:8080")]
        );
        assert_eq!(read("HTTPS://Example.com"), [link("HTTPS://Example.com")]);
        assert_eq!(
            read("www.rust-lang.org and more"),
            [link("www.rust-lang.org"), plain(" and more")]
        );
    }

    #[test]
    fn a_link_leaves_the_punctuation_of_the_sentence_behind() {
        assert_eq!(
            read("look: https://example.com/a."),
            [plain("look: "), link("https://example.com/a"), plain(".")]
        );
        assert_eq!(
            read("(https://example.com/wiki/Rust_(language))"),
            [
                plain("("),
                link("https://example.com/wiki/Rust_(language)"),
                plain(")")
            ]
        );
        assert_eq!(
            read("\"https://example.com/?q=1&b=2\""),
            [
                plain("\""),
                link("https://example.com/?q=1&b=2"),
                plain("\"")
            ]
        );
    }

    #[test]
    fn what_only_looks_like_a_link_stays_text() {
        for text in [
            "https://",
            "https:// example.com",
            "http://.../",
            "ftp://example.com",
            "javascript:alert(1)",
            "nothttps://example.com",
            "www.",
        ] {
            assert!(
                read(text).iter().all(|(is_link, _)| !is_link),
                "{text}: {:?}",
                read(text)
            );
        }
    }

    #[test]
    fn a_url_names_its_site() {
        let site = |text: &str| Url::parse(text).map(|url| url.site().to_owned());
        assert_eq!(
            site("https://www.github.com/Jagerente/whisperia").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            site("http://192.168.1.52:8080/a?b").as_deref(),
            Some("192.168.1.52:8080")
        );
        assert_eq!(
            site("https://me@example.com#top").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn only_web_addresses_are_urls() {
        assert!(Url::parse("https://example.com/a b").is_none());
        assert!(Url::parse("file:///C:/Windows").is_none());
        assert!(Url::parse("https://").is_none());
        assert!(Url::parse("https://example.com/\"onclick").is_none());
        assert_eq!(
            Url::parse("https://example.com/a?b#c").map(|u| u.to_string()),
            Some("https://example.com/a?b#c".to_owned())
        );
    }
}
