#![allow(dead_code)]

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use winit::{
    application::ApplicationHandler,
    cursor::{Cursor, CursorIcon},
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window as WinitWindow, WindowAttributes, WindowId, WindowLevel},
};
use x11rb::{
    connection::Connection,
    protocol::xproto::{
        Arc as X11Arc, ConfigureWindowAux, ConnectionExt, CoordMode, CreateGCAux, Gcontext, Point,
        Rectangle, Window as X11Window,
    },
    rust_connection::RustConnection,
};

mod osr;
mod osr_host;
mod osr_protocol;

pub use fenestra_cef::{
    ActivityOptions, ActivityRecord, CefActivityLease, CefLaunchMetric, CefLaunchMetricsSnapshot,
    CefLifecyclePolicy, CefWindowChrome, CefWindowControlAction, CefWindowControlRegion,
    FENESTRA_TRACE_ENV, ShellSurfaceAnchor, ShellSurfaceKeyboardInteractivity, ShellSurfaceLayer,
    ShellSurfaceMargin, ShellSurfaceOptions,
};
pub use fenestra_runtime::{
    RuntimeConfig, RuntimeEngine, RuntimeError, RuntimeInfo, RuntimeInstallProgress,
    RuntimeInstallStep, RuntimeMode, install_user_runtime_with_progress,
    launchable_cef_host_candidates, remove_user_minimal_runtime_if_client_requested,
    resolve_runtime,
};
pub use stuk_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    PlatformEvent, SingleInstancePolicy, TrayIcon, TrayMenuItem, WindowBackgroundEffect,
    WindowChrome, WindowRegion, WindowRegionRect, WindowRegions,
};
pub use stuk_style::Material;

pub const INSTALLING_WINDOW_ARG: &str = "--stuk-fenestra-installing-runtime";
pub const NATIVE_HOST_ARG: &str = "--stuk-fenestra-native-host";
const HOST_CONTROL_PREFIX: &str = "FENESTRA_HOST_CONTROL";

const WEBVIEW_TITLEBAR_HEIGHT: u32 = 48;
static WEBVIEW_INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Error)]
pub enum WebViewError {
    #[error(
        "CEF runtime not found; run `fenestra runtime install cef` or configure a runtime path"
    )]
    RuntimeNotFound,
    #[error("{0}")]
    Runtime(#[from] RuntimeError),
    #[error("CEF runtime version {found} is below minimum {required}")]
    RuntimeVersionTooLow { found: String, required: String },
    #[error("CEF runtime at {path} failed integrity check")]
    RuntimeIntegrityFailed { path: PathBuf },
    #[error("webview creation failed: {message}")]
    CreationFailed { message: String },
    #[error("bridge error: {message}")]
    BridgeError { message: String },
    #[error("security policy violation: {message}")]
    SecurityViolation { message: String },
}

impl From<fenestra_cef::CefError> for WebViewError {
    fn from(error: fenestra_cef::CefError) -> Self {
        match error {
            fenestra_cef::CefError::Runtime(error) => Self::Runtime(error),
            fenestra_cef::CefError::CreationFailed { message } => Self::CreationFailed { message },
            fenestra_cef::CefError::MobileSystemWebViewRequired => Self::CreationFailed {
                message: "CEF webviews use system webviews on mobile targets".to_string(),
            },
        }
    }
}

type WebViewResult<T> = std::result::Result<T, WebViewError>;

#[derive(Clone, Debug)]
pub struct WebViewConfig {
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
    pub material: Material,
    pub chrome: WindowChrome,
    pub cef_chrome: Option<CefWindowChrome>,
    pub frameless: bool,
    pub transparent: bool,
    pub background_effect: WindowBackgroundEffect,
    pub low_power_background_effect: Option<WindowBackgroundEffect>,
    pub regions: WindowRegions,
    pub shell_surface: Option<ShellSurfaceOptions>,
    pub drag_regions: Vec<WindowRegionRect>,
    pub drag_exclusion_regions: Vec<WindowRegionRect>,
    pub control_regions: Vec<CefWindowControlRegion>,
    pub lifecycle: CefLifecyclePolicy,
    pub security: WebViewSecurity,
    pub runtime: RuntimeConfig,
    pub bridge: BridgeRegistry,
    pub desktop_services: DesktopServiceConfig,
}

impl Default for WebViewConfig {
    fn default() -> Self {
        Self {
            entry: None,
            url: None,
            dev_url: None,
            dev_command: None,
            app_id: None,
            title: "Stuk".to_string(),
            width: 900,
            height: 640,
            min_width: 420,
            min_height: 280,
            resizable: true,
            visible: true,
            active: true,
            hide_on_blur: false,
            always_on_top: false,
            material: Material::Maris,
            chrome: WindowChrome::System,
            cef_chrome: None,
            frameless: false,
            transparent: true,
            background_effect: WindowBackgroundEffect::None,
            low_power_background_effect: None,
            regions: WindowRegions::default(),
            shell_surface: None,
            drag_regions: Vec::new(),
            drag_exclusion_regions: Vec::new(),
            control_regions: Vec::new(),
            lifecycle: CefLifecyclePolicy::default(),
            security: WebViewSecurity::default(),
            runtime: RuntimeConfig::default(),
            bridge: BridgeRegistry::default(),
            desktop_services: DesktopServiceConfig::default(),
        }
    }
}

impl WebViewConfig {
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

fn background_effect_for_glass_material(material: &Material) -> WindowBackgroundEffect {
    match material {
        Material::Luca => WindowBackgroundEffect::Luca,
        Material::Niko => WindowBackgroundEffect::Niko,
        Material::Maris => WindowBackgroundEffect::Maris,
        _ => WindowBackgroundEffect::Blur,
    }
}

#[derive(Clone, Debug, Default)]
pub struct DesktopServiceConfig {
    pub tray_icon: Option<TrayIcon>,
    pub autostart: Vec<AutostartEntry>,
    pub global_shortcuts: Vec<GlobalShortcutRegistration>,
    pub deep_links: Vec<DeepLinkRegistration>,
    pub native_messaging_hosts: Vec<NativeMessagingHost>,
    pub single_instance_id: Option<String>,
    pub single_instance_policy: Option<SingleInstancePolicy>,
}

#[derive(Clone, Debug)]
pub struct WebViewSecurity {
    pub remote_content: bool,
    pub allowed_origins: Vec<String>,
    pub allowed_bridge_permissions: Vec<String>,
    pub devtools: WebViewDevtools,
    pub allow_eval: bool,
    pub allow_node: bool,
    pub csp: String,
}

impl Default for WebViewSecurity {
    fn default() -> Self {
        Self {
            remote_content: false,
            allowed_origins: Vec::new(),
            allowed_bridge_permissions: Vec::new(),
            devtools: WebViewDevtools::DevOnly,
            allow_eval: false,
            allow_node: false,
            csp: "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'"
                .to_string(),
        }
    }
}

impl WebViewSecurity {
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        self.remote_content = true;
        let origin = origin.into();
        if !self
            .allowed_origins
            .iter()
            .any(|allowed| allowed == &origin)
        {
            self.allowed_origins.push(origin);
        }
        self
    }

    pub fn allow_bridge_permission(mut self, permission: impl Into<String>) -> Self {
        let permission = permission.into();
        if !self
            .allowed_bridge_permissions
            .iter()
            .any(|allowed| allowed == &permission)
        {
            self.allowed_bridge_permissions.push(permission);
        }
        self
    }

    pub fn remote_content(mut self, enabled: bool) -> Self {
        self.remote_content = enabled;
        self
    }

    fn to_cef(&self) -> fenestra_cef::WebViewSecurity {
        fenestra_cef::WebViewSecurity {
            remote_content: self.remote_content,
            allowed_origins: self.allowed_origins.clone(),
            allowed_bridge_permissions: self.allowed_bridge_permissions.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebViewDevtools {
    Disabled,
    DevOnly,
    Enabled,
}

#[derive(Clone, Debug)]
pub struct WebViewWindow {
    pub config: WebViewConfig,
    bridge_handlers: BridgeHandlers,
}

pub struct WebViewProcess {
    cef: Option<fenestra_cef::CefProcess>,
    child: Option<Child>,
    bridge_thread: Option<JoinHandle<()>>,
    bridge_emitter: Option<BridgeEventEmitter>,
    desktop_services: Option<fenestra_cef::LinuxDesktopServiceState>,
    desktop_event_thread: Option<JoinHandle<()>>,
    desktop_event_running: Option<Arc<AtomicBool>>,
}

impl WebViewProcess {
    fn from_cef(process: fenestra_cef::CefProcess) -> Self {
        Self {
            cef: Some(process),
            child: None,
            bridge_thread: None,
            bridge_emitter: None,
            desktop_services: None,
            desktop_event_thread: None,
            desktop_event_running: None,
        }
    }

    pub fn id(&self) -> u32 {
        if let Some(process) = &self.cef {
            return process.id();
        }
        self.child.as_ref().map(Child::id).unwrap_or_default()
    }

    pub fn wait(mut self) -> io::Result<ExitStatus> {
        if let Some(process) = self.cef.take() {
            return process.wait();
        }
        let Some(child) = self.child.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "webview process has no child process",
            ));
        };
        let status = child.wait();
        if let Some(running) = &self.desktop_event_running {
            running.store(false, Ordering::Relaxed);
        }
        if let Some(thread) = self.desktop_event_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.bridge_thread.take() {
            let _ = thread.join();
        }
        status
    }

    pub fn take_desktop_events(&self) -> Vec<PlatformEvent> {
        if let Some(process) = &self.cef {
            return process.take_desktop_events();
        }
        self.desktop_services
            .as_ref()
            .map(fenestra_cef::LinuxDesktopServiceState::take_events)
            .unwrap_or_default()
    }

