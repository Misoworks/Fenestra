// Drag region handling for the WebView2 backend.
//
// On Windows the WebView2 controller exposes
// `ICoreWebView2Controller2::add_NonClientRegionChanged` and
// `ICoreWebView2CompositionController4::DragRegions` for frameless
// hit-testing. The page-side `window.fenestra.dragRegions` API
// collects the rectangles and the controller delivers them to the
// host via the `NonClientRegionChanged` event. The host then
// translates the regions into `SetWindowRgn` / `WM_NCHITTEST`
// hit-testing so the frameless window can be dragged by the
// title bar.
//
// The CEF equivalent is `CefBrowserViewDelegate::OnDraggableRegionsChanged`
// (in `crates/fenestra-cef/host/shared/handler.cc:428`).
//
// For now this module exposes a no-op `apply_drag_regions`. The
// regions are pushed by the page through the bridge, the host
// receives them through `add_NonClientRegionChanged`, and the
// native hit-testing is performed by winit's window proc. The
// Rust side does not need to push any regions itself.

#![cfg(target_os = "windows")]

use fenestra_platform::WindowRegionRect;

use crate::WebView2Result;

/// Apply a list of drag regions to a frameless HWND. On Windows this
/// is a thin wrapper around the WebView2 controller's
/// `NonClientRegionChanged` event plus the host-side handling of
/// `HTCLIENT` / `HTCAPTION` hit-testing in `WM_NCHITTEST`.
///
/// The page-side `window.fenestra.dragRegions` API drives the
/// regions; the host receives them via the controller event and
/// turns them into `SetWindowRgn` calls. This function returns
/// `Ok(())` because the regions are managed on the host by the
/// controller event, not by the Rust side.
pub(crate) fn apply_drag_regions(
    _hwnd: isize,
    _regions: &[WindowRegionRect],
) -> WebView2Result<()> {
    Ok(())
}
