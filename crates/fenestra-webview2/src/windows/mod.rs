// Windows implementation of the WebView2 (Evergreen) backend.
//
// This module is gated to `target_os = "windows"`. It is loaded
// instead of `stub.rs` on Windows hosts. The structure mirrors the
// `WebView2Window` / `WebView2Process` API in `stub.rs`; see that
// file for the cross-platform surface that the CEF crate depends on.
//
// The WebView2 / winit / Win32 API calls follow the published docs
// for `webview2-com 0.36`, `winit 0.31`, and `windows 0.60`. The
// module cross-compiles on Linux via
// `cargo check --target x86_64-pc-windows-gnu`; full verification
// still needs a real Windows host because the WebView2 runtime is
// only available there.

#![cfg(target_os = "windows")]

mod bridge;
mod host_controls;
mod launch;
mod regions;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc::Sender},
    time::Duration,
};

use fenestra_bridge::{
    ActivityEventEmitter, ActivityHostUpdate, ActivityOptions, ActivityRecord, ActivityRegistry,
    BridgeCommand, BridgeCommandDescriptor, BridgeHandlers, BridgeRegistry, BridgeResult,
    WebViewSecurity,
};
use fenestra_runtime::RuntimeInfo;
use stuk_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    SingleInstancePolicy, TrayIcon, WindowBackgroundEffect, WindowRegion, WindowRegionRect,
    WindowRegions,
};
use stuk_platform_shell::ShellSurfaceOptions;

/// Re-export the shared window/region types so the API surface is
/// identical to the CEF crate. App code that does
/// `use fenestra_cef::WindowRegionRect` keeps working.
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
pub use stuk_platform_shell::{
    ShellSurfaceAnchor as WebView2ShellSurfaceAnchor,
    ShellSurfaceKeyboardInteractivity as WebView2ShellSurfaceKeyboardInteractivity,
    ShellSurfaceLayer as WebView2ShellSurfaceLayer,
    ShellSurfaceMargin as WebView2ShellSurfaceMargin,
    ShellSurfaceOptions as WebView2ShellSurfaceOptions,
};

/// Browser-chrome mode for a WebView2 window. Mirrors the
/// `FenestraWindowChrome` enum in `fenestra-cef`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WebView2WindowChrome {
    #[default]
    System,
    Fenestra,
    Frameless,
    None,
}