    pub fn emit_bridge_event(&self, name: impl Into<String>, payload: serde_json::Value) -> bool {
        let name = name.into();
        if let Some(process) = &self.cef {
            return process.emit_bridge_event(name, payload);
        }
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.emit(name, payload))
    }

    pub fn set_shell_surface_visible(&self, visible: bool) -> bool {
        self.set_visible(visible)
    }

    pub fn set_visible(&self, visible: bool) -> bool {
        if let Some(process) = &self.cef {
            return process.set_visible(visible);
        }
        self.bridge_emitter.as_ref().is_some_and(|emitter| {
            emitter.emit_host_control("visible", if visible { "1" } else { "0" })
        })
    }

    pub fn show(&self) -> bool {
        if let Some(process) = &self.cef {
            return process.show();
        }
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.emit_host_control("show", "1"))
    }

    pub fn hide(&self) -> bool {
        if let Some(process) = &self.cef {
            return process.hide();
        }
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.emit_host_control("hide", "1"))
    }

    pub fn focus_window(&self) -> bool {
        if let Some(process) = &self.cef {
            return process.focus_window();
        }
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.emit_host_control("focus", "1"))
    }

    pub fn begin_activity(&self, name: impl Into<String>) -> Option<CefActivityLease> {
        self.cef
            .as_ref()
            .map(|process| process.begin_activity(name))
    }

    pub fn begin_activity_with(&self, options: ActivityOptions) -> Option<CefActivityLease> {
        self.cef
            .as_ref()
            .map(|process| process.begin_activity_with(options))
    }

    pub fn activities(&self) -> Vec<ActivityRecord> {
        self.cef
            .as_ref()
            .map(fenestra_cef::CefProcess::activities)
            .unwrap_or_default()
    }

    pub fn metrics(&self) -> Option<CefLaunchMetricsSnapshot> {
        self.cef.as_ref().map(fenestra_cef::CefProcess::metrics)
    }

    fn start_desktop_event_forwarder(&mut self) {
        let (Some(services), Some(emitter)) =
            (self.desktop_services.as_ref(), self.bridge_emitter.clone())
        else {
            return;
        };
        let running = Arc::new(AtomicBool::new(true));
        self.desktop_event_running = Some(Arc::clone(&running));
        self.desktop_event_thread = Some(fenestra_cef::start_desktop_event_forwarder(
            services,
            running,
            move |event| {
                let (name, payload) = platform_event_payload(event);
                let _ = emitter.emit(name, payload);
            },
        ));
    }
}

#[derive(Clone)]
pub struct BridgeEventEmitter {
    stdin: Arc<Mutex<std::process::ChildStdin>>,
}

impl BridgeEventEmitter {
    pub fn emit(&self, name: impl Into<String>, payload: serde_json::Value) -> bool {
        let event = BridgeIpcEvent {
            name: name.into(),
            payload,
        };
        self.write_line(event)
    }

    fn emit_host_control(&self, command: &str, value: &str) -> bool {
        self.write_line(format!("{HOST_CONTROL_PREFIX}\t{command}\t{value}"))
    }

    fn write_line(&self, line: impl std::fmt::Display) -> bool {
        let Ok(mut stdin) = self.stdin.lock() else {
            return false;
        };
        if writeln!(stdin, "{line}").is_err() {
            return false;
        }
        stdin.flush().is_ok()
    }
}

fn platform_event_payload(event: PlatformEvent) -> (&'static str, serde_json::Value) {
    match event {
        PlatformEvent::Tray(activation) => (
            "tray.activate",
            serde_json::json!({
                "trayId": activation.tray_id,
                "itemId": activation.item_id,
                "action": activation.action,
            }),
        ),
        PlatformEvent::GlobalShortcut(activation) => (
            "globalShortcut.activate",
            serde_json::json!({
                "id": activation.id,
                "action": activation.action,
                "activationToken": activation.activation_token,
            }),
        ),
        PlatformEvent::SingleInstance(activation) => (
            "singleInstance.activate",
            serde_json::json!({
                "policy": format!("{:?}", activation.policy),
                "arguments": activation.arguments,
                "workingDirectory": activation.working_directory,
                "activationToken": activation.activation_token,
            }),
        ),
    }
}

impl WebViewWindow {
    pub fn new() -> Self {
        Self {
            config: WebViewConfig::default(),
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

    pub fn remote_url(self, url: impl Into<String>) -> Self {
        self.url(url)
    }

    pub fn bundled_url(self, url: impl Into<String>) -> Self {
        self.url(url)
    }

    pub fn dev_url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        allow_dev_origins(&mut self.config.security, &url);
        self.config.dev_url = Some(url);
        self
    }

    pub fn dev_server(self, url: impl Into<String>) -> Self {
        self.dev_url(url)
    }

    pub fn vite_dev_server(self, port: u16) -> Self {
        self.dev_url(format!("http://localhost:{port}"))
            .dev_command(format!("bun run dev -- --port {port} --strictPort"))
    }

    pub fn vite_dev_server_with_query(self, port: u16, query: impl AsRef<str>) -> Self {
        let query = query.as_ref().trim_start_matches('?');
        self.dev_url(format!("http://localhost:{port}?{query}"))
            .dev_command(format!("bun run dev -- --port {port} --strictPort"))
    }

    pub fn dev_command(mut self, command: impl Into<String>) -> Self {
        self.config.dev_command = Some(command.into());
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.config.title = title.into();
        self
    }

    pub fn app_id(mut self, app_id: impl Into<String>) -> Self {
        self.config.app_id = Some(app_id.into());
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
            self.apply_hidden_lifecycle_defaults();
        }
        self
    }

    pub fn hidden(self) -> Self {
        self.visible(false)
    }

    pub fn active(mut self, active: bool) -> Self {
        self.config.active = active;
        self
    }

    pub fn hide_on_blur(mut self, enabled: bool) -> Self {
        self.config.hide_on_blur = enabled;
        if enabled {
            self.apply_hidden_lifecycle_defaults();
        }
        self
    }

    pub fn always_on_top(mut self, always_on_top: bool) -> Self {
        self.config.always_on_top = always_on_top;
        self
    }

    pub fn material(mut self, material: Material) -> Self {
        self.config.material = material;
        self
    }

    pub fn transparent(mut self, transparent: bool) -> Self {
        self.config.transparent = transparent;
        if !transparent {
            self.config.background_effect = WindowBackgroundEffect::None;
            self.config.low_power_background_effect = None;
            self.config.regions.blur = None;
        }
        self
    }

    pub fn opaque(mut self) -> Self {
        self.config.transparent = false;
        self.config.background_effect = WindowBackgroundEffect::None;
        self.config.low_power_background_effect = None;
        self.config.regions.blur = None;
        self
    }

    pub fn glass(self) -> Self {
        self.glass_material(Material::Luca)
    }

    pub fn glass_material(mut self, material: Material) -> Self {
        self.config.background_effect = background_effect_for_glass_material(&material);
        self.config.material = material;
        self.config.transparent = true;
        self
    }

    pub fn glass_low_power_material(mut self, material: Material) -> Self {
        self.config.low_power_background_effect =
            Some(background_effect_for_glass_material(&material));
        self
    }

    pub fn background_effect(mut self, effect: WindowBackgroundEffect) -> Self {
        self.config.background_effect = effect;
        if effect.requires_transparency() {
            self.config.transparent = true;
        }
        self
    }

