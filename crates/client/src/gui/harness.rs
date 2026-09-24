use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
    time::Duration,
};

use i_slint_backend_testing::{
    AccessibleRole, ElementHandle, ElementQuery, TestingBackend, TestingBackendOptions,
};
use slint::ComponentHandle;

use super::{
    outside::Handed,
    ui::{AppWindow, State, Theme},
    Front, Options,
};

const INSIDE_THE_FIRST_LETTER: f32 = 6.0;

pub struct Hands {
    pub(super) window: AppWindow,
    pub(super) pictures: Rc<Cell<u64>>,
}

fn is_text(element: &ElementHandle) -> bool {
    element.accessible_role() == Some(AccessibleRole::Text)
}

fn the_one(label: &str, mut found: Vec<ElementHandle>) -> Option<ElementHandle> {
    if found.len() > 1 {
        let places: Vec<String> = found
            .iter()
            .map(|element| {
                let (at, size) = (element.absolute_position(), element.size());
                format!(
                    "{:?} at ({}, {}) {}x{}",
                    element.accessible_role(),
                    at.x,
                    at.y,
                    size.width,
                    size.height
                )
            })
            .collect();
        panic!(
            "\"{label}\" is the label of {} elements, a test cannot tell which one is meant: {}",
            found.len(),
            places.join(", ")
        );
    }
    found.pop()
}

fn contains(outer: &ElementHandle, inner: &ElementHandle) -> bool {
    let (at, size) = (outer.absolute_position(), outer.size());
    let (inner_at, inner_size) = (inner.absolute_position(), inner.size());
    let (x, y) = (
        inner_at.x + inner_size.width / 2.0,
        inner_at.y + inner_size.height / 2.0,
    );
    x >= at.x && x <= at.x + size.width && y >= at.y && y <= at.y + size.height
}

impl Hands {
    fn all_labelled(&self, label: &str) -> Vec<ElementHandle> {
        ElementHandle::find_by_accessible_label(&self.window, label).collect()
    }

    fn labelled(&self, label: &str) -> Option<ElementHandle> {
        let found = self
            .all_labelled(label)
            .into_iter()
            .filter(|element| !is_text(element))
            .collect();
        the_one(label, found)
    }

    fn find(&self, label: &str) -> Option<ElementHandle> {
        let (texts, others): (Vec<_>, Vec<_>) =
            self.all_labelled(label).into_iter().partition(is_text);
        if others.is_empty() {
            the_one(label, texts)
        } else {
            the_one(label, others)
        }
    }

    fn texts_where(&self, keep: impl Fn(&ElementHandle) -> bool) -> Vec<String> {
        ElementQuery::from_root(&self.window)
            .find_all()
            .into_iter()
            .filter(|element| is_text(element) && keep(element))
            .filter_map(|element| element.accessible_label())
            .filter(|label| !label.is_empty())
            .map(Into::into)
            .collect()
    }

    pub fn has(&self, label: &str) -> bool {
        self.all_labelled(label)
            .iter()
            .any(|element| !is_text(element))
    }

    pub fn texts(&self) -> Vec<String> {
        self.texts_where(|_| true)
    }

    pub fn count_text(&self, text: &str) -> usize {
        self.all_labelled(text)
            .iter()
            .filter(|element| is_text(element))
            .count()
    }

    pub fn has_text(&self, text: &str) -> bool {
        self.count_text(text) > 0
    }

    pub fn value_of(&self, label: &str) -> Option<String> {
        self.labelled(label)
            .and_then(|element| element.accessible_value())
            .map(Into::into)
    }

    pub fn values_of(&self, label: &str) -> Vec<String> {
        self.all_labelled(label)
            .iter()
            .filter(|element| !is_text(element))
            .filter_map(|element| element.accessible_value())
            .map(Into::into)
            .collect()
    }

    pub fn is_on(&self, label: &str) -> Option<bool> {
        let element = self.labelled(label)?;
        let checkable = element.accessible_checkable() == Some(true);
        match element.accessible_checked() {
            Some(true) => Some(true),
            Some(false) if checkable => Some(false),
            _ => None,
        }
    }

    pub fn texts_within(&self, label: &str) -> Vec<String> {
        let Some(outer) = self.find(label) else {
            return Vec::new();
        };
        self.texts_where(|element| contains(&outer, element))
    }

