//! Native buttons attached to the visible top edge of pinned windows.

use std::cell::{Cell, RefCell};
use std::mem::size_of;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetMonitorInfoW,
    MonitorFromPoint, SelectObject, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use resvg::{tiny_skia, usvg};

use crate::win::{self, Hwnd};
use super::{Event, emit};

const CLASS: PCWSTR = w!("TackHerePinMarker");
const SEAT_MS: u64 = 125;
const PULL_MS: u64 = 150;
const SEAT_DISTANCE_DIP: f32 = 18.0;
const PULL_DISTANCE_DIP: f32 = 25.0;
const FADE_DISTANCE_DIP: f32 = 18.0;
// The marker SVG's shank points down-left at 43 degrees.
const SHAFT_X: f32 = 0.7314;
const SHAFT_Y: f32 = 0.6820;
const ICON_SIZE_DIP: i32 = 24;
const HIT_MARGIN_DIP: i32 = 3;
const TITLE_EDGE_OVERLAP_DIP: i32 = 8;

thread_local! {
    static ART: usvg::Tree = usvg::Tree::from_data(
        include_bytes!("../../../resources/pin-marker.svg"),
        &usvg::Options::default(),
    ).expect("valid embedded marker SVG");
    static MARKERS: RefCell<Vec<Marker>> = const { RefCell::new(Vec::new()) };
    static TIMER: slint::Timer = slint::Timer::default();
    static INTERVAL: Cell<u64> = const { Cell::new(0) };
}

#[derive(Clone, Copy)]
enum Motion {
    Down { start: Instant, from: f32 },
    Rest,
    Up { start: Instant, from: f32 },
}

impl Motion {
    fn leaving(self) -> bool {
        matches!(self, Self::Up { .. })
    }

    fn elapsed(now: Instant, start: Instant, duration: u64) -> f32 {
        (now.duration_since(start).as_millis() as f32 / duration as f32).min(1.0)
    }

    fn frame(&mut self, now: Instant) -> Option<(f32, u8)> {
        match *self {
            Self::Rest => Some((0.0, 255)),
            Self::Down { start, from } => {
                let t = Self::elapsed(now, start, SEAT_MS);
                if t >= 1.0 {
                    *self = Self::Rest;
                }
                let progress = t * (0.35 + 0.65 * t);
                Some((from * (1.0 - progress), 255))
            }
            Self::Up { start, from } => {
                let t = Self::elapsed(now, start, PULL_MS);
                if t >= 1.0 {
                    return None;
                }
                let progress = t * (0.25 + 0.75 * t);
                let travel = from + (PULL_DISTANCE_DIP - from) * progress;
                let opacity = (PULL_DISTANCE_DIP - travel) / (PULL_DISTANCE_DIP - FADE_DISTANCE_DIP);
                Some((travel, (opacity.min(1.0) * 255.0).round() as u8))
            }
        }
    }
}

struct Surface {
    side: i32,
    padding: i32,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl Surface {
    fn new(side: i32, padding: i32) -> Option<Self> {
        let mut image = tiny_skia::Pixmap::new(side as u32, side as u32)?;
        ART.with(|svg| {
            let scale = (side - 2 * padding) as f32 / svg.size().width();
            let transform = tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, padding as f32, padding as f32);
            resvg::render(svg, transform, &mut image.as_mut());
        });
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: side,
                    biHeight: -side,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0);
            let Ok(bitmap) = bitmap else {
                let _ = DeleteDC(dc);
                return None;
            };
            // resvg renders premultiplied RGBA; the Win32 surface uses premultiplied BGRA.
            let pixels = std::slice::from_raw_parts_mut(bits.cast::<u8>(), image.data().len());
            pixels.copy_from_slice(image.data());
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            fill_hit_margin(pixels, image.data(), side, padding);
            let previous = SelectObject(dc, bitmap.into());
            Some(Self { side, padding, dc, bitmap, previous })
        }
    }
}

