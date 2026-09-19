use std::{collections::HashMap, sync::Mutex};

use super::{LinkPreview, PreviewError, Previews};
use crate::app::message::Url;

#[derive(Default)]
pub struct FakePreviews {
    pages: Mutex<HashMap<String, LinkPreview>>,
    asked: Mutex<Vec<String>>,
}

impl FakePreviews {
    pub fn with(self, url: &str, preview: LinkPreview) -> Self {
        self.pages
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(url.to_owned(), preview);
        self
    }

    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl Previews for FakePreviews {
    fn preview(&self, url: &Url) -> Result<LinkPreview, PreviewError> {
        self.asked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(url.to_string());
        self.pages
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(url.as_str())
            .cloned()
            .ok_or_else(|| PreviewError::Empty(url.to_string()))
    }
}