    pub fn regions(mut self, regions: WindowRegions) -> Self {
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

    pub fn chrome(mut self, chrome: WindowChrome) -> Self {
        self.config.chrome = chrome;
        self.config.cef_chrome = None;
        self.config.frameless = !chrome.uses_native_decorations();
        self
    }

    pub fn cef_chrome(mut self, chrome: CefWindowChrome) -> Self {
        self.config.cef_chrome = Some(chrome);
        self.config.frameless = !chrome.uses_native_decorations();
        self.config.chrome = match chrome {
            CefWindowChrome::System => WindowChrome::System,
            CefWindowChrome::Fenestra => WindowChrome::Stuk,
            CefWindowChrome::Frameless | CefWindowChrome::None => WindowChrome::None,
        };
        self
    }

    pub fn system_chrome(mut self) -> Self {
        self.config.chrome = WindowChrome::System;
        self.config.cef_chrome = Some(CefWindowChrome::System);
        self.config.frameless = false;
        self
    }

    pub fn fenestra_chrome(mut self) -> Self {
        self.config.chrome = WindowChrome::Stuk;
        self.config.cef_chrome = Some(CefWindowChrome::Fenestra);
        self.config.frameless = true;
        self
    }

    pub fn frameless(mut self) -> Self {
        self.config.chrome = WindowChrome::None;
        self.config.cef_chrome = Some(CefWindowChrome::Frameless);
        self.config.frameless = true;
        self
    }

    pub fn with_frameless(mut self, frameless: bool) -> Self {
        self.config.frameless = frameless;
        self.config.cef_chrome = Some(if frameless {
            CefWindowChrome::Frameless
        } else {
            CefWindowChrome::System
        });
        self.config.chrome = if frameless {
            WindowChrome::None
        } else {
            WindowChrome::System
        };
        self
    }

    pub fn no_chrome(mut self) -> Self {
        self.config.chrome = WindowChrome::None;
        self.config.cef_chrome = Some(CefWindowChrome::None);
        self.config.frameless = true;
        self
    }

    pub fn shell_surface(mut self, shell_surface: ShellSurfaceOptions) -> Self {
        self.config.shell_surface = Some(shell_surface);
        self.config.chrome = WindowChrome::None;
        self.config.cef_chrome = Some(CefWindowChrome::None);
        self.config.frameless = true;
        self.config.transparent = true;
        self
    }

    pub fn drag_region(mut self, rect: WindowRegionRect) -> Self {
        self.config.drag_regions.push(rect);
        self
    }

    pub fn drag_exclusion_region(mut self, rect: WindowRegionRect) -> Self {
        self.config.drag_exclusion_regions.push(rect);
        self
    }

    pub fn titlebar_drag_region(self, height: i32) -> Self {
        self.drag_region(WindowRegionRect::new(0, 0, i32::MAX, height))
    }

    pub fn control_region(
        mut self,
        action: CefWindowControlAction,
        rect: WindowRegionRect,
    ) -> Self {
        self.config
            .control_regions
            .push(CefWindowControlRegion::new(action, rect));
        self
    }

    pub fn lifecycle_policy(mut self, lifecycle: CefLifecyclePolicy) -> Self {
        self.config.lifecycle = lifecycle;
        self
    }

    pub fn active_frame_rate(mut self, frame_rate: u32) -> Self {
        self.config.lifecycle.active_frame_rate = frame_rate.max(1);
        self
    }

    pub fn background_frame_rate(mut self, frame_rate: u32) -> Self {
        self.config.lifecycle.background_frame_rate = frame_rate.max(1);
        self
    }

    pub fn suspend_on_minimize(mut self, enabled: bool) -> Self {
        self.config.lifecycle.suspend_on_minimize = enabled;
        self
    }

    pub fn suspend_on_occluded(mut self, enabled: bool) -> Self {
        self.config.lifecycle.suspend_on_occluded = enabled;
        self
    }

    pub fn suspend_on_blur(mut self, enabled: bool) -> Self {
        self.config.lifecycle.suspend_on_blur = enabled;
        self
    }

    pub fn hibernate_after(mut self, duration: Duration) -> Self {
        self.config.lifecycle.hibernate_after = Some(duration);
        self
    }

    pub fn disable_hibernation(mut self) -> Self {
        self.config.lifecycle.hibernate_after = None;
        self
    }

    fn apply_hidden_lifecycle_defaults(&mut self) {
        self.config.lifecycle.suspend_on_blur = true;
        self.config.lifecycle.background_frame_rate = 1;
        self.config.lifecycle.hibernate_grace = self
            .config
            .lifecycle
            .hibernate_grace
            .min(Duration::from_millis(150));
    }

    pub fn security(mut self, security: WebViewSecurity) -> Self {
        self.config.security = security;
        self
    }

    pub fn allowed_origin(mut self, origin: impl Into<String>) -> Self {
        allow_origin(&mut self.config.security, origin.into());
        self
    }

    pub fn allowed_bridge_origin(self, origin: impl Into<String>) -> Self {
        self.allowed_origin(origin)
    }

    pub fn runtime(mut self, runtime: RuntimeConfig) -> Self {
        self.config.runtime = runtime;
        self
    }

    pub fn bridge(mut self, bridge: BridgeRegistry) -> Self {
        self.config.bridge = bridge;
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

    pub fn global_shortcut(mut self, registration: GlobalShortcutRegistration) -> Self {
        self.config
            .desktop_services
            .global_shortcuts
            .push(registration);
        self
    }

    pub fn deep_link(mut self, registration: DeepLinkRegistration) -> Self {
        self.config.desktop_services.deep_links.push(registration);
        self
    }

    pub fn native_messaging_host(mut self, host: NativeMessagingHost) -> Self {
        self.config
            .desktop_services
            .native_messaging_hosts
            .push(host);
        self
    }

    pub fn single_instance(mut self, policy: SingleInstancePolicy) -> Self {
        self.config.desktop_services.single_instance_policy = Some(policy);
        self
    }

    pub fn single_instance_id(mut self, id: impl Into<String>) -> Self {
        self.config.desktop_services.single_instance_id = Some(id.into());
        self
    }

    pub fn bridge_command(mut self, command_name: impl Into<String>) -> Self {
        self.config.bridge.register(command_name);
        self
    }

    pub fn bridge_handler<F>(mut self, command_name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(BridgeCommand) -> BridgeResult + Send + Sync + 'static,
    {
        let name = command_name.into();
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

    pub fn bridge_handler_async<F, Fut>(self, command_name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(BridgeCommand) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = BridgeResult> + Send + 'static,
    {
        self.bridge_descriptor_handler_async(BridgeCommandDescriptor::new(command_name), handler)
    }

    pub fn bridge_descriptor_handler_async<F, Fut>(
        mut self,
        descriptor: BridgeCommandDescriptor,
        handler: F,
    ) -> Self
    where
        F: Fn(BridgeCommand) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = BridgeResult> + Send + 'static,
    {
        let name = descriptor.name.clone();
        self.config.bridge.register_descriptor(descriptor);
        self.bridge_handlers
            .register(name, move |command| pollster::block_on(handler(command)));
        self
    }

    fn into_cef_window(self) -> fenestra_cef::CefWindow {
        let config = self.config;
        let cef_chrome = webview_cef_chrome(&config);
        let mut window = fenestra_cef::CefWindow::new();
        window.config = fenestra_cef::CefConfig {
            entry: config.entry,
            url: config.url,
            dev_url: config.dev_url,
            dev_command: config.dev_command,
            app_id: config.app_id,
            title: config.title,
            width: config.width,
            height: config.height,
            min_width: config.min_width,
            min_height: config.min_height,
            resizable: config.resizable,
            visible: config.visible,
            active: config.active,
            hide_on_blur: config.hide_on_blur,
            always_on_top: config.always_on_top,
            transparent: config.transparent,
            frameless: config.frameless || !cef_chrome.uses_native_decorations(),
            chrome: cef_chrome,
            background_effect: config.background_effect,
            low_power_background_effect: config.low_power_background_effect,
            regions: config.regions,
            shell_surface: config.shell_surface,
            drag_regions: config.drag_regions,
            drag_exclusion_regions: config.drag_exclusion_regions,
            control_regions: config.control_regions,
            desktop_services: fenestra_cef::DesktopServiceConfig {
                tray_icon: config.desktop_services.tray_icon,
                autostart: config.desktop_services.autostart,
                global_shortcuts: config.desktop_services.global_shortcuts,
                deep_links: config.desktop_services.deep_links,
                native_messaging_hosts: config.desktop_services.native_messaging_hosts,
                single_instance_id: config.desktop_services.single_instance_id,
                single_instance_policy: config.desktop_services.single_instance_policy,
            },
            lifecycle: config.lifecycle,
            runtime: config.runtime,
            bridge: config.bridge.to_cef(),
            security: config.security.to_cef(),
        };

        for (name, handler) in self.bridge_handlers.handlers {
            window = window.bridge_handler(name, move |command| {
                handler(BridgeCommand {
                    name: command.name,
                    params: command.params,
                    origin: command.origin,
                })
                .map(|response| fenestra_cef::BridgeResponse::json(response.result))
                .map_err(|error| fenestra_cef::BridgeError::new(error.message))
            });
        }

        window
    }

    pub fn launch(self) -> WebViewResult<WebViewProcess> {
        let runtime = resolve_runtime(&self.config.runtime)?;
        self.launch_with_runtime(runtime)
    }

    pub fn launch_or_install(self) -> WebViewResult<WebViewProcess> {
        let runtime = resolve_or_install_runtime(&self.config.runtime)?;
        self.launch_with_runtime(runtime)
    }

    pub fn launch_with_runtime(self, runtime: RuntimeInfo) -> WebViewResult<WebViewProcess> {
        let cef_window = self.into_cef_window();
        cef_window
            .launch_with_runtime(runtime)
            .map(WebViewProcess::from_cef)
            .map_err(WebViewError::from)
    }

    fn entry_url(&self) -> WebViewResult<String> {
        if let Some(url) = &self.config.dev_url {
            return Ok(url.clone());
        }
        if let Some(url) = &self.config.url {
            return Ok(url.clone());
        }

        let Some(entry) = &self.config.entry else {
            return Err(WebViewError::CreationFailed {
                message: "webview has no entry, URL, or dev URL".to_string(),
            });
        };
        let (entry_path, suffix) = split_entry_suffix(entry);
        let path = canonical_entry(entry_path)?;
        Ok(format!("file://{}{}", path.display(), suffix))
    }

    fn ensure_default_bridge_handlers(&mut self) {
        for command in self.config.bridge.commands() {
            if self.bridge_handlers.contains(&command) {
                continue;
            }
            let command_name = command.clone();
            self.bridge_handlers.register(command, move |_| {
                Err(BridgeError::new(format!(
                    "Bridge command `{command_name}` has no Rust handler"
                )))
            });
        }
    }
}

pub fn run_installing_window_from_args(args: &[String]) -> bool {
    args.iter().any(|arg| arg == INSTALLING_WINDOW_ARG)
}

pub fn run_native_host_from_args(args: &[String]) -> bool {
    if fenestra_cef::run_fenestra_host_from_args(args) {
        return true;
    }
    if osr::run_from_args(args) {
        return true;
    }
    let Some(index) = args.iter().position(|arg| arg == NATIVE_HOST_ARG) else {
        return false;
    };
    let Some(config_path) = args.get(index + 1).map(PathBuf::from) else {
        eprintln!("missing webview native host config path");
        std::process::exit(1);
    };
    if let Err(error) = run_native_host(config_path) {
        eprintln!("webview native host failed: {error}");
        std::process::exit(1);
    }
    true
}

fn prepare_bridge_command(command: &mut Command, bridge_handlers: &BridgeHandlers) {
    if bridge_handlers.is_empty() {
        command.stdin(Stdio::null()).stdout(Stdio::null());
        return;
    }
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
}

struct BridgeDispatch {
    thread: Option<JoinHandle<()>>,
    emitter: Option<BridgeEventEmitter>,
}

fn spawn_bridge_dispatch(child: &mut Child, bridge_runtime: BridgeRuntime) -> BridgeDispatch {
    if bridge_runtime.is_empty() {
        return BridgeDispatch {
            thread: None,
            emitter: None,
        };
    }
    let Some(stdout) = child.stdout.take() else {
        return BridgeDispatch {
            thread: None,
            emitter: None,
        };
    };
    let Some(stdin) = child.stdin.take() else {
        return BridgeDispatch {
            thread: None,
            emitter: None,
        };
    };
    let stdin = Arc::new(Mutex::new(stdin));
    let emitter = BridgeEventEmitter {
        stdin: Arc::clone(&stdin),
    };
    let thread = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(std::result::Result::ok) {
            let Some(request) = BridgeIpcRequest::parse(&line) else {
                continue;
            };
            let response = bridge_runtime.dispatch(request.command);
            let line = BridgeIpcResponse::from_result(request.browser_id, request.id, response)
                .serialize();
            let Ok(mut stdin) = stdin.lock() else {
                break;
            };
            if writeln!(stdin, "{line}").is_err() {
                break;
            }
            let _ = stdin.flush();
        }
    });
    BridgeDispatch {
        thread: Some(thread),
        emitter: Some(emitter),
    }
}

#[derive(Debug)]
struct BridgeIpcRequest {
    browser_id: String,
    id: String,
    command: BridgeCommand,
}

impl BridgeIpcRequest {
    fn parse(line: &str) -> Option<Self> {
        let parts = line.splitn(6, '\t').collect::<Vec<_>>();
        if parts.first().copied()? != "FENESTRA_BRIDGE_REQUEST" {
            return None;
        }
        if parts.len() == 5 {
            let params = serde_json::from_str(parts[4]).ok()?;
            return Some(Self {
                browser_id: parts[1].to_string(),
                id: parts[2].to_string(),
                command: BridgeCommand {
                    name: parts[3].to_string(),
                    params,
                    origin: None,
                },
            });
        }
        if parts.len() != 6 {
            return None;
        }
        let params = serde_json::from_str(parts[5]).ok()?;
        let origin = if parts[3].is_empty() {
            None
        } else {
            Some(parts[3].to_string())
        };
        Some(Self {
            browser_id: parts[1].to_string(),
            id: parts[2].to_string(),
            command: BridgeCommand {
                name: parts[4].to_string(),
                params,
                origin,
            },
        })
    }
}

#[derive(Debug)]
struct BridgeIpcResponse {
    browser_id: String,
    id: String,
    ok: bool,
    payload: serde_json::Value,
}

impl BridgeIpcResponse {
    fn from_result(browser_id: String, id: String, result: BridgeResult) -> Self {
        match result {
            Ok(response) => Self {
                browser_id,
                id,
                ok: true,
                payload: response.result,
            },
            Err(error) => Self {
                browser_id,
                id,
                ok: false,
                payload: serde_json::json!({ "message": error.message }),
            },
        }
    }

    fn serialize(&self) -> String {
        let status = if self.ok { "ok" } else { "error" };
        let payload = serde_json::to_string(&self.payload).unwrap_or_else(|_| "null".to_string());
        format!(
            "FENESTRA_BRIDGE_RESPONSE\t{}\t{}\t{status}\t{payload}",
            self.browser_id, self.id
        )
    }
}

#[derive(Debug)]
struct BridgeIpcEvent {
    name: String,
    payload: serde_json::Value,
}

impl std::fmt::Display for BridgeIpcEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = serde_json::to_string(&self.name).unwrap_or_else(|_| "\"event\"".to_string());
        let payload = serde_json::to_string(&self.payload).unwrap_or_else(|_| "null".to_string());
        write!(formatter, "FENESTRA_BRIDGE_EVENT\t{name}\t{payload}")
    }
}

pub fn resolve_or_install_runtime(
    config: &RuntimeConfig,
) -> std::result::Result<RuntimeInfo, RuntimeError> {
    remove_user_minimal_runtime_if_client_requested(config)?;
    match resolve_runtime(config) {
        Ok(runtime) => Ok(runtime),
        Err(_) if config.allow_user_install => install_runtime_with_window(config),
        Err(error) => Err(error),
    }
}

fn install_runtime_with_window(
    config: &RuntimeConfig,
) -> std::result::Result<RuntimeInfo, RuntimeError> {
    install_user_runtime_with_progress(config, |_| {})
}

fn launch_native_host_process(
    runtime_dir: &Path,
    config: &WebViewConfig,
    bridge_handlers: &BridgeHandlers,
    url: &str,
) -> WebViewResult<Option<WebViewProcess>> {
    #[cfg(target_os = "linux")]
    {
        let host_binary = fenestra_cef::ensure_cef_host(runtime_dir)
            .map_err(|message| WebViewError::CreationFailed { message })?;
        if !use_x11_embedded_compat() {
            if !use_wayland_windowed_compat() {
                return osr::launch_process(runtime_dir, config, bridge_handlers, url).map(Some);
            }
            return launch_wayland_cef_host_process(
                runtime_dir,
                &host_binary,
                config,
                bridge_handlers,
                url,
            )
            .map(Some);
        }

        let host_config_path = std::env::temp_dir().join(format!(
            "stuk-fenestra-host-{}.json",
            webview_instance_key()
        ));
        let body = serde_json::json!({
            "runtime_dir": runtime_dir,
            "host_binary": host_binary,
            "url": url,
            "title": config.title,
            "width": config.width,
            "height": config.height,
            "min_width": config.min_width,
            "min_height": config.min_height,
            "resizable": config.resizable,
            "visible": config.visible,
            "active": config.active,
            "always_on_top": config.always_on_top,
            "transparent": config.transparent,
            "background_effect": config.effective_background_effect().as_str(),
            "chrome": config.chrome.as_str(),
            "bridge_commands": config.bridge.commands(),
        });
        std::fs::write(&host_config_path, body.to_string()).map_err(|error| {
            WebViewError::CreationFailed {
                message: format!("failed to write webview host config: {error}"),
            }
        })?;
        let exe = std::env::current_exe().map_err(|error| WebViewError::CreationFailed {
            message: error.to_string(),
        })?;
        let mut command = Command::new(exe);
        command
            .arg(NATIVE_HOST_ARG)
            .arg(&host_config_path)
            .env("WINIT_UNIX_BACKEND", "x11")
            .env_remove("WAYLAND_DISPLAY")
            .stderr(Stdio::inherit());
        prepare_bridge_command(&mut command, bridge_handlers);
        let mut child = command
            .spawn()
            .map_err(|error| WebViewError::CreationFailed {
                message: format!("failed to launch webview native host: {error}"),
            })?;
        let bridge_dispatch = spawn_bridge_dispatch(
            &mut child,
            BridgeRuntime::new(
                bridge_handlers.clone(),
                config.bridge.clone(),
                config.security.clone(),
            ),
        );
        return Ok(Some(WebViewProcess {
            cef: None,
            child: Some(child),
            bridge_thread: bridge_dispatch.thread,
            bridge_emitter: bridge_dispatch.emitter,
            desktop_services: None,
            desktop_event_thread: None,
            desktop_event_running: None,
        }));
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = runtime_dir;
        let _ = config;
        let _ = url;
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
fn use_x11_embedded_compat() -> bool {
    match std::env::var("FENESTRA_WEBVIEW_BACKEND")
        .or_else(|_| std::env::var("STUK_WEBVIEW_BACKEND"))
    {
        Ok(value) => matches!(value.as_str(), "x11" | "x11-embedded" | "compat"),
        Err(_) => {
            std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some()
        }
    }
}

#[cfg(target_os = "linux")]
fn use_wayland_windowed_compat() -> bool {
    std::env::var("FENESTRA_WEBVIEW_BACKEND")
        .or_else(|_| std::env::var("STUK_WEBVIEW_BACKEND"))
        .is_ok_and(|value| {
            matches!(
                value.as_str(),
                "windowed" | "cef-windowed" | "wayland-windowed"
            )
        })
}

#[cfg(target_os = "linux")]
fn launch_wayland_cef_host_process(
    runtime_dir: &Path,
    host_binary: &Path,
    config: &WebViewConfig,
    bridge_handlers: &BridgeHandlers,
    url: &str,
) -> WebViewResult<WebViewProcess> {
    let release_dir = runtime_dir.join("Release");
    let cache_dir = webview_cache_dir(runtime_dir, &config.title, url);
    std::fs::create_dir_all(&cache_dir).map_err(|error| WebViewError::CreationFailed {
        message: format!("failed to create CEF cache dir: {error}"),
    })?;
    let mut command = Command::new(host_binary);
    command
        .arg(format!("--url={url}"))
        .arg(format!("--fenestra-title={}", config.title))
        .arg("--fenestra-ozone-platform=wayland")
        .arg(format!("--fenestra-width={}", 800))
        .arg(format!("--fenestra-height={}", 600))
        .arg(format!(
            "--fenestra-background-effect={}",
            config.effective_background_effect().as_str()
        ))
        .arg(format!(
            "--fenestra-bridge-commands={}",
            config.bridge.commands().join(",")
        ))
        .arg(format!("--root-cache-path={}", cache_dir.display()))
        .arg(format!(
            "--cache-path={}",
            cache_dir.join("browser").display()
        ))
        .arg("--ozone-platform=wayland")
        .arg("--enable-features=UseOzonePlatform")
        .arg("--disable-features=Vulkan,DefaultANGLEVulkan,VulkanFromANGLE")
        .arg("--disable-vulkan")
        .arg("--disable-gpu")
        .current_dir(&release_dir)
        .env("GDK_BACKEND", "wayland")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("LD_LIBRARY_PATH", ld_library_path(&release_dir))
        .stdin(Stdio::null());
    if config.transparent {
        command
            .arg("--fenestra-transparent")
            .arg("--enable-transparent-visuals")
            .arg("--transparent-painting-enabled")
            .arg("--default-background-color=0x00000000");
    }
    if config.chrome != WindowChrome::System {
        command.arg("--fenestra-frameless");
    }
    prepare_bridge_command(&mut command, bridge_handlers);
    let mut child = command
        .spawn()
        .map_err(|error| WebViewError::CreationFailed {
            message: format!("failed to launch Wayland CEF host: {error}"),
        })?;
    let bridge_dispatch = spawn_bridge_dispatch(
        &mut child,
        BridgeRuntime::new(
            bridge_handlers.clone(),
            config.bridge.clone(),
            config.security.clone(),
        ),
    );
    Ok(WebViewProcess {
        cef: None,
        child: Some(child),
        bridge_thread: bridge_dispatch.thread,
        bridge_emitter: bridge_dispatch.emitter,
        desktop_services: None,
        desktop_event_thread: None,
        desktop_event_running: None,
    })
}

fn run_native_host(config_path: PathBuf) -> std::result::Result<(), String> {
    let text = std::fs::read_to_string(&config_path).map_err(|error| error.to_string())?;
    let config: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let runtime_dir = config
        .get("runtime_dir")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "native host config missing runtime_dir".to_string())?;
    let host_binary = config
        .get("host_binary")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "native host config missing host_binary".to_string())?;
    let url = config
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "native host config missing url".to_string())?
        .to_string();
    let title = config
        .get("title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Stuk")
        .to_string();
    let width = config
        .get("width")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(800) as u32;
    let height = config
        .get("height")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(600) as u32;
    let min_width = config
        .get("min_width")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(420) as u32;
    let min_height = config
        .get("min_height")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(280) as u32;
    let resizable = config
        .get("resizable")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let visible = config
        .get("visible")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let active = config
        .get("active")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let always_on_top = config
        .get("always_on_top")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let transparent = config
        .get("transparent")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let background_effect = config
        .get("background_effect")
        .and_then(serde_json::Value::as_str)
        .and_then(WindowBackgroundEffect::parse)
        .unwrap_or(WindowBackgroundEffect::None);
    let chrome = config
        .get("chrome")
        .and_then(serde_json::Value::as_str)
        .and_then(WindowChrome::parse)
        .unwrap_or(WindowChrome::System);
    let bridge_commands = config
        .get("bridge_commands")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let _ = std::fs::remove_file(config_path);