// Layered windows ignore fully transparent pixels when hit-testing.
// Give the shape a narrow, one-alpha click margin and keep the rest transparent.
fn fill_hit_margin(pixels: &mut [u8], artwork: &[u8], side: i32, padding: i32) {
    let index = |x, y| ((y * side + x) * 4 + 3) as usize;
    for y in 0..side {
        for x in 0..side {
            let pixel = index(x, y);
            if artwork[pixel] < 32 {
                continue;
            }
            for dy in -padding..=padding {
                for dx in -padding..=padding {
                    let (nx, ny) = (x + dx, y + dy);
                    if dx * dx + dy * dy <= padding * padding
                        && (0..side).contains(&nx) && (0..side).contains(&ny)
                    {
                        let neighbor = index(nx, ny);
                        pixels[neighbor] = pixels[neighbor].max(1);
                    }
                }
            }
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

struct Marker {
    hwnd: HWND,
    target: Hwnd,
    surface: Option<Surface>,
    motion: Motion,
    last_frame: Option<(i32, i32, i32, i32, u8)>,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONUP => {
                let target = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if target != 0 {
                    emit(Event::MarkerClicked(target), false);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

impl Marker {
    fn new(target: Hwnd, now: Instant, animate: bool) -> Option<Self> {
        static CLASS_INIT: std::sync::Once = std::sync::Once::new();
        unsafe {
            let instance = GetModuleHandleW(None).ok()?;
            CLASS_INIT.call_once(|| {
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance.into(),
                    hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
                    lpszClassName: CLASS,
                    ..Default::default()
                });
            });
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                CLASS, w!("Unpin window"), WS_POPUP, 0, 0, 0, 0,
                // An owned popup stays above the target even as it becomes active.
                Some(HWND(target as *mut _)), None, Some(instance.into()), None,
            ).ok()?;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, target);
            Some(Self {
                hwnd, target, surface: None, last_frame: None,
                motion: if animate { Motion::Down { start: now, from: SEAT_DISTANCE_DIP } } else { Motion::Rest },
            })
        }
    }

    fn set_interactive(&self, on: bool, below: Option<Hwnd>) {
        unsafe {
            let style = GetWindowLongW(self.hwnd, GWL_EXSTYLE) as u32;
            let transparent = WS_EX_TRANSPARENT.0;
            let wanted = if on { style & !transparent } else { style | transparent };
            SetWindowLongW(self.hwnd, GWL_EXSTYLE, wanted as i32);
            let target = if on { self.target } else { 0 };
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, target);
            SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, target);
            if !on {
                // The marker finishes lifting above the now-unpinned owner.
                let _ = win::apply_topmost(self.hwnd.0 as Hwnd, below);
            }
        }
    }

    fn update(&mut self, now: Instant) -> bool {
        let Some((travel, alpha)) = self.motion.frame(now) else { return false };
        let target = HWND(self.target as *mut _);
        if !win::is_valid_window(self.target) {
            return false;
        }
        if unsafe { IsIconic(target).as_bool() || !IsWindowVisible(target).as_bool() }
            || win::is_cloaked(target)
        {
            self.hide();
            return !self.motion.leaving();
        }
        let Some(rect) = win::frame_rect(self.target) else {
            self.hide();
            return !self.motion.leaving();
        };
        let dpi = unsafe { GetDpiForWindow(target) }.max(96) as i32;
        let padding = (HIT_MARGIN_DIP * dpi + 48) / 96;
        let side = (ICON_SIZE_DIP * dpi + 48) / 96 + 2 * padding;
        let rest_x = (rect.left + rect.right - side) / 2;
        let overlap = (TITLE_EDGE_OVERLAP_DIP * dpi + 48) / 96;
        let anchor = POINT { x: rest_x + side / 2, y: rect.top };
        let mut monitor = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut rest_y = rect.top - overlap;
        // The cap protrudes above the window edge only where the screen has room for it.
        unsafe {
            let screen = MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST);
            if GetMonitorInfoW(screen, &mut monitor).as_bool() {
                rest_y = rest_y.max(monitor.rcMonitor.top);
            }
        }
        let distance = travel * dpi as f32 / 96.0;
        let x = rest_x + (distance * SHAFT_X).round() as i32;
        let y = rest_y - padding - (distance * SHAFT_Y).round() as i32;
        let frame = (x, y, side, padding, alpha);
        if self.last_frame != Some(frame) {
            if self.surface.as_ref().is_none_or(|s| s.side != side || s.padding != padding) {
                self.surface = Surface::new(side, padding);
            }
            let Some(surface) = self.surface.as_ref() else {
                self.hide();
                return true;
            };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            unsafe {
                if UpdateLayeredWindow(
                    self.hwnd, None, Some(&POINT { x, y }),
                    Some(&SIZE { cx: side, cy: side }), Some(surface.dc),
                    Some(&POINT::default()), COLORREF(0), Some(&blend), ULW_ALPHA,
                ).is_err() {
                    return true;
                }
            }
            self.last_frame = Some(frame);
        }

        unsafe {
            if !IsWindowVisible(self.hwnd).as_bool() {
                // Show the marker immediately above its owner, in the owner's current group.
                let above = GetWindow(target, GW_HWNDPREV).ok();
                let mut flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW;
                if above == Some(self.hwnd) {
                    flags |= SWP_NOZORDER;
                }
                let _ = SetWindowPos(self.hwnd, Some(above.unwrap_or(HWND_TOPMOST)), 0, 0, 0, 0, flags);
            }
        }
        true
    }

    fn hide(&mut self) {
        unsafe { let _ = ShowWindow(self.hwnd, SW_HIDE); }
        self.last_frame = None;
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        unsafe { let _ = DestroyWindow(self.hwnd); }
    }
}

