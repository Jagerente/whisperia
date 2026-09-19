const MAX_TITLE_CHARS: usize = 120;
const MAX_DESCRIPTION_CHARS: usize = 300;
const MAX_SITE_CHARS: usize = 60;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub site: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub picture: Option<String>,
    pub colour: Option<u32>,
}

impl Page {
    pub fn read(html: &str) -> Self {
        let head = head_of(html);
        let metas: Vec<(String, String)> = tags(head, "meta")
            .filter_map(|attributes| {
                let key =
                    attribute(attributes, "property").or_else(|| attribute(attributes, "name"))?;
                let content = attribute(attributes, "content")?;
                Some((key.to_ascii_lowercase(), content))
            })
            .collect();
        let meta = |keys: &[&str]| {
            keys.iter().find_map(|key| {
                metas
                    .iter()
                    .find(|(found, content)| found == key && !content.trim().is_empty())
                    .map(|(_, content)| content.clone())
            })
        };
        Self {
            site: meta(&["og:site_name"]).and_then(|site| clean(&site, MAX_SITE_CHARS)),
            title: meta(&["og:title", "twitter:title"])
                .or_else(|| title_element(head))
                .and_then(|title| clean(&title, MAX_TITLE_CHARS)),
            description: meta(&["og:description", "twitter:description", "description"])
                .and_then(|description| clean(&description, MAX_DESCRIPTION_CHARS)),
            picture: meta(&[
                "og:image:secure_url",
                "og:image:url",
                "og:image",
                "twitter:image",
                "twitter:image:src",
            ])
            .map(|picture| picture.trim().to_owned()),
            colour: meta(&["theme-color"]).and_then(|colour| colour_of(&colour)),
        }
    }

    pub fn has_anything(&self) -> bool {
        self.title.is_some() || self.description.is_some()
    }
}

fn head_of(html: &str) -> &str {
    match find_ignoring_case(html, "</head") {
        Some(end) => &html[..end],
        None => html,
    }
}

fn find_ignoring_case(text: &str, needle: &str) -> Option<usize> {
    text.as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn tags<'a>(html: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    let mut rest = html;
    std::iter::from_fn(move || loop {
        let start = rest.find('<')?;
        let after = &rest[start + 1..];
        let named = after
            .get(..name.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(name))
            && after[name.len()..]
                .starts_with(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>');
        let Some(end) = tag_end(after) else {
            rest = "";
            return None;
        };
        let inside = &after[..end];
        rest = &after[end + 1..];
        if named {
            return Some(&inside[name.len()..]);
        }
    })
}

fn tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (at, c) in tag.char_indices() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(at),
            (None, _) => {}
        }
    }
    None
}

fn attribute(attributes: &str, wanted: &str) -> Option<String> {
    let mut rest = attributes;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if rest.is_empty() {
            return None;
        }
        let name_end = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
            .unwrap_or(rest.len());
        let name = &rest[..name_end];
        rest = rest[name_end..].trim_start();
        let value = match rest.strip_prefix('=') {
            Some(after) => {
                let after = after.trim_start();
                let (value, left) = match after.chars().next() {
                    Some(quote @ ('"' | '\'')) => {
                        let inner = &after[1..];
                        let end = inner.find(quote).unwrap_or(inner.len());
                        (&inner[..end], inner.get(end + 1..).unwrap_or(""))
                    }
                    _ => {
                        let end = after
                            .find(|c: char| c.is_ascii_whitespace())
                            .unwrap_or(after.len());
                        (&after[..end], &after[end..])
                    }
                };
                rest = left;
                value
            }
            None => "",
        };
        if name.eq_ignore_ascii_case(wanted) {
            return Some(decode_entities(value));
        }
    }
}