    fn inside(&self, component: &str) -> Vec<ElementHandle> {
        ElementQuery::from_root(&self.window)
            .match_inherits(component)
            .find_first()
            .map(|outer| {
                outer
                    .query_descendants()
                    .find_all()
                    .into_iter()
                    .filter(|element| !is_text(element))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn labels_starting_with(&self, start: &str) -> Vec<String> {
        ElementQuery::from_root(&self.window)
            .find_all()
            .into_iter()
            .filter(|element| !is_text(element))
            .filter_map(|element| element.accessible_label())
            .map(String::from)
            .filter(|label| label.starts_with(start))
            .collect()
    }

    pub fn text_bounds(&self, text: &str) -> Vec<(f32, f32, f32, f32)> {
        self.all_labelled(text)
            .iter()
            .filter(|element| is_text(element))
            .map(|element| {
                let (at, size) = (element.absolute_position(), element.size());
                (at.x, at.y, size.width, size.height)
            })
            .collect()
    }

    pub fn labels_in(&self, component: &str) -> Vec<String> {
        self.inside(component)
            .into_iter()
            .filter_map(|element| element.accessible_label())
            .filter(|label| !label.is_empty())
            .map(Into::into)
            .collect()
    }

    pub fn screen(&self) -> String {
        let mut said = String::new();
        for element in ElementQuery::from_root(&self.window).find_all() {
            let (Some(role), Some(label)) = (element.accessible_role(), element.accessible_label())
            else {
                continue;
            };
            if label.is_empty() {
                continue;
            }
            match role {
                AccessibleRole::Text => said.push_str(&label),
                role => {
                    let role = format!("{role:?}").to_lowercase();
                    said.push_str(&format!("[{role}] {label}"));
                    if let Some(value) = element.accessible_value().filter(|v| !v.is_empty()) {
                        said.push_str(&format!(" = {value}"));
                    }
                    match element.accessible_checked() {
                        Some(true) => said.push_str(" (on)"),
                        Some(false) if element.accessible_checkable() == Some(true) => {
                            said.push_str(" (off)")
                        }
                        _ => {}
                    }
                }
            }
            said.push('\n');
        }
        said
    }

    pub fn press(&mut self, label: &str) -> bool {
        let Some(element) = self.labelled(label) else {
            return false;
        };
        element.invoke_accessible_default_action();
        true
    }

    pub fn type_into(&mut self, label: &str, text: &str) -> bool {
        let Some(element) = self.labelled(label) else {
            return false;
        };
        element.set_accessible_value(text);
        true
    }

    pub fn slide(&mut self, label: &str, value: f32) -> bool {
        self.type_into(label, &value.to_string())
    }

    pub fn middle_of(&self, label: &str) -> Option<(f32, f32)> {
        let element = self.find(label)?;
        let (at, size) = (element.absolute_position(), element.size());
        Some((at.x + size.width / 2.0, at.y + size.height / 2.0))
    }

    pub fn bounds(&self, label: &str) -> Option<(f32, f32, f32, f32)> {
        let element = self.find(label)?;
        let (at, size) = (element.absolute_position(), element.size());
        Some((at.x, at.y, size.width, size.height))
    }

    pub fn hover(&mut self, label: &str) -> bool {
        let Some((x, y)) = self.middle_of(label) else {
            return false;
        };
        self.window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(x, y),
            });
        true
    }

    pub fn tap(&mut self, label: &str) -> bool {
        let Some((x, y)) = self.middle_of(label) else {
            return false;
        };
        self.click_at(x, y, slint::platform::PointerEventButton::Left);
        true
    }

    pub fn tap_start(&mut self, label: &str) -> bool {
        let Some((x, y, ..)) = self.bounds(label) else {
            return false;
        };
        self.click_at(
            x + INSIDE_THE_FIRST_LETTER,
            y + INSIDE_THE_FIRST_LETTER,
            slint::platform::PointerEventButton::Left,
        );
        true
    }

    pub fn tap_at(&mut self, x: f32, y: f32) {
        self.click_at(x, y, slint::platform::PointerEventButton::Left);
    }

    fn click_at(&mut self, x: f32, y: f32, button: slint::platform::PointerEventButton) {
        let position = slint::LogicalPosition::new(x, y);
        super::pointer_pressed(&self.window, x, y);
        let window = self.window.window();
        window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
        window.dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
        window.dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
    }

    pub fn scroll(&mut self, label: &str, down: f32) -> bool {
        let Some((x, y)) = self.middle_of(label) else {
            return false;
        };
        self.window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(x, y),
                delta_x: 0.0,
                delta_y: -down,
            });
        true
    }

    pub fn mouse_leaves(&mut self) {
        self.window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerExited);
    }

    pub fn escape(&mut self) {
        self.key(slint::platform::Key::Escape);
    }

    pub fn tab(&mut self) {
        self.key(slint::platform::Key::Tab);
    }

    pub fn enter(&mut self) {
        self.key(slint::platform::Key::Return);
    }

    pub fn type_text(&mut self, text: &str) {
        let window = self.window.window();
        for letter in text.chars() {
            let text: slint::SharedString = letter.to_string().into();
            window.dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.clone() });
            window.dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
        }
    }

    pub fn space(&mut self) {
        self.key(slint::platform::Key::Space);
    }

    pub fn down(&mut self) {
        self.key(slint::platform::Key::DownArrow);
    }

    pub fn f11(&mut self) {
        self.key(slint::platform::Key::F11);
    }

    pub fn control(&mut self, letter: char) {
        let control: slint::SharedString = slint::platform::Key::Control.into();
        let letter: slint::SharedString = letter.to_string().into();
        let window = self.window.window();
        window.dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: control.clone(),
        });
        window.dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: letter.clone(),
        });
        window.dispatch_event(slint::platform::WindowEvent::KeyReleased { text: letter });
        window.dispatch_event(slint::platform::WindowEvent::KeyReleased { text: control });
    }

    fn key(&mut self, key: slint::platform::Key) {
        let text: slint::SharedString = key.into();
        let window = self.window.window();
        window.dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.clone() });
        window.dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
    }

    pub fn pictures_shown(&self) -> u64 {
        self.pictures.get()
    }

    fn let_animations_end(&self) {
        let theme = self.window.global::<Theme>();
        let longest = theme.get_quick().max(theme.get_fade());
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(
            u64::try_from(longest).unwrap_or_default(),
        ));
    }

    pub fn use_dark_colours(&mut self, dark: bool) {
        self.window.global::<Theme>().set_scheme(if dark {
            slint::language::ColorScheme::Dark
        } else {
            slint::language::ColorScheme::Light
        });
    }

    pub fn in_dark_colours(&self) -> bool {
        self.window.global::<Theme>().get_dark()
    }

    pub fn notice(&self) -> Option<String> {
        let notice = self.window.global::<State>().get_notice();
        (!notice.is_empty()).then(|| notice.into())
    }

    pub fn colour(&self, token: Token) -> (u8, u8, u8) {
        let theme = self.window.global::<Theme>();
        let colour = match token {
            Token::Window => theme.get_window(),
            Token::Surface => theme.get_surface(),
            Token::Raised => theme.get_raised(),
            Token::RaisedHover => theme.get_raised_hover(),
            Token::Text => theme.get_text(),
            Token::TextDim => theme.get_text_dim(),
            Token::TextFaint => theme.get_text_faint(),
            Token::Good => theme.get_good(),
            Token::Bad => theme.get_bad(),
            Token::Warn => theme.get_warn(),
            Token::Gold => theme.get_gold(),
            Token::OnAccent => theme.get_on_accent(),
            Token::GoodFill => theme.get_good_fill(),
            Token::BadFill => theme.get_bad_fill(),
            Token::Toast => theme.get_toast(),
            Token::Menu => theme.get_menu(),
            Token::MenuHover => theme.get_menu_hover(),
        };
        (colour.red(), colour.green(), colour.blue())
    }

    pub fn avatar_colours(&self) -> Vec<(u8, u8, u8)> {
        use slint::Model;
        self.window
            .global::<Theme>()
            .get_avatars()
            .iter()
            .map(|colour| (colour.red(), colour.green(), colour.blue()))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Window,
    Surface,
    Raised,
    RaisedHover,
    Text,
    TextDim,
    TextFaint,
    Good,
    Bad,
    Warn,
    Gold,
    OnAccent,
    GoodFill,
    BadFill,
    Toast,
    Menu,
    MenuHover,
}

