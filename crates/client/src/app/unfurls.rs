use std::{
    collections::{HashMap, HashSet},
    sync::{mpsc::Receiver, Arc},
};

use super::message::{Message, Url};
use crate::{
    preview::{LinkPreview, Previews},
    wake::{self, Waker, WakingSender},
};

const MOST_PER_MESSAGE: usize = 3;
const MOST_KEPT: usize = 256;

enum Unfurl {
    Asking,
    Shown(Arc<LinkPreview>),
    Nothing,
}

pub(super) struct Unfurls {
    previews: Arc<dyn Previews>,
    known: HashMap<Url, Unfurl>,
    answers_to: WakingSender<(Url, Option<LinkPreview>)>,
    answers: Receiver<(Url, Option<LinkPreview>)>,
}

impl Unfurls {
    pub(super) fn new(previews: Arc<dyn Previews>, waker: &Waker) -> Self {
        let (answers_to, answers) = wake::channel(waker);
        Self {
            previews,
            known: HashMap::new(),
            answers_to,
            answers,
        }
    }

    pub(super) fn ask(&mut self, runtime: &tokio::runtime::Runtime, message: &Message) {
        for url in message.links().take(MOST_PER_MESSAGE) {
            if self.known.contains_key(url) {
                continue;
            }
            self.known.insert(url.clone(), Unfurl::Asking);
            let previews = self.previews.clone();
            let answer = self.answers_to.clone();
            let url = url.clone();
            runtime.spawn_blocking(move || {
                let preview = previews
                    .preview(&url)
                    .inspect_err(|e| tracing::debug!("no preview: {e}"))
                    .ok();
                let _ = answer.send((url, preview));
            });
        }
    }

    pub(super) fn heard(&mut self) -> bool {
        let answers: Vec<_> = self.answers.try_iter().collect();
        let heard = !answers.is_empty();
        for (url, preview) in answers {
            self.learn(url, preview);
        }
        heard
    }

    fn learn(&mut self, url: Url, preview: Option<LinkPreview>) {
        let unfurl = preview.map_or(Unfurl::Nothing, |p| Unfurl::Shown(Arc::new(p)));
        self.known.insert(url, unfurl);
    }

    pub(super) fn of(&self, url: &Url) -> Option<&Arc<LinkPreview>> {
        match self.known.get(url) {
            Some(Unfurl::Shown(preview)) => Some(preview),
            _ => None,
        }
    }

    pub(super) fn too_many(&self) -> bool {
        self.known.len() > MOST_KEPT
    }

    pub(super) fn keep_only(&mut self, wanted: &HashSet<&Url>) {
        self.known.retain(|url, _| wanted.contains(url));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use whisperia_proto::control::ChatText;

    use super::*;
    use crate::preview::fake::FakePreviews;

    fn message(text: &str) -> Message {
        Message::from(&ChatText::new(text).unwrap())
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap()
    }

    fn page(title: &str) -> LinkPreview {
        LinkPreview {
            site: "example.org".into(),
            title: Some(title.into()),
            description: None,
            picture: None,
            colour: None,
        }
    }

    fn wait(unfurls: &mut Unfurls, answers: usize) {
        let until = Instant::now() + Duration::from_secs(5);
        let mut heard = 0;
        while heard < answers {
            assert!(Instant::now() < until, "the previews did not come");
            let (url, preview) = unfurls
                .answers
                .recv_timeout(until - Instant::now())
                .expect("a preview answer");
            unfurls.learn(url, preview);
            heard += 1;
        }
    }

    #[test]
    fn each_address_is_asked_for_once_and_its_preview_kept() {
        let fake = Arc::new(FakePreviews::default().with("https://example.org/a", page("A")));
        let mut unfurls = Unfurls::new(fake.clone(), &Waker::default());
        let runtime = runtime();
        unfurls.ask(
            &runtime,
            &message("see https://example.org/a and https://example.org/b"),
        );
        unfurls.ask(&runtime, &message("again https://example.org/a"));
        wait(&mut unfurls, 2);
        let mut asked = fake.asked();
        asked.sort();
        assert_eq!(asked, ["https://example.org/a", "https://example.org/b"]);
        let a = Url::parse("https://example.org/a").unwrap();
        let b = Url::parse("https://example.org/b").unwrap();
        assert_eq!(unfurls.of(&a).unwrap().title.as_deref(), Some("A"));
        assert!(unfurls.of(&b).is_none());
    }

    #[test]
    fn a_message_has_at_most_three_previews() {
        let fake = Arc::new(FakePreviews::default());
        let mut unfurls = Unfurls::new(fake.clone(), &Waker::default());
        let runtime = runtime();
        unfurls.ask(
            &runtime,
            &message("https://a.org https://b.org https://c.org https://d.org"),
        );
        wait(&mut unfurls, 3);
        assert_eq!(fake.asked().len(), 3);
    }
}