    NativeWebViewHost {
        runtime_dir,
        host_binary,
        url,
        title,
        width,
        height,
        min_width,
        min_height,
        resizable,
        visible,
        active,
        always_on_top,
        transparent,
        background_effect,
        chrome,
        bridge_commands,
        window: None,
        child: None,
        child_window: None,
        surface_size: PhysicalSize::new(width, height),
        titlebar: WebViewTitlebarState::default(),
        launch_attempted: false,
        started: Instant::now(),
    }
    .run()
}

struct NativeWebViewHost {
    runtime_dir: PathBuf,
    host_binary: PathBuf,
    url: String,
    title: String,
    width: u32,
    height: u32,
    min_width: u32,
    min_height: u32,
    resizable: bool,
    visible: bool,
    active: bool,
    always_on_top: bool,
    transparent: bool,
    background_effect: WindowBackgroundEffect,
    chrome: WindowChrome,
    bridge_commands: Vec<String>,
    window: Option<Arc<dyn WinitWindow>>,
    child: Option<Child>,
    child_window: Option<X11Window>,
    surface_size: PhysicalSize<u32>,
    titlebar: WebViewTitlebarState,
    launch_attempted: bool,
    started: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WebViewTitlebarControl {
    Minimize,
    Maximize,
    Close,
}

#[derive(Debug)]
struct WebViewTitlebarState {
    hovered: Option<WebViewTitlebarControl>,
    pressed: Option<WebViewTitlebarControl>,
    cursor: CursorIcon,
}

impl Default for WebViewTitlebarState {
    fn default() -> Self {
        Self {
            hovered: None,
            pressed: None,
            cursor: CursorIcon::Default,
        }
    }
}

impl NativeWebViewHost {
    fn run(self) -> std::result::Result<(), String> {
        let event_loop = EventLoop::new().map_err(|error| error.to_string())?;
        event_loop.run_app(self).map_err(|error| error.to_string())
    }