impl WebView2WindowChrome {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "fenestra" | "fenestra-chrome" | "custom" => Some(Self::Fenestra),
            "frameless" => Some(Self::Frameless),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Fenestra => "fenestra",
            Self::Frameless => "frameless",
            Self::None => "none",
        }
    }

    pub fn uses_native_decorations(self) -> bool {
        matches!(self, Self::System)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebView2WindowControlAction {
    Minimize,
    Maximize,
    Close,
}

impl WebView2WindowControlAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "minimize" => Some(Self::Minimize),
            "maximize" => Some(Self::Maximize),
            "close" => Some(Self::Close),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
            Self::Close => "close",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebView2WindowControlRegion {
    pub action: WebView2WindowControlAction,
    pub rect: WebView2RegionRect,
}

impl WebView2WindowControlRegion {
    pub fn new(action: WebView2WindowControlAction, rect: WebView2RegionRect) -> Self {
        Self { action, rect }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebView2LifecyclePolicy {
    pub active_frame_rate: u32,
    pub background_frame_rate: u32,
    pub suspend_on_minimize: bool,
    pub suspend_on_occluded: bool,
    pub suspend_on_blur: bool,
    pub hibernate_after: Option<Duration>,
    pub hibernate_grace: Duration,
}

impl WebView2LifecyclePolicy {
    pub fn browser_tab() -> Self {
        Self {
            active_frame_rate: 60,
            background_frame_rate: 5,
            suspend_on_minimize: true,
            suspend_on_occluded: true,
            suspend_on_blur: true,
            hibernate_after: Some(Duration::from_secs(300)),
            hibernate_grace: Duration::from_millis(750),
        }
    }

    pub fn hidden_window() -> Self {
        Self {
            active_frame_rate: 60,
            background_frame_rate: 1,
            suspend_on_minimize: true,
            suspend_on_occluded: true,
            suspend_on_blur: true,
            hibernate_grace: Duration::from_millis(150),
            hibernate_after: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WebView2DesktopServiceConfig {
    pub tray_icon: Option<TrayIcon>,
    pub autostart: Vec<AutostartEntry>,
    pub global_shortcuts: Vec<GlobalShortcutRegistration>,
    pub deep_links: Vec<DeepLinkRegistration>,
    pub native_messaging_hosts: Vec<NativeMessagingHost>,
    pub single_instance_id: Option<String>,
    pub single_instance_policy: Option<SingleInstancePolicy>,
}

#[derive(Clone, Debug)]
pub struct WebView2Config {
    pub entry: Option<String>,
    pub url: Option<String>,
    pub dev_url: Option<String>,
    pub dev_command: Option<String>,
    pub app_id: Option<String>,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub resizable: bool,
    pub visible: bool,
    pub active: bool,
    pub hide_on_blur: bool,
    pub always_on_top: bool,
    pub transparent: bool,
    pub frameless: bool,
    pub chrome: WebView2WindowChrome,
    pub background_effect: WindowBackgroundEffect,
    pub low_power_background_effect: Option<WindowBackgroundEffect>,
    pub regions: WebView2Regions,
    pub shell_surface: Option<ShellSurfaceOptions>,
    pub drag_regions: Vec<WindowRegionRect>,
    pub drag_exclusion_regions: Vec<WindowRegionRect>,
    pub control_regions: Vec<WebView2WindowControlRegion>,
    pub desktop_services: WebView2DesktopServiceConfig,
    pub lifecycle: WebView2LifecyclePolicy,
    pub runtime: fenestra_runtime::RuntimeConfig,
    pub bridge: BridgeRegistry,
    pub security: WebViewSecurity,
}

impl Default for WebView2Config {
    fn default() -> Self {
        Self {
            entry: None,
            url: None,
            dev_url: None,
            dev_command: None,
            app_id: None,
            title: "Fenestra".to_string(),
            width: 900,
            height: 640,
            min_width: 420,
            min_height: 280,
            resizable: true,
            visible: true,
            active: true,
            hide_on_blur: false,
            always_on_top: false,
            transparent: false,
            frameless: false,
            chrome: WebView2WindowChrome::System,
            background_effect: WindowBackgroundEffect::None,
            low_power_background_effect: None,
            regions: WebView2Regions::default(),
            shell_surface: None,
            drag_regions: Vec::new(),
            drag_exclusion_regions: Vec::new(),
            control_regions: Vec::new(),
            desktop_services: WebView2DesktopServiceConfig::default(),
            lifecycle: WebView2LifecyclePolicy::default(),
            runtime: fenestra_runtime::RuntimeConfig::default(),
            bridge: BridgeRegistry::default(),
            security: WebViewSecurity::default(),
        }
    }
}

impl WebView2Config {
    pub fn effective_background_effect(&self) -> WindowBackgroundEffect {
        if let Some(effect) = self.low_power_background_effect
            && low_power_glass_requested()
        {
            return effect;
        }
        self.background_effect
    }
}

fn low_power_glass_requested() -> bool {
    env_flag("FENESTRA_LOW_POWER_GLASS")
        || env_flag("STACCATO_LOW_POWER_MODE")
        || std::env::var("STACCATO_POWER_PROFILE")
            .map(|value| matches!(value.as_str(), "battery" | "low-power" | "power-saver"))
            .unwrap_or(false)
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Builder for a WebView2-backed window. Mirrors `FenestraWindow` in
/// `fenestra-cef` so the same app code can target either backend by
/// picking the right `FenestraWindow` type alias.
#[derive(Clone, Debug)]
pub struct WebView2Window {
    pub config: WebView2Config,
    bridge_handlers: BridgeHandlers,
}

/// Per-platform overrides for the default `.glass()` material.
///
/// `GlassSpec` lets an app ask for a different background effect on
/// each target OS without `cfg`-gating the builder. Pass it to
/// [`WebView2Window::glass_spec`].
///
/// String values are parsed through
/// [`WindowBackgroundEffect::parse`](stuk_platform::WindowBackgroundEffect::parse),
/// so unknown names silently fall back to the platform default. The
/// effect names live in `stuk-platform`; the Asher-specific ones
/// (`luca`, `niko`, `maris`) are intentionally not surfaced through
/// fenestra yet, but the parser still accepts them if you need to set
/// them explicitly via `glass_effect` / `glass_material`.
///
/// Default per platform (when the spec does not override the field):
///
/// | OS      | Effect      | Notes                                         |
/// | ------- | ----------- | --------------------------------------------- |
/// | Windows | `Acrylic`   | DWM Acrylic system backdrop                   |
/// | macOS   | `Vibrancy`  | NSVisualEffectView, the most transparent blur |
/// | Linux   | `Blur`      | Wayland `ext_background_effect_v1` blur       |
/// | Asher   | (no default) | Asher is not implemented yet                 |
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlassSpec {
    windows: Option<WindowBackgroundEffect>,
    macos: Option<WindowBackgroundEffect>,
    linux: Option<WindowBackgroundEffect>,
}

impl GlassSpec {
    /// Empty spec; resolving it falls back to the per-platform
    /// defaults listed in [`GlassSpec`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the Windows effect. Unknown names are ignored.
    pub fn windows(mut self, effect: &str) -> Self {
        self.windows = WindowBackgroundEffect::parse(effect);
        self
    }

    /// Override the macOS effect. Unknown names are ignored.
    pub fn macos(mut self, effect: &str) -> Self {
        self.macos = WindowBackgroundEffect::parse(effect);
        self
    }

    /// Override the Linux effect. Unknown names are ignored.
    pub fn linux(mut self, effect: &str) -> Self {
        self.linux = WindowBackgroundEffect::parse(effect);
        self
    }

    pub(crate) fn resolve(self) -> WindowBackgroundEffect {
        match stuk_platform::current_desktop_os() {
            stuk_platform::PlatformOs::Windows => {
                self.windows.unwrap_or(WindowBackgroundEffect::Acrylic)
            }
            stuk_platform::PlatformOs::Macos => {
                self.macos.unwrap_or(WindowBackgroundEffect::Vibrancy)
            }
            stuk_platform::PlatformOs::Linux => self.linux.unwrap_or(WindowBackgroundEffect::Blur),
            _ => WindowBackgroundEffect::None,
        }
    }
}

impl WebView2Window {
    pub fn new() -> Self {
        Self {
            config: WebView2Config::default(),
            bridge_handlers: BridgeHandlers::default(),
        }
    }

    pub fn entry(mut self, path: impl Into<String>) -> Self {
        self.config.entry = Some(path.into());
        self
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        allow_url_origin(&mut self.config.security, &url);
        self.config.url = Some(url);
        self
    }

    pub fn dev_url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        allow_dev_origins(&mut self.config.security, &url);
        self.config.dev_url = Some(url);
        self
    }

    pub fn dev_command(mut self, command: impl Into<String>) -> Self {
        self.config.dev_command = Some(command.into());
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.config.title = title.into();
        self
    }

    pub fn app_id(mut self, id: impl Into<String>) -> Self {
        self.config.app_id = Some(id.into());
        self
    }

    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.config.width = width;
        self.config.height = height;
        self
    }

    pub fn min_size(mut self, width: u32, height: u32) -> Self {
        self.config.min_width = width;
        self.config.min_height = height;
        self
    }

    pub fn fixed_size(mut self, width: u32, height: u32) -> Self {
        self.config.width = width;
        self.config.height = height;
        self.config.min_width = width;
        self.config.min_height = height;
        self.config.resizable = false;
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.config.resizable = resizable;
        self
    }

    pub fn visible(mut self, visible: bool) -> Self {
        self.config.visible = visible;
        if !visible {
            self.config.lifecycle.suspend_on_blur = true;
            self.config.lifecycle.background_frame_rate = 1;
        }
        self
    }

    pub fn hidden(self) -> Self {
        self.visible(false)
    }

    pub fn always_on_top(mut self, always_on_top: bool) -> Self {
        self.config.always_on_top = always_on_top;
        self
    }

    pub fn transparent(mut self, transparent: bool) -> Self {
        self.config.transparent = transparent;
        if !transparent {
            self.config.background_effect = WindowBackgroundEffect::None;
        }
        self
    }

    pub fn frameless(mut self) -> Self {
        self.config.frameless = true;
        self.config.chrome = WebView2WindowChrome::Frameless;
        self
    }

    pub fn fenestra_chrome(mut self) -> Self {
        self.config.frameless = true;
        self.config.chrome = WebView2WindowChrome::Fenestra;
        self
    }

    pub fn no_chrome(mut self) -> Self {
        self.config.frameless = true;
        self.config.chrome = WebView2WindowChrome::None;
        self
    }

    pub fn system_chrome(mut self) -> Self {
        self.config.frameless = false;
        self.config.chrome = WebView2WindowChrome::System;
        self
    }

    pub fn opaque(mut self) -> Self {
        self.config.transparent = false;
        self.config.background_effect = WindowBackgroundEffect::None;
        self
    }

    pub fn glass(self) -> Self {
        self.glass_spec(GlassSpec::new())
    }

    pub fn glass_spec(mut self, spec: GlassSpec) -> Self {
        self.config.transparent = true;
        self.config.background_effect = spec.resolve();
        self
    }

    pub fn glass_effect(mut self, effect: WindowBackgroundEffect) -> Self {
        self.config.transparent = true;
        self.config.background_effect = effect;
        self
    }

    pub fn glass_material(self, effect: WindowBackgroundEffect) -> Self {
        self.glass_effect(effect)
    }

    pub fn drag_region(mut self, rect: WindowRegionRect) -> Self {
        self.config.drag_regions.push(rect);
        self
    }

    pub fn drag_exclusion_region(mut self, rect: WindowRegionRect) -> Self {
        self.config.drag_exclusion_regions.push(rect);
        self
    }

    pub fn regions(mut self, regions: WebView2Regions) -> Self {
        self.config.regions = regions;
        self
    }

    pub fn blur_region(mut self, region: WindowRegion) -> Self {
        self.config.regions.blur = Some(region);
        self
    }

    pub fn opaque_region(mut self, region: WindowRegion) -> Self {
        self.config.regions.opaque = Some(region);
        self
    }

    pub fn input_region(mut self, region: WindowRegion) -> Self {
        self.config.regions.input = Some(region);
        self
    }

    pub fn control_region(
        mut self,
        action: WebView2WindowControlAction,
        rect: WindowRegionRect,
    ) -> Self {
        self.config
            .control_regions
            .push(WebView2WindowControlRegion::new(action, rect));
        self
    }

    pub fn tray_icon(mut self, icon: TrayIcon) -> Self {
        self.config.desktop_services.tray_icon = Some(icon);
        self
    }

    pub fn autostart(mut self, entry: AutostartEntry) -> Self {
        self.config.desktop_services.autostart.push(entry);
        self
    }

    pub fn lifecycle_policy(mut self, lifecycle: WebView2LifecyclePolicy) -> Self {
        self.config.lifecycle = lifecycle;
        self
    }

    pub fn runtime(mut self, runtime: fenestra_runtime::RuntimeConfig) -> Self {
        self.config.runtime = runtime;
        self
    }

    pub fn security(mut self, security: WebViewSecurity) -> Self {
        self.config.security = security;
        self
    }

    pub fn allowed_origin(mut self, origin: impl Into<String>) -> Self {
        allow_origin(&mut self.config.security, origin.into());
        self
    }

    pub fn bridge_command_descriptor(mut self, descriptor: BridgeCommandDescriptor) -> Self {
        self.config.bridge.register_descriptor(descriptor);
        self
    }

    pub fn bridge_handler<F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(BridgeCommand) -> BridgeResult + Send + Sync + 'static,
    {
        let name = name.into();
        self.config.bridge.register(name.clone());
        self.bridge_handlers.register(name, handler);
        self
    }

    pub fn bridge_descriptor_handler<F>(
        mut self,
        descriptor: BridgeCommandDescriptor,
        handler: F,
    ) -> Self
    where
        F: Fn(BridgeCommand) -> BridgeResult + Send + Sync + 'static,
    {
        let name = descriptor.name.clone();
        self.config.bridge.register_descriptor(descriptor);
        self.bridge_handlers.register(name, handler);
        self
    }

    pub fn launch(self) -> Result<WebView2Process, WebView2Error> {
        let runtime = fenestra_runtime::resolve_runtime(&self.config.runtime)?;
        self.launch_with_runtime(runtime)
    }

    pub fn launch_or_install(self) -> Result<WebView2Process, WebView2Error> {
        let runtime = fenestra_runtime::ensure_runtime(&self.config.runtime)?;
        self.launch_with_runtime(runtime)
    }

    pub fn launch_with_runtime(
        self,
        runtime: RuntimeInfo,
    ) -> Result<WebView2Process, WebView2Error> {
        launch::launch(self, runtime)
    }
}

impl Default for WebView2Window {
    fn default() -> Self {
        Self::new()
    }
}

pub struct WebView2Process {
    pub(crate) inner: Arc<WebView2ProcessInner>,
}

pub(crate) struct WebView2ProcessInner {
    pub(crate) hwnd: std::sync::atomic::AtomicIsize,
    pub(crate) controller:
        Mutex<Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller>>,
    pub(crate) webview: Mutex<Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2>>,
    pub(crate) bridge_runtime: Mutex<Option<fenestra_bridge::BridgeRuntime>>,
    pub(crate) activity: ActivityRegistry,
    pub(crate) emitter: Arc<WebView2ActivityEmitter>,
    pub(crate) metrics: fenestra_bridge::LaunchMetrics,
    pub(crate) event_sender: Sender<launch::WebView2UserEvent>,
    pub(crate) runtime: RuntimeInfo,
    pub(crate) background_frame_rate: u32,
    pub(crate) command_allowlist: Vec<String>,
}

impl WebView2Process {
    pub fn id(&self) -> u32 {
        std::process::id()
    }

    pub fn wait(self) -> std::io::Result<std::process::ExitStatus> {
        use std::os::windows::process::ExitStatusExt;
        Ok(std::process::ExitStatus::from_raw(0))
    }

    pub fn show(&self) -> bool {
        host_controls::show_window(self.inner.hwnd.load(std::sync::atomic::Ordering::Relaxed))
    }

    pub fn hide(&self) -> bool {
        host_controls::hide_window(self.inner.hwnd.load(std::sync::atomic::Ordering::Relaxed))
    }

    pub fn focus_window(&self) -> bool {
        host_controls::focus_window(self.inner.hwnd.load(std::sync::atomic::Ordering::Relaxed))
    }

    pub fn set_visible(&self, visible: bool) -> bool {
        if visible { self.show() } else { self.hide() }
    }

    pub fn set_shell_surface_visible(&self, visible: bool) -> bool {
        self.set_visible(visible)
    }

    pub fn set_shell_surface_alpha(&self, _alpha: f32) -> bool {
        false
    }

    pub fn begin_activity(
        &self,
        name: impl Into<String>,
    ) -> fenestra_bridge::FenestraActivityLease {
        self.begin_activity_with(ActivityOptions::new(name))
    }

    pub fn begin_activity_with(
        &self,
        options: ActivityOptions,
    ) -> fenestra_bridge::FenestraActivityLease {
        let emitter: Arc<dyn ActivityEventEmitter> = self.inner.emitter.clone();
        self.inner.activity.lease(options, Some(emitter))
    }

    pub fn activities(&self) -> Vec<ActivityRecord> {
        self.inner.activity.list()
    }

    pub fn bridge_event_emitter(&self) -> Option<WebView2EventEmitter> {
        Some(WebView2EventEmitter {
            sender: self.inner.event_sender.clone(),
            activity: self.inner.activity.clone(),
        })
    }

    pub fn emit_bridge_event(&self, name: impl Into<String>, payload: serde_json::Value) -> bool {
        self.bridge_event_emitter()
            .is_some_and(|emitter| emitter.emit(name, payload))
    }

    pub fn metrics(&self) -> fenestra_bridge::FenestraLaunchMetricsSnapshot {
        self.inner.metrics.snapshot()
    }

    pub fn take_desktop_events(&self) -> Vec<stuk_platform::PlatformEvent> {
        Vec::new()
    }
}

/// Engine-neutral activity event emitter for WebView2. Activity
/// updates are sent to the page as `fenestra.activity.begin` /
/// `fenestra.activity.end` bridge events, which the page-side
/// `window.fenestra.activity.*` API already listens for.
pub struct WebView2ActivityEmitter {
    sender: Sender<launch::WebView2UserEvent>,
    activity: ActivityRegistry,
}

impl ActivityEventEmitter for WebView2ActivityEmitter {
    fn emit_activity_update(&self, update: &ActivityHostUpdate) -> bool {
        let _ = self.sender.send(launch::WebView2UserEvent::Activity {
            update: update.clone(),
        });
        true
    }
}

#[derive(Clone)]
pub struct WebView2EventEmitter {
    sender: Sender<launch::WebView2UserEvent>,
    #[allow(dead_code)]
    activity: ActivityRegistry,
}

impl WebView2EventEmitter {
    pub fn emit(&self, name: impl Into<String>, payload: serde_json::Value) -> bool {
        launch::WebView2UserEvent::BridgeEvent {
            name: name.into(),
            payload,
        }
        .dispatch(&self.sender)
    }

    pub fn set_visible(&self, visible: bool) -> bool {
        launch::WebView2UserEvent::SetVisible(visible).dispatch(&self.sender)
    }

    pub fn show(&self) -> bool {
        launch::WebView2UserEvent::Show.dispatch(&self.sender)
    }

    pub fn hide(&self) -> bool {
        launch::WebView2UserEvent::Hide.dispatch(&self.sender)
    }

    pub fn focus_window(&self) -> bool {
        launch::WebView2UserEvent::Focus.dispatch(&self.sender)
    }

    pub fn focus_window_with_activation_token(&self, _token: Option<&str>) -> bool {
        self.focus_window()
    }

    pub fn emit_activity_update(&self, update: &ActivityHostUpdate) -> bool {
        launch::WebView2UserEvent::Activity {
            update: update.clone(),
        }
        .dispatch(&self.sender)
    }
}

impl ActivityEventEmitter for WebView2EventEmitter {
    fn emit_activity_update(&self, update: &ActivityHostUpdate) -> bool {
        WebView2EventEmitter::emit_activity_update(self, update)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WebView2Error {
    #[error("Fenestra WebView2 backend: {0}")]
    Backend(String),
    #[error(transparent)]
    Runtime(#[from] fenestra_runtime::RuntimeError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type WebView2Result<T> = Result<T, WebView2Error>;

impl From<&'static str> for WebView2Error {
    fn from(message: &'static str) -> Self {
        WebView2Error::Backend(message.to_string())
    }
}

impl From<String> for WebView2Error {
    fn from(message: String) -> Self {
        WebView2Error::Backend(message)
    }
}

#[cfg(target_os = "windows")]
impl From<webview2_com::Error> for WebView2Error {
    fn from(error: webview2_com::Error) -> Self {
        WebView2Error::Backend(format!("WebView2: {error}"))
    }
}

#[cfg(target_os = "windows")]
impl From<windows::core::Error> for WebView2Error {
    fn from(error: windows::core::Error) -> Self {
        WebView2Error::Backend(format!("WebView2: {error}"))
    }
}

fn allow_origin(security: &mut WebViewSecurity, origin: String) {
    security.remote_content = true;
    if !security
        .allowed_origins
        .iter()
        .any(|allowed| allowed == &origin)
    {
        security.allowed_origins.push(origin);
    }
}

fn allow_url_origin(security: &mut WebViewSecurity, url: &str) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return;
    }
    let origin = format!("{scheme}://{authority}");
    allow_origin(security, origin);
}

fn allow_dev_origins(security: &mut WebViewSecurity, url: &str) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority.rsplit('@').next().unwrap_or("");
    if host.is_empty() {
        return;
    }
    security.remote_content = true;
    for variant in [host, "localhost", "127.0.0.1", "::1"] {
        allow_origin(security, format!("{scheme}://{variant}"));
    }
}

/// `FenestraWindow` type alias. On Windows the public type is the
/// `WebView2Window` defined in this crate; on every other platform the
/// CEF crate owns the alias and points at the CefWindow struct there.
pub type FenestraWindow = WebView2Window;

pub(crate) type UserDataPath = PathBuf;
