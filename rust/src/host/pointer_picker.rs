//! Capture one click with a pin cursor, without activating a window.

use std::cell::Cell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, GetStockObject, BLACK_BRUSH, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{w, PCWSTR, Result};

use super::{emit, markers, register_hotkey, unregister_hotkey, Event};
use crate::win;

pub(super) const CANCEL_HOTKEY: i32 = 5;

#[derive(Clone, Copy)]
struct Picker {
    hwnd: HWND,
    pin_cursor: HCURSOR,
    unpin_cursor: HCURSOR,
}

thread_local! {
    static PICKER: Cell<Option<Picker>> = const { Cell::new(None) };
}

fn cursor_at(point: POINT) {
    let Some(picker) = PICKER.with(Cell::get) else { return };
    unsafe {
        // Query the native hit test through our capture layer, then restore it.
        let style = GetWindowLongW(picker.hwnd, GWL_EXSTYLE);
        SetWindowLongW(picker.hwnd, GWL_EXSTYLE, style | WS_EX_TRANSPARENT.0 as i32);
        let pinned = win::window_at(point).is_some_and(markers::is_pinned);
        SetWindowLongW(picker.hwnd, GWL_EXSTYLE, style);
        SetCursor(Some(if pinned { picker.unpin_cursor } else { picker.pin_cursor }));
    }
}

fn client_point(hwnd: HWND, lp: LPARAM) -> Option<POINT> {
    let mut point = POINT { x: lp.0 as i16 as i32, y: (lp.0 >> 16) as i16 as i32 };
    unsafe { ClientToScreen(hwnd, &mut point).as_bool().then_some(point) }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_SETCURSOR => {
                let mut point = POINT::default();
                if GetCursorPos(&mut point).is_ok() {
                    cursor_at(point);
                }
                LRESULT(1)
            }
            WM_MOUSEMOVE => {
                if let Some(point) = client_point(hwnd, lp) {
                    cursor_at(point);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let point = client_point(hwnd, lp);
                // Resolve the underlying window only once the picker is hidden.
                if cancel() && let Some(target) = point.and_then(win::window_at) {
                    emit(Event::WindowPicked(target), false);
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                cancel();
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn create() -> Result<Picker> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let pin_cursor = LoadCursorW(Some(instance.into()), PCWSTR(2 as _))?;
        let unpin_cursor = LoadCursorW(Some(instance.into()), PCWSTR(3 as _))?;
        let class = w!("TackHerePointerPicker");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: pin_cursor,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            class,
            w!(""),
            WS_POPUP,
            0, 0, 0, 0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        // A fully transparent layered window passes clicks through to the target.
        if let Err(error) = SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA) {
            let _ = DestroyWindow(hwnd);
            return Err(error);
        }
        Ok(Picker { hwnd, pin_cursor, unpin_cursor })
    }
}

pub(super) fn begin() -> Result<()> {
    let picker = match PICKER.with(Cell::get) {
        Some(picker) => picker,
        None => {
            let picker = create()?;
            PICKER.with(|state| state.set(Some(picker)));
            picker
        }
    };
    unsafe {
        if IsWindowVisible(picker.hwnd).as_bool() {
            return Ok(());
        }
        SetWindowPos(
            picker.hwnd,
            Some(HWND_TOPMOST),
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )?;
        let mut point = POINT::default();
        if GetCursorPos(&mut point).is_ok() {
            cursor_at(point);
        }
    }
    register_hotkey(CANCEL_HOTKEY, 0, VK_ESCAPE.0.into());
    Ok(())
}

pub(super) fn is_active() -> bool {
    PICKER.with(Cell::get).is_some_and(|picker| unsafe { IsWindowVisible(picker.hwnd).as_bool() })
}

pub(super) fn cancel() -> bool {
    let Some(picker) = PICKER.with(Cell::get) else { return false };
    unsafe {
        if !ShowWindow(picker.hwnd, SW_HIDE).as_bool() {
            return false;
        }
        SetCursor(LoadCursorW(None, IDC_ARROW).ok());
    }
    unregister_hotkey(CANCEL_HOTKEY);
    emit(Event::PickerEnded, false);
    true
}