    fn parent_xid(&self) -> Option<X11Window> {
        let window = self.window.as_ref()?;
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(handle) => Some(handle.window as X11Window),
            RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
            _ => None,
        }
    }

    fn titlebar_height(&self, window: &Arc<dyn WinitWindow>) -> u32 {
        webview_titlebar_height(self.chrome, window.scale_factor())
    }

    fn content_bounds(&self, window: &Arc<dyn WinitWindow>) -> (i32, i32, u32, u32) {
        let titlebar_height = self.titlebar_height(window);
        let height = self
            .surface_size
            .height
            .saturating_sub(titlebar_height)
            .max(1);
        (
            0,
            titlebar_height as i32,
            self.surface_size.width.max(1),
            height,
        )
    }

    fn resize_child(&self) {
        let (Some(window), Some(child_window)) = (&self.window, self.child_window) else {
            return;
        };
        let (x, y, width, height) = self.content_bounds(window);
        let _ = resize_x11_window(child_window, x, y, width, height);
    }

    fn redraw_chrome(&mut self) {
        if self.chrome.uses_native_decorations() {
            return;
        }
        let Some(parent) = self.parent_xid() else {
            return;
        };
        let Some(window) = &self.window else {
            return;
        };
        let titlebar_height = self.titlebar_height(window);
        let _ = draw_x11_webview_chrome(
            parent,
            self.surface_size.width,
            self.surface_size.height,
            titlebar_height,
            &self.title,
            self.titlebar.hovered,
            self.titlebar.pressed,
        );
    }

    fn update_hover(&mut self, window: &Arc<dyn WinitWindow>, x: f64, y: f64) {
        let titlebar_height = self.titlebar_height(window);
        let hovered = titlebar_control_at(self.surface_size.width, titlebar_height, x, y);
        let cursor = if hovered.is_some() {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        };
        let changed = self.titlebar.hovered != hovered || self.titlebar.cursor != cursor;
        self.titlebar.hovered = hovered;
        if self.titlebar.cursor != cursor {
            self.titlebar.cursor = cursor;
            window.set_cursor(Cursor::Icon(cursor));
        }
        if changed {
            window.request_redraw();
        }
    }

    fn press_titlebar(&mut self, window: &Arc<dyn WinitWindow>, x: f64, y: f64) -> bool {
        let titlebar_height = self.titlebar_height(window);
        if titlebar_height == 0 || y > f64::from(titlebar_height) {
            return false;
        }
        if let Some(control) = titlebar_control_at(self.surface_size.width, titlebar_height, x, y) {
            self.titlebar.pressed = Some(control);
            window.request_redraw();
        } else {
            let _ = window.drag_window();
        }
        true
    }

    fn release_titlebar(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window: &Arc<dyn WinitWindow>,
        x: f64,
        y: f64,
    ) -> bool {
        let titlebar_height = self.titlebar_height(window);
        let control = titlebar_control_at(self.surface_size.width, titlebar_height, x, y);
        let handled = if let Some(pressed) = self.titlebar.pressed.take() {
            if control == Some(pressed) {
                self.activate_titlebar_control(event_loop, window, pressed);
            }
            true
        } else {
            titlebar_height > 0 && y <= f64::from(titlebar_height)
        };
        if handled {
            window.request_redraw();
        }
        handled
    }

    fn activate_titlebar_control(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window: &Arc<dyn WinitWindow>,
        control: WebViewTitlebarControl,
    ) {
        match control {
            WebViewTitlebarControl::Minimize => window.set_minimized(true),
            WebViewTitlebarControl::Maximize => window.set_maximized(!window.is_maximized()),
            WebViewTitlebarControl::Close => {
                if let Some(child) = self.child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                event_loop.exit();
            }
        }
    }

    fn launch_child(&mut self, event_loop: &dyn ActiveEventLoop) {
        let Some(parent) = self.parent_xid() else {
            eprintln!("webview native host requires an X11 parent window");
            event_loop.exit();
            return;
        };
        let Some(window) = &self.window else {
            event_loop.exit();
            return;
        };
        let (x, y, width, height) = self.content_bounds(window);
        let release_dir = self.runtime_dir.join("Release");
        let cache_dir = webview_cache_dir(&self.runtime_dir, &self.title, &self.url);
        let _ = std::fs::create_dir_all(&cache_dir);
        let mut command = Command::new(&self.host_binary);
        command
            .arg(format!("--url={}", self.url))
            .arg(format!("--fenestra-parent-window=0x{parent:x}"))
            .arg(format!("--fenestra-x={x}"))
            .arg(format!("--fenestra-y={y}"))
            .arg(format!("--fenestra-width={}", width.max(1)))
            .arg(format!("--fenestra-height={}", height.max(1)))
            .arg(format!(
                "--fenestra-background-effect={}",
                self.background_effect.as_str()
            ))
            .arg(format!(
                "--fenestra-bridge-commands={}",
                self.bridge_commands.join(",")
            ))
            .arg(format!("--root-cache-path={}", cache_dir.display()))
            .arg(format!(
                "--cache-path={}",
                cache_dir.join("browser").display()
            ))
            .arg("--ozone-platform=x11")
            .current_dir(&release_dir)
            .env_remove("WAYLAND_DISPLAY")
            .env("GDK_BACKEND", "x11")
            .env("XDG_SESSION_TYPE", "x11")
            .env("LD_LIBRARY_PATH", ld_library_path(&release_dir));
        if self.transparent {
            command
                .arg("--fenestra-transparent")
                .arg("--enable-transparent-visuals")
                .arg("--transparent-painting-enabled")
                .arg("--default-background-color=0x00000000");
        }
        if !self.bridge_commands.is_empty() {
            command.stdin(Stdio::piped()).stdout(Stdio::piped());
        } else {
            command.stdin(Stdio::null()).stdout(Stdio::null());
        }
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!("failed to launch CEF child: {error}");
                event_loop.exit();
                return;
            }
        };
        self.child = Some(child);
        if !self.bridge_commands.is_empty()
            && let Some(child) = self.child.as_mut()
        {
            spawn_native_host_bridge_proxy(child);
        }
        for _ in 0..100 {
            if let Some(window_id) = find_x11_child(parent) {
                self.child_window = Some(window_id);
                self.resize_child();
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("CEF host started but child browser window was not visible yet");
    }
}

impl ApplicationHandler for NativeWebViewHost {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attributes = WindowAttributes::default()
            .with_title(self.title.clone())
            .with_surface_size(LogicalSize::new(
                f64::from(self.width),
                f64::from(self.height),
            ))
            .with_min_surface_size(LogicalSize::new(
                f64::from(self.min_width),
                f64::from(self.min_height),
            ))
            .with_resizable(self.resizable)
            .with_decorations(self.chrome.uses_native_decorations())
            .with_visible(self.visible)
            .with_active(self.active)
            .with_window_level(if self.always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            })
            .with_transparent(self.transparent);
        if let Some(position) = centered_window_position(event_loop, self.width, self.height) {
            attributes = attributes.with_position(position);
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::<dyn WinitWindow>::from(window),
            Err(error) => {
                eprintln!("failed to create webview native host window: {error}");
                event_loop.exit();
                return;
            }
        };
        self.surface_size = window.surface_size();
        self.window = Some(window);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        self.launch_attempted = true;
        self.launch_child(event_loop);
    }

    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if id != window.id() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                if let Some(child) = self.child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                event_loop.exit();
            }
            WindowEvent::SurfaceResized(size) => {
                self.surface_size = size;
                self.resize_child();
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.redraw_chrome();
            }
            WindowEvent::PointerMoved {
                position, primary, ..
            } if primary => {
                self.update_hover(&window, position.x, position.y);
            }
            WindowEvent::PointerLeft { primary, .. } if primary => {
                self.titlebar.hovered = None;
                if self.titlebar.cursor != CursorIcon::Default {
                    self.titlebar.cursor = CursorIcon::Default;
                    window.set_cursor(Cursor::Icon(CursorIcon::Default));
                }
                window.request_redraw();
            }
            WindowEvent::PointerButton {
                state: ElementState::Pressed,
                primary: true,
                position,
                button,
                ..
            } if button.clone().mouse_button() == Some(MouseButton::Left) => {
                let _ = self.press_titlebar(&window, position.x, position.y);
            }
            WindowEvent::PointerButton {
                state: ElementState::Released,
                primary: true,
                position,
                button,
                ..
            } if button.clone().mouse_button() == Some(MouseButton::Left) => {
                let _ = self.release_titlebar(event_loop, &window, position.x, position.y);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.started.elapsed() > Duration::from_millis(500)
            && let Some(child) = self.child.as_mut()
            && matches!(child.try_wait(), Ok(Some(_)))
        {
            event_loop.exit();
        }
    }
}

fn centered_window_position(
    event_loop: &dyn ActiveEventLoop,
    width: u32,
    height: u32,
) -> Option<PhysicalPosition<i32>> {
    let monitor = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next())?;
    let mode = monitor.current_video_mode()?;
    let monitor_size = mode.size();
    let monitor_position = monitor.position()?;
    let scale = monitor.scale_factor().max(1.0);
    let physical_width = (f64::from(width) * scale).round() as i32;
    let physical_height = (f64::from(height) * scale).round() as i32;
    let x = monitor_position.x + (monitor_size.width as i32 - physical_width).max(0) / 2;
    let y = monitor_position.y + (monitor_size.height as i32 - physical_height).max(0) / 2;
    Some(PhysicalPosition::new(x, y))
}

fn spawn_native_host_bridge_proxy(child: &mut Child) {
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            let mut output = io::stdout();
            for line in reader.lines().map_while(std::result::Result::ok) {
                if writeln!(output, "{line}").is_err() {
                    break;
                }
                let _ = output.flush();
            }
        });
    }

    if let Some(mut stdin) = child.stdin.take() {
        thread::spawn(move || {
            let input = io::stdin();
            for line in input.lock().lines().map_while(std::result::Result::ok) {
                if writeln!(stdin, "{line}").is_err() {
                    break;
                }
                let _ = stdin.flush();
            }
        });
    }
}

fn webview_titlebar_height(chrome: WindowChrome, scale_factor: f64) -> u32 {
    if matches!(
        chrome,
        WindowChrome::Stuk | WindowChrome::Compact | WindowChrome::Sidebar
    ) {
        (f64::from(WEBVIEW_TITLEBAR_HEIGHT) * scale_factor.max(1.0)).round() as u32
    } else {
        0
    }
}

