use windows::{
    core::{factory, BOOL, PWSTR},
    Graphics::Capture::GraphicsCaptureItem,
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM},
        Graphics::{
            Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
            Gdi::{
                CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC,
                DeleteObject, EnumDisplayMonitors, GdiFlush, GetDC, GetMonitorInfoW, ReleaseDC,
                SelectObject, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
                DIB_RGB_COLORS, HALFTONE, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFO, SRCCOPY,
            },
        },
        Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS},
        System::{
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
            WinRT::Graphics::Capture::IGraphicsCaptureItemInterop,
        },
        UI::{
            HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            WindowsAndMessaging::{
                DrawIconEx, EnumWindows, GetAncestor, GetClassLongPtrW, GetClassNameW,
                GetShellWindow, GetWindowLongW, GetWindowPlacement, GetWindowRect, GetWindowTextW,
                GetWindowThreadProcessId, IsIconic, IsWindowVisible, SendMessageTimeoutW,
                ShowWindowAsync, DI_NORMAL, GA_ROOT, GCLP_HICON, GWL_EXSTYLE, GWL_STYLE, HICON,
                ICON_BIG, ICON_SMALL2, MONITORINFOF_PRIMARY, PW_RENDERFULLCONTENT,
                SMTO_ABORTIFHUNG, SW_RESTORE, WINDOWPLACEMENT, WM_GETICON, WS_DISABLED,
                WS_EX_TOOLWINDOW,
            },
        },
    },
};

use crate::{Context, Error};