/// Resolve a marker hit to the window it belongs to.
pub fn target_of(hwnd: Hwnd) -> Option<Hwnd> {
    if win::class_name(hwnd) != "TackHerePinMarker" {
        return None;
    }
    let target = unsafe { GetWindowLongPtrW(HWND(hwnd as *mut _), GWLP_USERDATA) };
    (target != 0).then_some(target)
}

pub fn is_pinned(target: Hwnd) -> bool {
    MARKERS.with(|m| m.borrow().iter().any(|m| m.target == target && !m.motion.leaving()))
}

pub fn raise_with_owner(target: Hwnd, below: Option<Hwnd>) {
    MARKERS.with(|markers| {
        if let Some(marker) = markers.borrow().iter().find(|m| m.target == target && !m.motion.leaving()) {
            // Our explicit SetWindowPos on the owner uses SWP_NOOWNERZORDER.
            let _ = win::apply_topmost(marker.hwnd.0 as Hwnd, below);
        }
    });
}

pub fn is_marker(hwnd: Hwnd) -> bool {
    MARKERS.with(|markers| markers.borrow().iter().any(|marker| marker.hwnd.0 as Hwnd == hwnd))
}

pub fn sync(targets: &[Hwnd], below: Option<Hwnd>) {
    let now = Instant::now();
    let animate = win::animations_enabled();
    MARKERS.with(|markers| {
        let mut markers = markers.borrow_mut();
        if !animate {
            markers.retain(|m| targets.contains(&m.target));
        }
        for marker in markers.iter_mut() {
            let wanted = targets.contains(&marker.target);
            if wanted && marker.motion.leaving() {
                let from = marker.motion.frame(now).map_or(SEAT_DISTANCE_DIP, |(travel, _)| travel);
                marker.motion = if animate { Motion::Down { start: now, from } } else { Motion::Rest };
                marker.set_interactive(true, below);
            } else if !wanted && !marker.motion.leaving() {
                let from = marker.motion.frame(now).map_or(0.0, |(travel, _)| travel);
                marker.motion = Motion::Up { start: now, from };
                marker.set_interactive(false, below);
            }
        }
        for &target in targets {
            if !markers.iter().any(|m| m.target == target) {
                markers.extend(Marker::new(target, now, animate));
            }
        }
    });
    refresh();
}

fn refresh() {
    let now = Instant::now();
    let interval = MARKERS.with(|markers| {
        let mut markers = markers.borrow_mut();
        markers.retain_mut(|m| m.update(now));
        if markers.is_empty() {
            0
        } else if markers.iter().any(|m| !matches!(m.motion, Motion::Rest)) {
            16
        } else {
            80
        }
    });
    if INTERVAL.with(|old| old.replace(interval)) != interval {
        TIMER.with(|timer| {
            if interval == 0 {
                timer.stop();
            } else {
                timer.start(slint::TimerMode::Repeated, Duration::from_millis(interval), refresh);
            }
        });
    }
}

pub fn clear() {
    TIMER.with(|t| t.stop());
    INTERVAL.with(|i| i.set(0));
    MARKERS.with(|m| m.borrow_mut().clear());
}