fn titlebar_control_at(
    surface_width: u32,
    titlebar_height: u32,
    x: f64,
    y: f64,
) -> Option<WebViewTitlebarControl> {
    if titlebar_height == 0 || y < 0.0 || y > f64::from(titlebar_height) {
        return None;
    }
    let size = (f64::from(titlebar_height) * 0.62).clamp(22.0, 28.0);
    let gap = 8.0;
    let right = 10.0;
    let y0 = (f64::from(titlebar_height) - size) * 0.5;
    let close_x = f64::from(surface_width) - right - size;
    let maximize_x = close_x - gap - size;
    let minimize_x = maximize_x - gap - size;
    [
        (WebViewTitlebarControl::Minimize, minimize_x),
        (WebViewTitlebarControl::Maximize, maximize_x),
        (WebViewTitlebarControl::Close, close_x),
    ]
    .into_iter()
    .find_map(|(control, x0)| {
        (x >= x0 && x <= x0 + size && y >= y0 && y <= y0 + size).then_some(control)
    })
}

fn draw_x11_webview_chrome(
    window: X11Window,
    surface_width: u32,
    surface_height: u32,
    titlebar_height: u32,
    title: &str,
    hovered: Option<WebViewTitlebarControl>,
    pressed: Option<WebViewTitlebarControl>,
) -> std::result::Result<(), String> {
    let (connection, _) = RustConnection::connect(None).map_err(|error| error.to_string())?;
    let background = create_gc(&connection, window, 0x181818, 1)?;
    let titlebar = create_gc(&connection, window, 0x2c2c30, 1)?;
    let separator = create_gc(&connection, window, 0x3a3a3d, 1)?;
    let text = create_gc(&connection, window, 0xf3f3f1, 1)?;
    let icon = create_gc(&connection, window, 0xf3f3f1, 2)?;
    let icon_muted = create_gc(&connection, window, 0xd7d7d4, 2)?;
    connection
        .poly_fill_rectangle(
            window,
            background,
            &[Rectangle {
                x: 0,
                y: 0,
                width: u16_saturating(surface_width),
                height: u16_saturating(surface_height),
            }],
        )
        .map_err(|error| error.to_string())?;
    if titlebar_height > 0 {
        connection
            .poly_fill_rectangle(
                window,
                titlebar,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: u16_saturating(surface_width),
                    height: u16_saturating(titlebar_height),
                }],
            )
            .map_err(|error| error.to_string())?;
        connection
            .poly_line(
                CoordMode::ORIGIN,
                window,
                separator,
                &[
                    Point {
                        x: 0,
                        y: i16_saturating(titlebar_height.saturating_sub(1)),
                    },
                    Point {
                        x: i16_saturating(surface_width),
                        y: i16_saturating(titlebar_height.saturating_sub(1)),
                    },
                ],
            )
            .map_err(|error| error.to_string())?;
        draw_x11_title_text(
            &connection,
            window,
            titlebar,
            text,
            surface_width,
            titlebar_height,
            title,
        )?;
        draw_x11_titlebar_controls(
            &connection,
            window,
            surface_width,
            titlebar_height,
            hovered,
            pressed,
            icon,
            icon_muted,
        )?;
    }
    for gc in [background, titlebar, separator, text, icon, icon_muted] {
        let _ = connection.free_gc(gc);
    }
    connection.flush().map_err(|error| error.to_string())
}