const PREVIEW_WIDTH: i32 = 384;
const PREVIEW_HEIGHT: i32 = 240;
const SMALLEST_WINDOW: i32 = 48;
const ICON_SIZE: i32 = 32;
const ICON_ANSWER_MS: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceId {
    Screen { handle: isize },
    Window { handle: isize, process: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListedSource {
    Screen {
        id: SourceId,
        size: (u32, u32),
    },
    Window {
        id: SourceId,
        title: String,
        program: Option<String>,
        minimized: bool,
    },
}

impl ListedSource {
    pub fn id(&self) -> SourceId {
        match self {
            Self::Screen { id, .. } | Self::Window { id, .. } => *id,
        }
    }
}

pub struct Preview {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Source {
    pub(crate) item: GraphicsCaptureItem,
    process: Option<u32>,
}

impl Source {
    pub fn open(id: SourceId) -> Result<Self, Error> {
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|_| Error::CaptureUnsupported)?;
        match id {
            SourceId::Screen { handle } => {
                let monitor = list_monitors()
                    .into_iter()
                    .find(|monitor| monitor.handle.0 as isize == handle)
                    .ok_or(Error::SourceGone)?;

                let item = unsafe { interop.CreateForMonitor(monitor.handle) }
                    .context("opening the screen for capture")?;
                Ok(Self {
                    item,
                    process: None,
                })
            }
            SourceId::Window { handle, process } => {
                let window = HWND(handle as *mut _);
                if process_of(window) != Some(process) {
                    return Err(Error::SourceGone);
                }
                if is_minimized(window) {
                    let _ = unsafe { ShowWindowAsync(window, SW_RESTORE) };
                }

                let item = unsafe { interop.CreateForWindow(window) }
                    .context("opening the window for capture")?;
                Ok(Self {
                    item,
                    process: Some(process),
                })
            }
        }
    }

    pub fn process_id(&self) -> Option<u32> {
        self.process
    }

    pub fn size(&self) -> Result<(u32, u32), Error> {
        let size = self
            .item
            .Size()
            .context("asking for the size of what is shared")?;
        Ok((size.Width.max(1) as u32, size.Height.max(1) as u32))
    }
}

struct Monitor {
    handle: HMONITOR,
    primary: bool,
    area: RECT,
}

impl Monitor {
    fn size(&self) -> (u32, u32) {
        (
            (self.area.right - self.area.left).max(0) as u32,
            (self.area.bottom - self.area.top).max(0) as u32,
        )
    }
}

fn list_monitors() -> Vec<Monitor> {
    unsafe extern "system" fn visit(handle: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        let list = unsafe { &mut *(list.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };

        if unsafe { GetMonitorInfoW(handle, &mut info) }.as_bool() {
            list.push(Monitor {
                handle,
                primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
                area: info.rcMonitor,
            });
        }
        true.into()
    }

    let mut list: Vec<Monitor> = Vec::new();
    let into = LPARAM(&mut list as *mut Vec<Monitor> as isize);

    let _ = unsafe { EnumDisplayMonitors(None, None, Some(visit), into) };
    list.sort_by_key(|m| !m.primary);
    list
}

pub(crate) fn dpi_aware() {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

pub fn list_sources() -> Vec<ListedSource> {
    dpi_aware();
    let screens = list_monitors()
        .into_iter()
        .map(|monitor| ListedSource::Screen {
            id: SourceId::Screen {
                handle: monitor.handle.0 as isize,
            },
            size: monitor.size(),
        });
    let windows = windows_worth_showing().into_iter().filter_map(|window| {
        Some(ListedSource::Window {
            id: SourceId::Window {
                handle: window.0 as isize,
                process: process_of(window)?,
            },
            title: title_of(window),
            program: program_of(window),
            minimized: is_minimized(window),
        })
    });
    screens.chain(windows).collect()
}

pub fn preview_of(id: SourceId) -> Option<Preview> {
    dpi_aware();
    match id {
        SourceId::Screen { handle } => list_monitors()
            .into_iter()
            .find(|monitor| monitor.handle.0 as isize == handle)
            .and_then(|monitor| preview_of_screen(monitor.area)),
        SourceId::Window { handle, process } => {
            let window = HWND(handle as *mut _);
            (process_of(window) == Some(process))
                .then(|| preview_of_window(window))
                .flatten()
        }
    }
}

pub fn icon_of(id: SourceId) -> Option<Preview> {
    let SourceId::Window { handle, process } = id else {
        return None;
    };
    let window = HWND(handle as *mut _);
    if process_of(window) != Some(process) {
        return None;
    }
    draw_icon(icon_of_window(window)?)
}

fn icon_of_window(window: HWND) -> Option<HICON> {
    for kind in [ICON_BIG, ICON_SMALL2] {
        let mut icon = 0usize;

        let answered = unsafe {
            SendMessageTimeoutW(
                window,
                WM_GETICON,
                WPARAM(kind as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                ICON_ANSWER_MS,
                Some(&mut icon),
            )
        };
        if answered.0 != 0 && icon != 0 {
            return Some(HICON(icon as *mut _));
        }
    }

    let icon = unsafe { GetClassLongPtrW(window, GCLP_HICON) };
    (icon != 0).then_some(HICON(icon as *mut _))
}

struct Canvas(HDC);

impl Canvas {
    fn compatible_with(dc: HDC) -> Option<Self> {
        let canvas = unsafe { CreateCompatibleDC(Some(dc)) };
        (!canvas.is_invalid()).then_some(Self(canvas))
    }

    fn select(&self, object: &Owned) -> Selected<'_> {
        let before = unsafe { SelectObject(self.0, object.0) };
        Selected {
            canvas: self,
            before,
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        let _ = unsafe { DeleteDC(self.0) };
    }
}

struct Selected<'a> {
    canvas: &'a Canvas,
    before: HGDIOBJ,
}

impl Drop for Selected<'_> {
    fn drop(&mut self) {
        unsafe { SelectObject(self.canvas.0, self.before) };
    }
}

struct Owned(HGDIOBJ);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = unsafe { DeleteObject(self.0) };
    }
}

struct ScreenDc(HDC);

impl ScreenDc {
    fn get() -> Option<Self> {
        let dc = unsafe { GetDC(None) };
        (!dc.is_invalid()).then_some(Self(dc))
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe { ReleaseDC(None, self.0) };
    }
}

struct Pixels {
    bitmap: Owned,
    bits: *mut u8,
    len: usize,
}

impl Pixels {
    fn new(dc: HDC, width: i32, height: i32) -> Option<Self> {
        let layout = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();

        let bitmap: HBITMAP =
            unsafe { CreateDIBSection(Some(dc), &layout, DIB_RGB_COLORS, &mut bits, None, 0) }
                .ok()?;
        let bitmap = Owned(bitmap.into());
        (!bits.is_null()).then(|| Self {
            bitmap,
            bits: bits.cast(),
            len: width as usize * height as usize * 4,
        })
    }

    fn bgra(&self) -> &[u8] {
        let _ = unsafe { GdiFlush() };

        unsafe { std::slice::from_raw_parts(self.bits, self.len) }
    }

    fn fill(&mut self, value: u8) {
        let _ = unsafe { GdiFlush() };

        unsafe { std::slice::from_raw_parts_mut(self.bits, self.len) }.fill(value);
    }
}

fn draw_icon(icon: HICON) -> Option<Preview> {
    let screen = ScreenDc::get()?;
    let canvas = Canvas::compatible_with(screen.0)?;
    let mut pixels = Pixels::new(screen.0, ICON_SIZE, ICON_SIZE)?;
    let mut drawn = Vec::with_capacity(2);
    for background in [0u8, 255] {
        pixels.fill(background);
        let selected = canvas.select(&pixels.bitmap);

        let painted = unsafe {
            DrawIconEx(
                canvas.0, 0, 0, icon, ICON_SIZE, ICON_SIZE, 0, None, DI_NORMAL,
            )
        };
        drop(selected);
        if painted.is_err() {
            return None;
        }
        drawn.push(pixels.bgra().to_vec());
    }
    let [on_black, on_white] = &drawn[..] else {
        return None;
    };
    let rgba = unmix(on_black, on_white);
    rgba.chunks_exact(4)
        .any(|pixel| pixel[3] > 0)
        .then_some(Preview {
            width: ICON_SIZE as u32,
            height: ICON_SIZE as u32,
            rgba,
        })
}

fn unmix(on_black: &[u8], on_white: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(on_black.len());
    for (black, white) in on_black.chunks_exact(4).zip(on_white.chunks_exact(4)) {
        let shows_through = (0..3)
            .map(|c| i32::from(white[c]) - i32::from(black[c]))
            .max()
            .unwrap_or(0)
            .clamp(0, 255);
        let alpha = 255 - shows_through;
        let colour = |c: u8| match alpha {
            0 => 0,
            alpha => (i32::from(c) * 255 / alpha).min(255) as u8,
        };
        rgba.extend_from_slice(&[
            colour(black[2]),
            colour(black[1]),
            colour(black[0]),
            alpha as u8,
        ]);
    }
    rgba
}

fn process_of(window: HWND) -> Option<u32> {
    let mut process = 0u32;

    unsafe { GetWindowThreadProcessId(window, Some(&mut process)) };
    (process != 0).then_some(process)
}

fn title_of(window: HWND) -> String {
    let mut text = [0u16; 512];

    let len = unsafe { GetWindowTextW(window, &mut text) }.max(0) as usize;
    String::from_utf16_lossy(&text[..len.min(text.len())])
}

struct Process(windows::Win32::Foundation::HANDLE);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn program_of(window: HWND) -> Option<String> {
    let process = process_of(window)?;

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process) }.ok()?;
    let handle = Process(handle);
    let mut path = [0u16; 1024];
    let mut len = path.len() as u32;

    unsafe {
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;
    let path = String::from_utf16_lossy(&path[..(len as usize).min(path.len())]);
    std::path::Path::new(&path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

fn windows_worth_showing() -> Vec<HWND> {
    unsafe extern "system" fn visit(window: HWND, list: LPARAM) -> BOOL {
        let list = unsafe { &mut *(list.0 as *mut Vec<HWND>) };
        if worth_showing(window) {
            list.push(window);
        }
        true.into()
    }

    let mut list: Vec<HWND> = Vec::new();
    let into = LPARAM(&mut list as *mut Vec<HWND> as isize);

    let _ = unsafe { EnumWindows(Some(visit), into) };
    list
}

fn dwm_u32(
    window: HWND,
    attribute: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE,
) -> Option<u32> {
    let mut value = 0u32;

    unsafe {
        DwmGetWindowAttribute(
            window,
            attribute,
            (&mut value as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    }
    .ok()
    .map(|()| value)
}

fn class_of(window: HWND) -> String {
    let mut class = [0u16; 64];

    let len = unsafe { GetClassNameW(window, &mut class) }.max(0) as usize;
    String::from_utf16_lossy(&class[..len.min(class.len())])
}

pub(crate) fn is_minimized(window: HWND) -> bool {
    unsafe { IsIconic(window) }.as_bool()
}

fn worth_showing(window: HWND) -> bool {
    let visible = unsafe { IsWindowVisible(window) }.as_bool();

    let shell = unsafe { GetShellWindow() };

    let root = unsafe { GetAncestor(window, GA_ROOT) };
    if !visible || window == shell || root != window {
        return false;
    }

    let style = unsafe { GetWindowLongW(window, GWL_STYLE) } as u32;

    let extended = unsafe { GetWindowLongW(window, GWL_EXSTYLE) } as u32;
    if style & WS_DISABLED.0 != 0 || extended & WS_EX_TOOLWINDOW.0 != 0 {
        return false;
    }
    if dwm_u32(window, DWMWA_CLOAKED).is_some_and(|cloaked| cloaked != 0)
        || class_of(window) == "Windows.UI.Core.CoreWindow"
    {
        return false;
    }
    if process_of(window) == Some(std::process::id()) || title_of(window).trim().is_empty() {
        return false;
    }
    let area = match is_minimized(window) {
        true => restored_area(window),
        false => bounds_of(window).map(|(_, visible)| visible),
    };
    area.is_some_and(|area| {
        area.right - area.left >= SMALLEST_WINDOW && area.bottom - area.top >= SMALLEST_WINDOW
    })
}

fn restored_area(window: HWND) -> Option<RECT> {
    let mut placement = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };

    unsafe { GetWindowPlacement(window, &mut placement) }.ok()?;
    Some(placement.rcNormalPosition)
}

pub(crate) fn size_of_window(window: HWND) -> Option<(u32, u32)> {
    let (_, visible) = bounds_of(window)?;
    let (width, height) = (visible.right - visible.left, visible.bottom - visible.top);
    (width > 0 && height > 0).then_some((width as u32, height as u32))
}

fn bounds_of(window: HWND) -> Option<(RECT, RECT)> {
    let mut whole = RECT::default();
    let mut visible = RECT::default();

    unsafe { GetWindowRect(window, &mut whole) }.ok()?;

    let asked = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut visible as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
    };
    Some((whole, if asked.is_ok() { visible } else { whole }))
}

fn preview_of_screen(area: RECT) -> Option<Preview> {
    let screen = ScreenDc::get()?;
    shrink(screen.0, area)
}

fn preview_of_window(window: HWND) -> Option<Preview> {
    if is_minimized(window) {
        return None;
    }
    let (whole, visible) = bounds_of(window)?;
    let (width, height) = (whole.right - whole.left, whole.bottom - whole.top);
    if width <= 0 || height <= 0 {
        return None;
    }
    let screen = ScreenDc::get()?;
    let canvas = Canvas::compatible_with(screen.0)?;

    let bitmap = unsafe { CreateCompatibleBitmap(screen.0, width, height) };
    if bitmap.is_invalid() {
        return None;
    }
    let bitmap = Owned(bitmap.into());
    let _selected = canvas.select(&bitmap);

    let drawn = unsafe { PrintWindow(window, canvas.0, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)) };
    drawn.as_bool().then_some(())?;
    shrink(
        canvas.0,
        RECT {
            left: visible.left - whole.left,
            top: visible.top - whole.top,
            right: visible.right - whole.left,
            bottom: visible.bottom - whole.top,
        },
    )
}

fn shrink(from: HDC, area: RECT) -> Option<Preview> {
    let (from_width, from_height) = (area.right - area.left, area.bottom - area.top);
    if from_width <= 0 || from_height <= 0 {
        return None;
    }
    let scale = f64::min(
        f64::from(PREVIEW_WIDTH) / f64::from(from_width),
        f64::from(PREVIEW_HEIGHT) / f64::from(from_height),
    )
    .min(1.0);
    let width = ((f64::from(from_width) * scale).round() as i32).max(1);
    let height = ((f64::from(from_height) * scale).round() as i32).max(1);
    let canvas = Canvas::compatible_with(from)?;
    let pixels = Pixels::new(from, width, height)?;
    let selected = canvas.select(&pixels.bitmap);

    unsafe { SetStretchBltMode(canvas.0, HALFTONE) };

    let drawn = unsafe {
        StretchBlt(
            canvas.0,
            0,
            0,
            width,
            height,
            Some(from),
            area.left,
            area.top,
            from_width,
            from_height,
            SRCCOPY,
        )
    };
    drop(selected);
    drawn.as_bool().then_some(())?;
    let rgba = pixels
        .bgra()
        .chunks_exact(4)
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], 255])
        .collect();
    Some(Preview {
        width: width as u32,
        height: height as u32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

    #[test]
    fn an_icon_drawn_on_black_and_on_white_gives_its_colour_and_how_solid_it_is() {
        let on_black = [0, 0, 255, 0, 0, 0, 0, 0, 128, 0, 0, 0];
        let on_white = [0, 0, 255, 0, 255, 255, 255, 0, 255, 127, 127, 0];
        assert_eq!(
            unmix(&on_black, &on_white),
            [255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 255, 128]
        );
    }

    #[test]
    fn a_window_handle_that_now_belongs_to_another_program_is_not_opened() {
        let window = unsafe { GetDesktopWindow() };
        let owner = process_of(window).unwrap_or_default();
        let taken_over = SourceId::Window {
            handle: window.0 as isize,
            process: owner.wrapping_add(1),
        };
        assert!(matches!(Source::open(taken_over), Err(Error::SourceGone)));
        assert!(preview_of(taken_over).is_none());
        assert!(icon_of(taken_over).is_none());
    }

    #[test]
    fn a_screen_that_is_gone_is_not_opened() {
        let unplugged = SourceId::Screen { handle: -1 };
        assert!(matches!(Source::open(unplugged), Err(Error::SourceGone)));
        assert!(preview_of(unplugged).is_none());
    }
}
