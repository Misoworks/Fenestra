// Non-Windows stub. The WebView2 (Evergreen) backend only runs on Windows.
// The real implementation lives behind `#[cfg(target_os = "windows")]` in
// `windows.rs`. The shape of the public surface is mirrored here so that
// cross-platform code that only references type names can compile on
// every host.
//
// `FenestraWindow` is a type alias to `WebView2Window` on Windows and to the
// CEF-backed struct on every other platform; that alias lives in
// `fenestra-cef` and is re-exported by it. This stub exists so the
// `fenestra-webview2` crate compiles on every host, so `cargo build
// --workspace` can validate the Linux + macOS build paths.

use std::path::PathBuf;

use fenestra_bridge::{BridgeHandlers, FenestraLaunchMetricsSnapshot};
use fenestra_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    SingleInstancePolicy, TrayIcon, WindowBackgroundEffect, WindowRegion, WindowRegionRect,
    WindowRegions,
};
use fenestra_runtime::RuntimeInfo;

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct WebView2Window {
    pub config: WebView2Config,
    bridge_handlers: BridgeHandlers,
}

impl WebView2Window {
    pub fn new() -> Self {
        Self {
            config: WebView2Config::default(),
            bridge_handlers: BridgeHandlers::default(),
        }
    }

    pub fn bridge_handler<F>(self, _name: impl Into<String>, _handler: F) -> Self
    where
        F: Fn(fenestra_bridge::BridgeCommand) -> fenestra_bridge::BridgeResult
            + Send
            + Sync
            + 'static,
    {
        self
    }

    pub fn launch(self) -> Result<WebView2Process, WebView2Error> {
        Err(WebView2Error::UnsupportedOnThisPlatform)
    }

    pub fn launch_or_install(self) -> Result<WebView2Process, WebView2Error> {
        Err(WebView2Error::UnsupportedOnThisPlatform)
    }
}

impl Default for WebView2Window {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Default)]
pub struct WebView2Config {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub url: Option<String>,
    pub entry: Option<String>,
}

pub struct WebView2Process {
    pub(crate) _phantom: std::marker::PhantomData<RuntimeInfo>,
}

impl WebView2Process {
    pub fn id(&self) -> u32 {
        0
    }
    pub fn show(&self) -> bool {
        false
    }
    pub fn hide(&self) -> bool {
        false
    }
    pub fn focus_window(&self) -> bool {
        false
    }
    pub fn set_visible(&self, _visible: bool) -> bool {
        false
    }
    pub fn begin_activity(
        &self,
        _name: impl Into<String>,
    ) -> fenestra_bridge::FenestraActivityLease {
        fenestra_bridge::ActivityRegistry::default()
            .lease(fenestra_bridge::ActivityOptions::new(""), None)
    }
    pub fn emit_bridge_event(&self, _name: impl Into<String>, _payload: serde_json::Value) -> bool {
        false
    }
    pub fn metrics(&self) -> FenestraLaunchMetricsSnapshot {
        FenestraLaunchMetricsSnapshot {
            label: String::new(),
            elapsed: std::time::Duration::ZERO,
            stages: Vec::new(),
        }
    }
    pub fn take_desktop_events(&self) -> Vec<fenestra_platform::PlatformEvent> {
        Vec::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WebView2Error {
    #[error("WebView2 backend is only supported on Windows")]
    UnsupportedOnThisPlatform,
}

pub type WebView2Result<T> = Result<T, WebView2Error>;

// Re-export the fenestra-platform types so the API surface is identical to the
// CEF crate.
pub use fenestra_platform::{
    ShellSurfaceAnchor, ShellSurfaceKeyboardInteractivity, ShellSurfaceLayer, ShellSurfaceMargin,
    ShellSurfaceOptions,
};

// Type re-exports that are needed by callers regardless of platform.
pub type WebView2Region = WindowRegion;
pub type WebView2RegionRect = WindowRegionRect;
pub type WebView2Regions = WindowRegions;
pub type WebView2BackgroundEffect = WindowBackgroundEffect;
pub type WebView2AutostartEntry = AutostartEntry;
pub type WebView2DeepLinkRegistration = DeepLinkRegistration;
pub type WebView2GlobalShortcutRegistration = GlobalShortcutRegistration;
pub type WebView2NativeMessagingHost = NativeMessagingHost;
pub type WebView2SingleInstancePolicy = SingleInstancePolicy;
pub type WebView2TrayIcon = TrayIcon;

// Reserved for the Windows-only path. Defining them here keeps the
// `cargo doc --workspace` run clean on non-Windows hosts.
#[allow(dead_code)]
pub(crate) fn _unused_paths() -> Vec<PathBuf> {
    Vec::new()
}
