// Win32 host-control calls: show / hide / focus / minimize / maximize,
// plus DWM glass. These are the simple synchronous calls the launch
// loop uses to drive the window from `WebView2UserEvent`. The
// signatures use the `windows 0.60` series to match what
// `webview2-com 0.36` brings in.

#![cfg(target_os = "windows")]

use fenestra_platform::WindowBackgroundEffect;
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        BringWindowToTop, SW_HIDE, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW,
        SetForegroundWindow, ShowWindow,
    },
};

use crate::{WebView2Config, WebView2Result};

/// Show the window. Equivalent to `ShowWindow(hwnd, SW_SHOW)`.
pub(crate) fn show_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { ShowWindow(HWND(hwnd as *mut _), SW_SHOW) }.as_bool()
}

/// Hide the window. Equivalent to `ShowWindow(hwnd, SW_HIDE)`.
pub(crate) fn hide_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { ShowWindow(HWND(hwnd as *mut _), SW_HIDE) }.as_bool()
}

/// Bring the window to the foreground. Calls
/// `SetForegroundWindow(hwnd)` after a `BringWindowToTop(hwnd)` for
/// older Win32 compatibility.
pub(crate) fn focus_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    let hwnd = HWND(hwnd as *mut _);
    let _ = unsafe { BringWindowToTop(hwnd) };
    unsafe { SetForegroundWindow(hwnd) }.as_bool()
}

/// Minimize the window. Equivalent to `ShowWindow(hwnd, SW_MINIMIZE)`.
pub(crate) fn minimize_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { ShowWindow(HWND(hwnd as *mut _), SW_MINIMIZE) }.as_bool()
}

/// Maximize the window. Equivalent to `ShowWindow(hwnd, SW_MAXIMIZE)`.
pub(crate) fn maximize_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { ShowWindow(HWND(hwnd as *mut _), SW_MAXIMIZE) }.as_bool()
}

/// Restore a minimized or maximized window to its previous size.
/// Equivalent to `ShowWindow(hwnd, SW_RESTORE)`.
pub(crate) fn unmaximize_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { ShowWindow(HWND(hwnd as *mut _), SW_RESTORE) }.as_bool()
}

/// Apply a DWM `DWMWA_SYSTEMBACKDROP_TYPE` to the HWND if the user
/// asked for a glass-like background effect. The
/// `DWM_SYSTEMBACKDROP_TYPE` enum was added in Windows 11 build 22000
/// for the new Mica / Acrylic / Tabbed system backdrops; on older
/// Windows builds the call is a no-op.
pub(crate) fn apply_dwm_backdrop(hwnd: isize, config: &WebView2Config) -> WebView2Result<()> {
    use windows::Win32::Graphics::Dwm::{
        DWM_SYSTEMBACKDROP_TYPE, DWMWA_SYSTEMBACKDROP_TYPE, DwmSetWindowAttribute,
    };
    if hwnd == 0 {
        return Ok(());
    }
    let backdrop = match config.background_effect {
        WindowBackgroundEffect::Acrylic => DWM_SYSTEMBACKDROP_TYPE(3),
        WindowBackgroundEffect::Mica | WindowBackgroundEffect::MicaAlt => {
            DWM_SYSTEMBACKDROP_TYPE(2)
        }
        WindowBackgroundEffect::Blur => DWM_SYSTEMBACKDROP_TYPE(1),
        WindowBackgroundEffect::None
        | WindowBackgroundEffect::Glass
        | WindowBackgroundEffect::Vibrancy
        | WindowBackgroundEffect::HudWindow
        | WindowBackgroundEffect::Sidebar
        | WindowBackgroundEffect::UnderWindowBackground => return Ok(()),
    };
    let hr = unsafe {
        DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const _,
            std::mem::size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        )
    };
    if hr.is_err() {
        // Older Windows builds (pre-22H2) do not have the
        // DWMWA_SYSTEMBACKDROP_TYPE attribute. The failure is not
        // fatal — a window without a system backdrop is still
        // functional, it just lacks the glass effect.
        return Ok(());
    }
    Ok(())
}
