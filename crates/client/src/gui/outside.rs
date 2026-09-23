use crate::app::Url;

pub(super) trait Outside {
    fn open(&self, url: &Url);
    fn copy(&self, text: &str);
}

pub(super) struct Desk;

impl Outside for Desk {
    fn open(&self, url: &Url) {
        #[cfg(windows)]
        let opened = std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url.as_str()])
            .spawn();
        #[cfg(not(windows))]
        let opened = std::process::Command::new("xdg-open")
            .arg(url.as_str())
            .spawn();
        if let Err(e) = opened {
            tracing::warn!("cannot open {url}: {e}");
        }
    }

    fn copy(&self, text: &str) {
        #[cfg(windows)]
        if let Err(e) = arboard::Clipboard::new().and_then(|mut board| board.set_text(text)) {
            tracing::warn!("cannot copy: {e}");
        }
        #[cfg(not(windows))]
        tracing::warn!("copying is not available here: {} characters", text.len());
    }
}

#[cfg(feature = "testing")]
#[derive(Default)]
pub(super) struct Handed {
    pub(super) opened: std::cell::RefCell<Vec<String>>,
    pub(super) copied: std::cell::RefCell<Vec<String>>,
}

#[cfg(feature = "testing")]
impl Outside for Handed {
    fn open(&self, url: &Url) {
        self.opened.borrow_mut().push(url.to_string());
    }

    fn copy(&self, text: &str) {
        self.copied.borrow_mut().push(text.to_owned());
    }
}