pub struct Harness {
    front: Rc<RefCell<Front>>,
    hands: Hands,
    handed: Rc<Handed>,
}

impl std::ops::Deref for Harness {
    type Target = Hands;

    fn deref(&self) -> &Hands {
        &self.hands
    }
}

impl std::ops::DerefMut for Harness {
    fn deref_mut(&mut self) -> &mut Hands {
        &mut self.hands
    }
}

impl Harness {
    pub fn new(options: Options) -> anyhow::Result<Self> {
        let _ =
            slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
                mock_time: true,
                threading: false,
                renderer_name: Some("skia".into()),
            })));
        let handed = Rc::new(Handed::default());
        let front = Front::new(options, super::SystemTheme::default(), handed.clone())?;
        let window = front.borrow().window.clone_strong();
        window
            .window()
            .set_size(slint::LogicalSize::new(1180.0, 760.0));
        window.show()?;
        Ok(Self {
            front,
            hands: Hands {
                window,
                pictures: Rc::default(),
            },
            handed,
        })
    }

    pub fn opened(&self) -> Vec<String> {
        self.handed.opened.borrow().clone()
    }

    pub fn copied(&self) -> Vec<String> {
        self.handed.copied.borrow().clone()
    }

    pub fn tick(&mut self) {
        self.front.borrow_mut().tick();
        self.hands.let_animations_end();
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.hands
            .window
            .window()
            .set_size(slint::LogicalSize::new(width, height));
    }

    pub fn hide(&mut self) {
        let _ = self.hands.window.hide();
    }

    pub fn close(&mut self) {
        self.hands
            .window
            .window()
            .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    }

    pub fn is_shown(&self) -> bool {
        self.hands.window.window().is_visible()
    }

    pub fn wants_to_quit(&self) -> bool {
        self.front.borrow().quitting
    }

    pub fn next_tick_in(&self) -> Option<Duration> {
        let now = std::time::Instant::now();
        self.front
            .borrow()
            .core
            .next_tick(now)
            .map(|at| at.saturating_duration_since(now))
    }

    pub fn brightest_in(&self, label: &str) -> anyhow::Result<u8> {
        let element = self
            .hands
            .find(label)
            .ok_or_else(|| anyhow::anyhow!("nothing is called \"{label}\""))?;
        let brightest = self
            .pixels_of(&element)?
            .iter()
            .map(|pixel| pixel.r.min(pixel.g).min(pixel.b))
            .max();
        Ok(brightest.unwrap_or(0))
    }

    pub fn most_colourful_in(&self, label: &str) -> anyhow::Result<(u8, u8, u8)> {
        let element = self
            .hands
            .find(label)
            .ok_or_else(|| anyhow::anyhow!("nothing is called \"{label}\""))?;
        let spread = |pixel: &slint::Rgba8Pixel| {
            pixel.r.max(pixel.g).max(pixel.b) - pixel.r.min(pixel.g).min(pixel.b)
        };
        let pixels = self.pixels_of(&element)?;
        let pixel = pixels
            .iter()
            .max_by_key(|pixel| spread(pixel))
            .ok_or_else(|| anyhow::anyhow!("\"{label}\" is nowhere in the window"))?;
        Ok((pixel.r, pixel.g, pixel.b))
    }

    fn pixels_of(&self, element: &ElementHandle) -> anyhow::Result<Vec<slint::Rgba8Pixel>> {
        let pixels = self.hands.window.window().take_snapshot()?;
        let scale = self.hands.window.window().scale_factor();
        let (at, size) = (element.absolute_position(), element.size());
        let across = |from: f32, length: f32, limit: u32| {
            let first = ((from * scale).max(0.0) as u32).min(limit);
            let last = (((from + length) * scale).max(0.0) as u32).min(limit);
            first..last
        };
        let width = pixels.width();
        let mut found = Vec::new();
        for y in across(at.y, size.height, pixels.height()) {
            for x in across(at.x, size.width, width) {
                found.push(pixels.as_slice()[(y * width + x) as usize]);
            }
        }
        Ok(found)
    }

    pub fn picture(&self, to: &Path) -> anyhow::Result<()> {
        self.hands.let_animations_end();
        let pixels = self.hands.window.window().take_snapshot()?;
        let file = std::io::BufWriter::new(std::fs::File::create(to)?);
        let mut encoder = png::Encoder::new(file, pixels.width(), pixels.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()?
            .write_image_data(pixels.as_bytes())?;
        Ok(())
    }
}
