use super::Message;
use crate::dsp::cue::Cue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Said {
    Info(String),
    Error(String),
}

#[derive(Default)]
pub(super) struct Outbox {
    pub(super) cues: Vec<Cue>,
    pub(super) said: Vec<Said>,
    pub(super) unfurl: Vec<Message>,
    pub(super) changed: bool,
}

impl Outbox {
    pub(super) fn info(&mut self, text: impl Into<String>) {
        self.said.push(Said::Info(text.into()));
    }

    pub(super) fn error(&mut self, text: impl Into<String>) {
        self.said.push(Said::Error(text.into()));
    }

    pub(super) fn cue(&mut self, cue: Cue) {
        self.cues.push(cue);
    }

    pub(super) fn unfurl(&mut self, message: &Message) {
        self.unfurl.push(message.clone());
    }

    pub(super) fn changed(&mut self) {
        self.changed = true;
    }
}