fn create_gc(
    connection: &RustConnection,
    window: X11Window,
    color: u32,
    line_width: u32,
) -> std::result::Result<Gcontext, String> {
    let gc = connection
        .generate_id()
        .map_err(|error| error.to_string())?;
    connection
        .create_gc(
            gc,
            window,
            &CreateGCAux::new()
                .foreground(color)
                .background(0x2c2c30)
                .line_width(line_width)
                .graphics_exposures(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(gc)
}

fn draw_x11_title_text(
    connection: &RustConnection,
    window: X11Window,
    background_gc: Gcontext,
    text_gc: Gcontext,
    surface_width: u32,
    titlebar_height: u32,
    title: &str,
) -> std::result::Result<(), String> {
    let title = title.as_bytes();
    let approx_width = title.len() as i32 * 7;
    let x = ((surface_width as i32 - approx_width) / 2).max(16);
    let y = ((titlebar_height as i32 + 9) / 2).max(16);
    connection
        .poly_fill_rectangle(
            window,
            background_gc,
            &[Rectangle {
                x: i16_saturating_i32(x - 4),
                y: i16_saturating_i32(y - 13),
                width: u16_saturating((approx_width + 8).max(1) as u32),
                height: 18,
            }],
        )
        .map_err(|error| error.to_string())?;
    connection
        .image_text8(
            window,
            text_gc,
            i16_saturating_i32(x),
            i16_saturating_i32(y),
            title,
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_x11_titlebar_controls(
    connection: &RustConnection,
    window: X11Window,
    surface_width: u32,
    titlebar_height: u32,
    hovered: Option<WebViewTitlebarControl>,
    pressed: Option<WebViewTitlebarControl>,
    icon_gc: Gcontext,
    icon_muted_gc: Gcontext,
) -> std::result::Result<(), String> {
    let size = (f64::from(titlebar_height) * 0.62)
        .clamp(22.0, 28.0)
        .round() as i16;
    let gap = 8;
    let right = 10;
    let y = ((titlebar_height as i16 - size) / 2).max(0);
    let close_x = i16_saturating(surface_width) - right - size;
    let maximize_x = close_x - gap - size;
    let minimize_x = maximize_x - gap - size;
    for (control, x) in [
        (WebViewTitlebarControl::Minimize, minimize_x),
        (WebViewTitlebarControl::Maximize, maximize_x),
        (WebViewTitlebarControl::Close, close_x),
    ] {
        let fill = if pressed == Some(control) {
            0x555557
        } else if hovered == Some(control) {
            0x47474a
        } else {
            0x3b3b3e
        };
        let fill_gc = create_gc(connection, window, fill, 1)?;
        connection
            .poly_fill_arc(
                window,
                fill_gc,
                &[X11Arc {
                    x,
                    y,
                    width: size as u16,
                    height: size as u16,
                    angle1: 0,
                    angle2: 360 * 64,
                }],
            )
            .map_err(|error| error.to_string())?;
        let _ = connection.free_gc(fill_gc);
        draw_x11_control_icon(
            connection,
            window,
            if hovered == Some(control) || pressed == Some(control) {
                icon_gc
            } else {
                icon_muted_gc
            },
            control,
            x,
            y,
            size,
        )?;
    }
    Ok(())
}

fn draw_x11_control_icon(
    connection: &RustConnection,
    window: X11Window,
    gc: Gcontext,
    control: WebViewTitlebarControl,
    x: i16,
    y: i16,
    size: i16,
) -> std::result::Result<(), String> {
    let c = size / 2;
    let left = x + c - 5;
    let right = x + c + 5;
    let top = y + c - 5;
    let bottom = y + c + 5;
    let middle = y + c + 4;
    let points = match control {
        WebViewTitlebarControl::Minimize => vec![
            Point { x: left, y: middle },
            Point {
                x: right,
                y: middle,
            },
        ],
        WebViewTitlebarControl::Maximize => vec![
            Point { x: left, y: top },
            Point { x: right, y: top },
            Point {
                x: right,
                y: bottom,
            },
            Point { x: left, y: bottom },
            Point { x: left, y: top },
        ],
        WebViewTitlebarControl::Close => {
            connection
                .poly_line(
                    CoordMode::ORIGIN,
                    window,
                    gc,
                    &[
                        Point { x: left, y: top },
                        Point {
                            x: right,
                            y: bottom,
                        },
                    ],
                )
                .map_err(|error| error.to_string())?;
            vec![Point { x: right, y: top }, Point { x: left, y: bottom }]
        }
    };
    connection
        .poly_line(CoordMode::ORIGIN, window, gc, &points)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn u16_saturating(value: u32) -> u16 {
    value.min(u32::from(u16::MAX)) as u16
}

fn i16_saturating(value: u32) -> i16 {
    value.min(i16::MAX as u32) as i16
}

fn i16_saturating_i32(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

impl Default for WebViewWindow {
    fn default() -> Self {
        Self::new()
    }
}

fn runtime_command(
    runtime_dir: &Path,
    config: &WebViewConfig,
    url: &str,
    force_x11: bool,
) -> Option<Command> {
    for candidate in cef_executable_candidates(runtime_dir) {
        if !candidate.is_file() {
            continue;
        }
        let cache_dir = webview_cache_dir(runtime_dir, &config.title, url);
        let _ = std::fs::create_dir_all(&cache_dir);
        let mut command = Command::new(&candidate);
        command
            .arg(format!("--url={url}"))
            .arg("--enable-chrome-runtime")
            .arg("--use-alloy-style")
            .arg("--use-views")
            .arg("--hide-frame")
            .arg("--disable-vulkan")
            .arg("--disable-gpu")
            .arg("--hide-controls")
            .arg("--hide-overlays")
            .arg(format!(
                "--fenestra-bridge-commands={}",
                config.bridge.commands().join(",")
            ))
            .arg(format!("--root-cache-path={}", cache_dir.display()))
            .arg(format!(
                "--cache-path={}",
                cache_dir.join("browser").display()
            ))
            .current_dir(candidate.parent().unwrap_or(runtime_dir));
        if config.transparent {
            command
                .arg("--enable-transparent-visuals")
                .arg("--transparent-painting-enabled")
                .arg("--default-background-color=0x00000000");
        }
        if force_x11 {
            command.arg("--ozone-platform=x11");
        } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            command
                .arg("--ozone-platform=wayland")
                .arg("--enable-features=UseOzonePlatform");
        }
        return Some(command);
    }
    let _ = url;
    None
}

fn cef_executable_candidates(runtime_dir: &Path) -> Vec<PathBuf> {
    launchable_cef_host_candidates(runtime_dir)
}

fn ld_library_path(release_dir: &Path) -> String {
    fenestra_cef::ld_library_path(release_dir)
}

fn webview_cache_dir(_runtime_dir: &Path, title: &str, url: &str) -> PathBuf {
    fenestra_cef::webview_cache_dir(title, url)
}

fn webview_instance_key() -> String {
    let counter = WEBVIEW_INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{counter}-{timestamp}", std::process::id())
}

fn remove_system_decorations(pid: u32, previous_windows: BTreeSet<String>) {
    #[cfg(target_os = "linux")]
    {
        for _ in 0..40 {
            let window_id =
                find_x11_window_for_pid(pid).or_else(|| find_new_x11_window(&previous_windows));
            let Some(window_id) = window_id else {
                std::thread::sleep(Duration::from_millis(75));
                continue;
            };
            let _ = Command::new("xprop")
                .args([
                    "-id",
                    &window_id,
                    "-f",
                    "_MOTIF_WM_HINTS",
                    "32c",
                    "-set",
                    "_MOTIF_WM_HINTS",
                    "0x2, 0x0, 0x0, 0x0, 0x0",
                ])
                .output();
            return;
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        let _ = previous_windows;
    }
}

fn find_x11_child(parent: X11Window) -> Option<X11Window> {
    let (connection, _) = RustConnection::connect(None).ok()?;
    connection
        .query_tree(parent)
        .ok()?
        .reply()
        .ok()?
        .children
        .into_iter()
        .next()
}

fn resize_x11_window(
    child: X11Window,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> std::result::Result<(), String> {
    let (connection, _) = RustConnection::connect(None).map_err(|error| error.to_string())?;
    connection
        .configure_window(
            child,
            &ConfigureWindowAux::new()
                .x(x)
                .y(y)
                .width(width.max(1))
                .height(height.max(1)),
        )
        .map_err(|error| error.to_string())?;
    connection.flush().map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
fn find_x11_window_for_pid(pid: u32) -> Option<String> {
    for window_id in x11_client_windows() {
        let output = Command::new("xprop")
            .args(["-id", &window_id, "_NET_WM_PID"])
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let props = String::from_utf8_lossy(&output.stdout);
        if props
            .split(|ch: char| !ch.is_ascii_digit())
            .any(|part| part.parse::<u32>().ok() == Some(pid))
        {
            return Some(window_id);
        }
    }

    None
}

fn x11_client_windows() -> BTreeSet<String> {
    let root = Command::new("xprop")
        .args(["-root", "_NET_CLIENT_LIST"])
        .output();
    let Ok(root) = root else {
        return BTreeSet::new();
    };
    if !root.status.success() {
        return BTreeSet::new();
    }

    let text = String::from_utf8_lossy(&root.stdout);
    text.split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| part.starts_with("0x"))
        .map(ToString::to_string)
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn x11_client_windows() -> BTreeSet<String> {
    BTreeSet::new()
}

#[cfg(target_os = "linux")]
fn find_new_x11_window(previous_windows: &BTreeSet<String>) -> Option<String> {
    x11_client_windows()
        .into_iter()
        .find(|window_id| !previous_windows.contains(window_id))
}

#[derive(Clone, Debug)]
pub struct BridgeCommand {
    pub name: String,
    pub params: serde_json::Value,
    pub origin: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BridgeResponse {
    pub result: serde_json::Value,
}

impl BridgeResponse {
    pub fn json(result: serde_json::Value) -> Self {
        Self { result }
    }
}

#[derive(Clone, Debug)]
pub struct BridgeError {
    pub message: String,
}

impl BridgeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub type BridgeResult = std::result::Result<BridgeResponse, BridgeError>;
type BridgeHandler = Arc<dyn Fn(BridgeCommand) -> BridgeResult + Send + Sync>;

#[derive(Clone, Default)]
pub struct BridgeHandlers {
    handlers: BTreeMap<String, BridgeHandler>,
}

impl std::fmt::Debug for BridgeHandlers {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BridgeHandlers")
            .field("commands", &self.commands())
            .finish()
    }
}

impl BridgeHandlers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(&mut self, command_name: impl Into<String>, handler: F)
    where
        F: Fn(BridgeCommand) -> BridgeResult + Send + Sync + 'static,
    {
        self.handlers.insert(command_name.into(), Arc::new(handler));
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    pub fn contains(&self, command_name: &str) -> bool {
        self.handlers.contains_key(command_name)
    }

    pub fn commands(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    fn dispatch(&self, command: BridgeCommand) -> BridgeResult {
        let Some(handler) = self.handlers.get(&command.name) else {
            return Err(BridgeError::new(format!(
                "Bridge command `{}` is not registered",
                command.name
            )));
        };
        handler(command)
    }
}

#[derive(Clone, Debug)]
struct BridgeRuntime {
    handlers: BridgeHandlers,
    registry: BridgeRegistry,
    security: WebViewSecurity,
}

impl BridgeRuntime {
    fn new(handlers: BridgeHandlers, registry: BridgeRegistry, security: WebViewSecurity) -> Self {
        Self {
            handlers,
            registry,
            security,
        }
    }

    fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    fn dispatch(&self, command: BridgeCommand) -> BridgeResult {
        let descriptor = self.registry.descriptor(&command.name);
        self.validate_permissions(&command, descriptor)?;
        self.validate_targets(&command, descriptor)?;
        self.validate_origin(&command, descriptor)?;
        self.handlers.dispatch(command)
    }

    fn validate_targets(
        &self,
        command: &BridgeCommand,
        descriptor: Option<&BridgeCommandDescriptor>,
    ) -> std::result::Result<(), BridgeError> {
        let Some(descriptor) = descriptor else {
            return Ok(());
        };
        if descriptor.targets.is_empty() {
            return Ok(());
        }
        let active = current_bridge_targets();
        if descriptor
            .targets
            .iter()
            .any(|target| active.iter().any(|active| active == target))
        {
            return Ok(());
        }
        Err(BridgeError::new(format!(
            "Bridge command `{}` is unavailable on this target",
            command.name
        )))
    }

    fn validate_permissions(
        &self,
        command: &BridgeCommand,
        descriptor: Option<&BridgeCommandDescriptor>,
    ) -> std::result::Result<(), BridgeError> {
        let Some(descriptor) = descriptor else {
            return Ok(());
        };
        for permission in &descriptor.permissions {
            if !self
                .security
                .allowed_bridge_permissions
                .iter()
                .any(|allowed| allowed == permission || allowed == "*")
            {
                return Err(BridgeError::new(format!(
                    "Bridge command `{}` requires permission `{permission}`",
                    command.name
                )));
            }
        }
        Ok(())
    }

    fn validate_origin(
        &self,
        command: &BridgeCommand,
        descriptor: Option<&BridgeCommandDescriptor>,
    ) -> std::result::Result<(), BridgeError> {
        let Some(origin) = command.origin.as_deref() else {
            return Ok(());
        };
        if is_local_bridge_origin(origin) {
            return Ok(());
        }

        let command_origins = descriptor
            .map(|descriptor| descriptor.allowed_origins.as_slice())
            .unwrap_or(&[]);
        if origin_matches_any(origin, command_origins) {
            return Ok(());
        }

        if self.security.remote_content
            && origin_matches_any(origin, self.security.allowed_origins.as_slice())
        {
            return Ok(());
        }

        Err(BridgeError::new(format!(
            "Bridge command `{}` is not allowed from origin `{origin}`",
            command.name
        )))
    }
}

fn is_local_bridge_origin(origin: &str) -> bool {
    origin == "null"
        || origin == "about:blank"
        || origin.starts_with("file://")
        || origin.starts_with("devtools://")
}

fn origin_matches_any(origin: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|candidate| {
        candidate == origin
            || candidate == "*"
            || (candidate.ends_with("/*") && origin.starts_with(candidate.trim_end_matches('*')))
    })
}

fn current_bridge_targets() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    {
        &["desktop", "linux"]
    }
    #[cfg(target_os = "windows")]
    {
        &["desktop", "windows"]
    }
    #[cfg(target_os = "macos")]
    {
        &["desktop", "macos"]
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        &["desktop"]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BridgeCommandDescriptor {
    pub name: String,
    pub description: Option<String>,
    pub params_schema: Option<serde_json::Value>,
    pub permissions: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub targets: Vec<String>,
}

impl BridgeCommandDescriptor {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            params_schema: None,
            permissions: Vec::new(),
            allowed_origins: Vec::new(),
            targets: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn params_schema(mut self, schema: serde_json::Value) -> Self {
        self.params_schema = Some(schema);
        self
    }

    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.permissions.push(permission.into());
        self
    }

    pub fn allowed_origin(mut self, origin: impl Into<String>) -> Self {
        self.allowed_origins.push(origin.into());
        self
    }

    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.targets.push(target.into());
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct BridgeRegistry {
    commands: Vec<BridgeCommandDescriptor>,
}

impl BridgeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, command_name: impl Into<String>) {
        self.register_descriptor(BridgeCommandDescriptor::new(command_name));
    }

    pub fn register_descriptor(&mut self, command: BridgeCommandDescriptor) {
        if !self.is_registered(&command.name) {
            self.commands.push(command);
        }
    }

    pub fn is_registered(&self, command_name: &str) -> bool {
        self.commands.iter().any(|c| c.name == command_name)
    }

    pub fn descriptors(&self) -> &[BridgeCommandDescriptor] {
        &self.commands
    }

    pub fn descriptor(&self, command_name: &str) -> Option<&BridgeCommandDescriptor> {
        self.commands
            .iter()
            .find(|command| command.name == command_name)
    }

    pub fn commands(&self) -> Vec<String> {
        self.commands
            .iter()
            .map(|command| command.name.clone())
            .collect()
    }

    pub fn capabilities_json(&self) -> serde_json::Value {
        serde_json::json!({
            "commands": self.commands.iter().map(|command| {
                serde_json::json!({
                    "name": &command.name,
                    "description": &command.description,
                    "paramsSchema": &command.params_schema,
                    "permissions": &command.permissions,
                    "allowedOrigins": &command.allowed_origins,
                    "targets": &command.targets,
                })
            }).collect::<Vec<_>>()
        })
    }

    pub fn js_api(&self) -> String {
        let commands = serde_json::to_string(&self.commands()).unwrap_or_else(|_| "[]".to_string());
        let capabilities =
            serde_json::to_string(&self.capabilities_json()).unwrap_or_else(|_| "{}".to_string());
        format!(
            r#"(function(){{
  const commands = new Set({commands});
  const capabilities = {capabilities};
  const pending = new Map();
  const listeners = new Map();
  let nextId = 1;
  window.__fenestraBridgeResolve = function(id, ok, payload) {{
    const key = String(id);
    const entry = pending.get(key);
    if (!entry) return;
    pending.delete(key);
    if (ok) {{
      entry.resolve(payload);
    }} else {{
      entry.reject(new Error((payload && payload.message) || "Fenestra bridge command failed"));
    }}
  }};
  window.__fenestraBridgeEmit = function(name, payload) {{
    const key = String(name);
    const set = listeners.get(key);
    if (set) {{
      for (const callback of Array.from(set)) {{
        queueMicrotask(() => callback(payload));
      }}
    }}
    window.dispatchEvent(new CustomEvent("fenestra:" + key, {{ detail: payload }}));
  }};
  const bridge = {{
    __native: true,
    commands: Array.from(commands),
    capabilities,
    listen(name, callback) {{
      const key = String(name);
      let set = listeners.get(key);
      if (!set) {{
        set = new Set();
        listeners.set(key, set);
      }}
      set.add(callback);
      return () => {{
        set.delete(callback);
        if (!set.size) listeners.delete(key);
      }};
    }},
    invoke(name, params = {{}}) {{
      if (!commands.has(name)) {{
        return Promise.reject(new Error(`Fenestra bridge command not registered: ${{name}}`));
      }}
      const id = String(nextId++);
      const payload = encodeURIComponent(JSON.stringify(params));
      const url = `fenestra://bridge/${{encodeURIComponent(id)}}?name=${{encodeURIComponent(name)}}&payload=${{payload}}`;
      return new Promise((resolve, reject) => {{
        pending.set(id, {{ resolve, reject }});
        setTimeout(() => {{
          if (pending.has(id)) {{
            pending.delete(id);
            reject(new Error(`Fenestra bridge command timed out: ${{name}}`));
          }}
        }}, 60000);
        window.location.href = url;
      }});
    }}
  }};
  window.fenestra = window.fenestra || {{}};
  window.fenestra.bridge = bridge;
  window.stuk = window.stuk || {{}};
  window.stuk.bridge = bridge;
}})();"#
        )
    }

    fn to_cef(&self) -> fenestra_cef::BridgeRegistry {
        let mut registry = fenestra_cef::BridgeRegistry::default();
        for command in &self.commands {
            registry.register_descriptor(command.to_cef());
        }
        registry
    }
}

impl BridgeCommandDescriptor {
    fn to_cef(&self) -> fenestra_cef::BridgeCommandDescriptor {
        fenestra_cef::BridgeCommandDescriptor {
            name: self.name.clone(),
            description: self.description.clone(),
            params_schema: self.params_schema.clone(),
            permissions: self.permissions.clone(),
            allowed_origins: self.allowed_origins.clone(),
            targets: self.targets.clone(),
        }
    }
}

fn webview_cef_chrome(config: &WebViewConfig) -> CefWindowChrome {
    config.cef_chrome.unwrap_or(match config.chrome {
        WindowChrome::System => CefWindowChrome::System,
        WindowChrome::Stuk | WindowChrome::Compact | WindowChrome::Sidebar => {
            CefWindowChrome::Fenestra
        }
        WindowChrome::None => CefWindowChrome::None,
    })
}

fn canonical_entry(entry: &str) -> WebViewResult<PathBuf> {
    if entry.trim().is_empty() {
        return Err(WebViewError::CreationFailed {
            message: "webview entry path is empty".to_string(),
        });
    }
    let entry_path = PathBuf::from(entry);
    let path = if entry_path.is_absolute() {
        entry_path
    } else {
        std::env::current_dir()
            .map_err(|error| WebViewError::CreationFailed {
                message: error.to_string(),
            })?
            .join(entry_path)
    };
    path.canonicalize()
        .map_err(|error| WebViewError::CreationFailed {
            message: format!("failed to resolve webview entry: {error}"),
        })
}

fn split_entry_suffix(entry: &str) -> (&str, &str) {
    let split = [entry.find('?'), entry.find('#')]
        .into_iter()
        .flatten()
        .min();
    match split {
        Some(index) => (&entry[..index], &entry[index..]),
        None => (entry, ""),
    }
}

#[derive(Clone, Debug)]
struct DevUrlParts {
    scheme: String,
    host: String,
    port: u16,
}

fn allow_dev_origins(security: &mut WebViewSecurity, url: &str) {
    let Some(parts) = dev_url_parts(url) else {
        return;
    };
    for host in dev_origin_hosts(&parts.host) {
        allow_origin(security, format_origin(&parts.scheme, &host, parts.port));
    }
}

fn allow_url_origin(security: &mut WebViewSecurity, url: &str) {
    let Some(parts) = dev_url_parts(url) else {
        return;
    };
    allow_origin(
        security,
        format_origin(&parts.scheme, &parts.host, parts.port),
    );
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

fn dev_url_parts(url: &str) -> Option<DevUrlParts> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest[..authority_end].rsplit('@').next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        let (host, after_host) = stripped.split_once(']')?;
        let port = after_host
            .strip_prefix(':')
            .and_then(|value| value.parse().ok())
            .unwrap_or(default_port);
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|character| character.is_ascii_digit()) => {
                (host, port.parse().ok()?)
            }
            _ => (authority, default_port),
        }
    };
    (!host.is_empty()).then(|| DevUrlParts {
        scheme: scheme.to_string(),
        host: host.to_string(),
        port,
    })
}

fn dev_origin_hosts(host: &str) -> Vec<String> {
    let mut hosts = vec![host.to_string()];
    if is_loopback_host(host) || is_unspecified_host(host) {
        hosts.extend(["localhost", "127.0.0.1", "::1"].map(str::to_string));
    }
    let mut unique = Vec::new();
    for host in hosts {
        if !unique.iter().any(|existing| existing == &host) {
            unique.push(host);
        }
    }
    unique
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

fn is_unspecified_host(host: &str) -> bool {
    host == "0.0.0.0" || host == "::"
}

fn format_origin(scheme: &str, host: &str, port: u16) -> String {
    if is_default_port(scheme, port) {
        format!("{}://{}", scheme, format_url_host(host))
    } else {
        format!("{}://{}:{}", scheme, format_url_host(host), port)
    }
}

fn is_default_port(scheme: &str, port: u16) -> bool {
    matches!((scheme, port), ("http", 80) | ("https", 443))
}

fn format_url_host(host: &str) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webview_window_has_secure_defaults() {
        let window = WebViewWindow::new();
        let security = &window.config.security;
        assert!(!security.remote_content);
        assert!(!security.allow_eval);
        assert!(!security.allow_node);
        assert_eq!(security.devtools, WebViewDevtools::DevOnly);
        assert!(security.csp.contains("default-src 'self'"));
    }

    #[test]
    fn bridge_registry_tracks_commands() {
        let mut registry = BridgeRegistry::new();
        registry.register("unlock_vault");
        registry.register("save_note");
        registry.register("unlock_vault");
        assert!(registry.is_registered("unlock_vault"));
        assert!(registry.is_registered("save_note"));
        assert!(!registry.is_registered("delete_all"));
        assert_eq!(registry.commands().len(), 2);
    }

    #[test]
    fn webview_config_builder() {
        let window = WebViewWindow::new()
            .entry("ui/dist/index.html")
            .dev_url("http://localhost:5173")
            .material(Material::Maris)
            .chrome(WindowChrome::Compact)
            .transparent(true);
        assert_eq!(window.config.entry.as_deref(), Some("ui/dist/index.html"));
        assert_eq!(
            window.config.dev_url.as_deref(),
            Some("http://localhost:5173")
        );
        assert!(window.config.transparent);
        assert_eq!(window.config.runtime.engine, RuntimeEngine::Cef);
    }

    #[test]
    fn webview_entry_keeps_query_and_hash_suffix() {
        let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
        let window = WebViewWindow::new().entry(format!("{}?fenestra=1#/", entry.display()));

        let url = window.entry_url().unwrap();

        assert!(url.starts_with("file://"));
        assert!(url.ends_with("/src/lib.rs?fenestra=1#/"));
    }

    #[test]
    fn webview_glass_material_selects_matching_effect() {
        let window = WebViewWindow::new()
            .glass_material(Material::Niko)
            .glass_low_power_material(Material::Maris);

        assert!(window.config.transparent);
        assert_eq!(window.config.material, Material::Niko);
        assert_eq!(
            window.config.background_effect,
            WindowBackgroundEffect::Niko
        );
        assert_eq!(
            window.config.low_power_background_effect,
            Some(WindowBackgroundEffect::Maris)
        );
    }

    #[test]
    fn webview_config_exposes_current_cef_controls() {
        let window = WebViewWindow::new()
            .fenestra_chrome()
            .titlebar_drag_region(48)
            .drag_exclusion_region(WindowRegionRect::new(820, 0, 80, 48))
            .control_region(
                CefWindowControlAction::Close,
                WindowRegionRect::new(-44, 8, 28, 28),
            )
            .active_frame_rate(120)
            .background_frame_rate(3)
            .suspend_on_blur(true)
            .hibernate_after(Duration::from_secs(30));

        assert_eq!(window.config.cef_chrome, Some(CefWindowChrome::Fenestra));
        assert!(window.config.frameless);
        assert_eq!(window.config.drag_regions.len(), 1);
        assert_eq!(window.config.drag_exclusion_regions.len(), 1);
        assert_eq!(window.config.control_regions.len(), 1);
        assert_eq!(window.config.lifecycle.active_frame_rate, 120);
        assert_eq!(window.config.lifecycle.background_frame_rate, 3);
        assert!(window.config.lifecycle.suspend_on_blur);
        assert_eq!(
            window.config.lifecycle.hibernate_after,
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn hidden_webview_uses_hidden_lifecycle_defaults() {
        let window = WebViewWindow::new().hidden();
        assert!(!window.config.visible);
        assert_eq!(window.config.lifecycle.background_frame_rate, 1);
        assert!(window.config.lifecycle.suspend_on_blur);
        assert_eq!(window.config.lifecycle.hibernate_after, None);
    }

    #[test]
    fn dev_url_allows_loopback_variants() {
        let window = WebViewWindow::new().dev_server("http://localhost:5173?fenestra=1");
        let origins = &window.config.security.allowed_origins;
        assert!(window.config.security.remote_content);
        assert!(
            origins
                .iter()
                .any(|origin| origin == "http://localhost:5173")
        );
        assert!(
            origins
                .iter()
                .any(|origin| origin == "http://127.0.0.1:5173")
        );
        assert!(origins.iter().any(|origin| origin == "http://[::1]:5173"));
    }

    #[test]
    fn webview_url_sets_production_url_and_bridge_origin() {
        let window = WebViewWindow::new().url("https://raday.lantharos.com/dashboard");

        assert_eq!(
            window.entry_url().unwrap(),
            "https://raday.lantharos.com/dashboard"
        );
        assert!(window.config.security.remote_content);
        assert!(
            window
                .config
                .security
                .allowed_origins
                .iter()
                .any(|origin| origin == "https://raday.lantharos.com")
        );
    }

    #[test]
    fn webview_dev_url_takes_precedence_over_production_url() {
        let window = WebViewWindow::new()
            .url("https://raday.lantharos.com")
            .dev_url("http://localhost:5173");

        assert_eq!(window.entry_url().unwrap(), "http://localhost:5173");
        assert!(
            window
                .config
                .security
                .allowed_origins
                .iter()
                .any(|origin| origin == "https://raday.lantharos.com")
        );
        assert!(
            window
                .config
                .security
                .allowed_origins
                .iter()
                .any(|origin| origin == "http://localhost:5173")
        );
    }
}