fn title_element(head: &str) -> Option<String> {
    let start = find_ignoring_case(head, "<title")?;
    let open_end = head[start..].find('>')? + start + 1;
    let close = find_ignoring_case(&head[open_end..], "</title")? + open_end;
    Some(decode_entities(&head[open_end..close]))
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest
            .find(';')
            .filter(|end| *end <= 10)
            .and_then(|end| entity(&rest[1..end]).map(|c| (c, end)));
        match entity {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

fn clean(text: &str, most: usize) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let joined = words.join(" ");
    let visible: String = joined.chars().filter(|c| !c.is_control()).collect();
    if visible.is_empty() {
        return None;
    }
    if visible.chars().count() <= most {
        return Some(visible);
    }
    let cut: String = visible.chars().take(most - 3).collect();
    Some(format!("{}...", cut.trim_end()))
}

fn colour_of(text: &str) -> Option<u32> {
    let hex = text.trim().strip_prefix('#')?;
    let full = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_owned(),
        _ => return None,
    };
    u32::from_str_radix(&full, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_graph_tags_give_the_site_title_description_picture_and_colour() {
        let page = Page::read(
            r##"<!doctype html><html><head>
            <title>Ignored when og:title is there</title>
            <meta property="og:site_name" content="GitHub">
            <meta property="og:title" content="jagerente/whisperia">
            <meta property="og:description" content="Voice chat with screen sharing.">
            <meta property="og:image" content="https://example.org/card.png" />
            <meta name="theme-color" content="#1e2433">
            </head><body><meta property="og:title" content="not in the head"></body></html>"##,
        );
        assert_eq!(
            page,
            Page {
                site: Some("GitHub".into()),
                title: Some("jagerente/whisperia".into()),
                description: Some("Voice chat with screen sharing.".into()),
                picture: Some("https://example.org/card.png".into()),
                colour: Some(0x1e2433),
            }
        );
    }

    #[test]
    fn a_page_without_open_graph_falls_back_to_its_title_and_description() {
        let page = Page::read(
            "<HEAD><TITLE>\n  Plain   page \n</TITLE><META NAME=description CONTENT='Just a page'></HEAD>",
        );
        assert_eq!(page.title.as_deref(), Some("Plain page"));
        assert_eq!(page.description.as_deref(), Some("Just a page"));
        assert!(page.has_anything());
    }

    #[test]
    fn entities_are_decoded_and_unknown_ones_kept() {
        let page = Page::read(
            r#"<meta property="og:title" content="Tom &amp; Jerry &#8212; &#x41;&quot;B&quot; &copy &unknown;">"#,
        );
        assert_eq!(
            page.title.as_deref(),
            Some("Tom & Jerry \u{2014} A\"B\" &copy &unknown;")
        );
    }

    #[test]
    fn a_quote_inside_another_quote_does_not_end_the_tag() {
        let page = Page::read(r#"<meta content="a > b, it's 'fine'" property='og:title'>"#);
        assert_eq!(page.title.as_deref(), Some("a > b, it's 'fine'"));
    }

    #[test]
    fn long_texts_are_cut_and_control_characters_dropped() {
        let long = "word ".repeat(100);
        let page = Page::read(&format!(
            "<meta property=\"og:title\" content=\"{long}\"><meta property=\"og:description\" content=\"a\u{7}b\">"
        ));
        let title = page.title.unwrap();
        assert!(title.chars().count() <= MAX_TITLE_CHARS);
        assert!(title.ends_with("..."));
        assert_eq!(page.description.as_deref(), Some("ab"));
    }

    #[test]
    fn a_page_with_nothing_to_say_has_nothing() {
        let page = Page::read("<html><head><meta charset=utf-8></head><body>hi</body></html>");
        assert!(!page.has_anything());
        assert_eq!(
            Page::read("<meta property=\"og:title\" content=\"  \">"),
            Page::default()
        );
    }

    #[test]
    fn a_colour_is_three_or_six_hex_digits() {
        assert_eq!(colour_of("#fff"), Some(0xffffff));
        assert_eq!(colour_of(" #FF0054 "), Some(0xff0054));
        assert_eq!(colour_of("red"), None);
        assert_eq!(colour_of("#12345"), None);
    }

    #[test]
    fn a_broken_tag_ends_the_search() {
        let page = Page::read("<meta property=\"og:title\" content=\"never closed");
        assert_eq!(page, Page::default());
    }
}
