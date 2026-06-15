// WebView2 (Evergreen) backend for Fenestra cross-platform webview
// windows.
//
// `FenestraWindow` is a type alias that points at the right backend on
// each host: `WebView2Window` (this crate) on Windows, the
// CEF-backed struct in `fenestra-cef` on Linux / macOS. Both backends
// share the bridge protocol, activity registry, and JS surface defined
// in `fenestra-bridge`.

#![cfg_attr(target_os = "windows", allow(dead_code))]

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(target_os = "windows"))]
mod stub;
#[cfg(not(target_os = "windows"))]
pub use stub::*;
