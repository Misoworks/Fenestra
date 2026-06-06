mod bridge;
mod desktop_services;
mod host;
mod metrics;
mod osr;
mod osr_frame_buffer;
mod osr_host;
mod osr_layer_host;
mod osr_protocol;
mod process_tree;

use std::{
    future::Future,
    io::{self, BufRead, BufReader, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(crate) use bridge::BridgeRuntime;
pub use bridge::{
    BridgeCommand, BridgeCommandDescriptor, BridgeError, BridgeHandlers, BridgeRegistry,
    BridgeResponse, BridgeResult, WebViewSecurity,
};
pub use desktop_services::{
    LinuxDesktopServiceState, apply_linux_desktop_services, start_desktop_event_forwarder,
};
pub use fenestra_runtime::{
    RuntimeConfig, RuntimeEngine, RuntimeError, RuntimeInfo, RuntimeInstallProgress,
    RuntimeInstallStep, RuntimeLocation, RuntimeMode, RuntimePackage, detect_runtime,
    install_user_runtime_with_progress, resolve_runtime, user_runtime_path,
};
pub use host::{ensure_cef_host, ld_library_path, webview_cache_dir};
use metrics::LaunchMetrics;
pub use metrics::{CefLaunchMetric, CefLaunchMetricsSnapshot, FENESTRA_TRACE_ENV};
use process_tree::{ManagedChild, prepare_child_command};
pub use stuk_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    PlatformEvent, SingleInstancePolicy, TrayIcon, TrayMenuItem, WindowBackgroundEffect,
    WindowRegion, WindowRegionRect, WindowRegions,
};
pub use stuk_platform_shell::{
    ShellSurfaceAnchor, ShellSurfaceKeyboardInteractivity, ShellSurfaceLayer, ShellSurfaceMargin,
    ShellSurfaceOptions,
};
use thiserror::Error;
use winit::{dpi::PhysicalPosition, event_loop::ActiveEventLoop};

pub(crate) const HOST_CONTROL_PREFIX: &str = "FENESTRA_HOST_CONTROL";
pub(crate) const DISABLED_CEF_FEATURES: &str = concat!(
    "Vulkan,",
    "DefaultANGLEVulkan,",
    "VulkanFromANGLE,",
    "OptimizationGuideOnDeviceModel,",
    "AutofillServerCommunication,",
    "MediaRouter,",
    "Translate,",
    "InterestFeedContentSuggestions"
);

pub(crate) fn apply_common_cef_args(command: &mut Command) {
    command
        .arg("--ozone-platform=wayland")
        .arg("--enable-features=UseOzonePlatform")
        .arg(format!("--disable-features={DISABLED_CEF_FEATURES}"))
        .arg("--disable-vulkan")
        .arg("--disable-gpu")
        .arg("--disable-background-networking")
        .arg("--disable-component-update")
        .arg("--disable-component-extensions-with-background-pages")
        .arg("--disable-default-apps")
        .arg("--disable-domain-reliability")
        .arg("--disable-extensions")
        .arg("--disable-sync")
        .arg("--disable-translate")
        .arg("--disable-breakpad")
        .arg("--disable-crash-reporter")
        .arg("--metrics-recording-only")
        .arg("--no-default-browser-check")
        .arg("--no-first-run")
        .arg("--password-store=basic");
}

#[derive(Debug, Error)]
pub enum CefError {
    #[error("{0}")]
    Runtime(#[from] RuntimeError),
    #[error("CEF webview creation failed: {message}")]
    CreationFailed { message: String },
    #[error("CEF webviews use system webviews on mobile targets, not downloadable CEF runtimes")]
    MobileSystemWebViewRequired,
}

pub type CefResult<T> = std::result::Result<T, CefError>;

pub fn run_fenestra_host_from_args(args: &[String]) -> bool {
    osr::run_from_args(args)
}

#[derive(Clone, Debug)]
pub struct CefConfig {
    pub entry: Option<String>,
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
    pub chrome: CefWindowChrome,
    pub background_effect: WindowBackgroundEffect,
    pub regions: WindowRegions,
    pub shell_surface: Option<ShellSurfaceOptions>,
    pub drag_regions: Vec<WindowRegionRect>,
    pub drag_exclusion_regions: Vec<WindowRegionRect>,
    pub control_regions: Vec<CefWindowControlRegion>,
    pub desktop_services: DesktopServiceConfig,
    pub lifecycle: CefLifecyclePolicy,
    pub runtime: RuntimeConfig,
    pub bridge: BridgeRegistry,
    pub security: WebViewSecurity,
}

impl Default for CefConfig {
    fn default() -> Self {
        Self {
            entry: None,
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
            chrome: CefWindowChrome::System,
            background_effect: WindowBackgroundEffect::None,
            regions: WindowRegions::default(),
            shell_surface: None,
            drag_regions: Vec::new(),
            drag_exclusion_regions: Vec::new(),
            control_regions: Vec::new(),
            desktop_services: DesktopServiceConfig::default(),
            lifecycle: CefLifecyclePolicy::default(),
            runtime: RuntimeConfig::default(),
            bridge: BridgeRegistry::default(),
            security: WebViewSecurity::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CefLifecyclePolicy {
    pub active_frame_rate: u32,
    pub background_frame_rate: u32,
    pub suspend_on_minimize: bool,
    pub suspend_on_occluded: bool,
    pub suspend_on_blur: bool,
    pub hibernate_after: Option<Duration>,
    pub hibernate_grace: Duration,
}

impl Default for CefLifecyclePolicy {
    fn default() -> Self {
        Self {
            active_frame_rate: 60,
            background_frame_rate: 5,
            suspend_on_minimize: true,
            suspend_on_occluded: true,
            suspend_on_blur: false,
            hibernate_after: None,
            hibernate_grace: Duration::from_millis(750),
        }
    }
}

impl CefLifecyclePolicy {
    pub fn browser_tab() -> Self {
        Self {
            suspend_on_blur: true,
            hibernate_after: Some(Duration::from_secs(300)),
            ..Self::default()
        }
    }

    pub fn hidden_window() -> Self {
        Self {
            background_frame_rate: 1,
            suspend_on_blur: true,
            hibernate_grace: Duration::from_millis(150),
            ..Self::default()
        }
    }

    pub fn memory_saver_hidden_window() -> Self {
        Self {
            hibernate_after: Some(Duration::from_secs(5)),
            ..Self::hidden_window()
        }
    }

    pub fn with_hibernate_after(mut self, duration: Duration) -> Self {
        self.hibernate_after = Some(duration);
        self
    }

    pub fn without_hibernation(mut self) -> Self {
        self.hibernate_after = None;
        self
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CefWindowChrome {
    #[default]
    System,
    Fenestra,
    Frameless,
    None,
}

impl CefWindowChrome {
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
pub enum CefWindowControlAction {
    Minimize,
    Maximize,
    Close,
}

impl CefWindowControlAction {
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
pub struct CefWindowControlRegion {
    pub action: CefWindowControlAction,
    pub rect: WindowRegionRect,
}

impl CefWindowControlRegion {
    pub fn new(action: CefWindowControlAction, rect: WindowRegionRect) -> Self {
        Self { action, rect }
    }
}

#[derive(Clone, Debug)]
pub struct CefWindow {
    pub config: CefConfig,
    bridge_handlers: BridgeHandlers,
}

pub struct CefProcess {
    child: ManagedChild,
    sidecars: Vec<ManagedChild>,
    bridge_thread: Option<JoinHandle<()>>,
    bridge_emitter: Option<BridgeEventEmitter>,
    desktop_services: Option<LinuxDesktopServiceState>,
    desktop_event_thread: Option<JoinHandle<()>>,
    desktop_event_running: Option<Arc<AtomicBool>>,
    metrics: LaunchMetrics,
}

impl CefProcess {
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn wait(mut self) -> std::io::Result<ExitStatus> {
        let status = self.child.wait();
        self.cleanup_sidecars();
        self.stop_desktop_event_forwarder();
        self.join_bridge_thread();
        status
    }

    pub fn take_desktop_events(&self) -> Vec<PlatformEvent> {
        self.desktop_services
            .as_ref()
            .map(LinuxDesktopServiceState::take_events)
            .unwrap_or_default()
    }

    pub fn emit_bridge_event(&self, name: impl Into<String>, payload: serde_json::Value) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.emit(name, payload))
    }

    pub fn set_shell_surface_visible(&self, visible: bool) -> bool {
        self.set_visible(visible)
    }

    pub fn set_shell_surface_alpha(&self, alpha: f32) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.set_alpha(alpha))
    }

    pub fn set_visible(&self, visible: bool) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.set_visible(visible))
    }

    pub fn show(&self) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(BridgeEventEmitter::show)
    }

    pub fn hide(&self) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(BridgeEventEmitter::hide)
    }

    pub fn focus_window(&self) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(BridgeEventEmitter::focus_window)
    }

    pub fn bridge_event_emitter(&self) -> Option<BridgeEventEmitter> {
        self.bridge_emitter.clone()
    }

    pub fn metrics(&self) -> CefLaunchMetricsSnapshot {
        self.metrics.snapshot()
    }

    fn start_desktop_event_forwarder(&mut self) {
        let (Some(services), Some(emitter)) =
            (self.desktop_services.as_ref(), self.bridge_emitter.clone())
        else {
            return;
        };
        let running = Arc::new(AtomicBool::new(true));
        self.desktop_event_running = Some(Arc::clone(&running));
        self.desktop_event_thread = Some(desktop_services::start_desktop_event_forwarder(
            services,
            running,
            move |event| {
                let (name, payload) = platform_event_payload(event);
                let _ = emitter.emit(name, payload);
            },
        ));
    }

    fn cleanup_sidecars(&mut self) {
        for sidecar in &mut self.sidecars {
            sidecar.terminate();
        }
        self.sidecars.clear();
    }

    fn stop_desktop_event_forwarder(&mut self) {
        if let Some(running) = &self.desktop_event_running {
            running.store(false, Ordering::Relaxed);
        }
        if let Some(thread) = self.desktop_event_thread.take() {
            let _ = thread.join();
        }
        self.desktop_event_running = None;
    }

    fn join_bridge_thread(&mut self) {
        if let Some(thread) = self.bridge_thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for CefProcess {
    fn drop(&mut self) {
        self.cleanup_sidecars();
        self.stop_desktop_event_forwarder();
        self.child.terminate();
        self.join_bridge_thread();
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

    pub fn set_visible(&self, visible: bool) -> bool {
        self.emit_host_control("visible", if visible { "1" } else { "0" })
    }

    pub fn set_alpha(&self, alpha: f32) -> bool {
        self.emit_host_control("alpha", &format!("{:.4}", alpha.clamp(0.0, 1.0)))
    }

    pub fn show(&self) -> bool {
        self.emit_host_control("show", "1")
    }

    pub fn hide(&self) -> bool {
        self.emit_host_control("hide", "1")
    }

    pub fn focus_window(&self) -> bool {
        self.emit_host_control("focus", "1")
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
            }),
        ),
    }
}

impl CefWindow {
    pub fn new() -> Self {
        Self {
            config: CefConfig::default(),
            bridge_handlers: BridgeHandlers::default(),
        }
    }

    pub fn entry(mut self, path: impl Into<String>) -> Self {
        self.config.entry = Some(path.into());
        self
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

    pub fn transparent(mut self, transparent: bool) -> Self {
        self.config.transparent = transparent;
        if !transparent {
            self.config.background_effect = WindowBackgroundEffect::None;
            self.config.regions.blur = None;
        }
        self
    }

    pub fn opaque(mut self) -> Self {
        self.config.transparent = false;
        self.config.background_effect = WindowBackgroundEffect::None;
        self.config.regions.blur = None;
        self
    }

    pub fn frameless(mut self) -> Self {
        self.config.frameless = true;
        self.config.chrome = CefWindowChrome::Frameless;
        self
    }

    pub fn fenestra_chrome(mut self) -> Self {
        self.config.frameless = true;
        self.config.chrome = CefWindowChrome::Fenestra;
        self
    }

    pub fn with_frameless(mut self, frameless: bool) -> Self {
        self.config.frameless = frameless;
        self.config.chrome = if frameless {
            CefWindowChrome::Frameless
        } else {
            CefWindowChrome::System
        };
        self
    }

    pub fn system_chrome(mut self) -> Self {
        self.config.frameless = false;
        self.config.chrome = CefWindowChrome::System;
        self
    }

    pub fn no_chrome(mut self) -> Self {
        self.config.frameless = true;
        self.config.chrome = CefWindowChrome::None;
        self
    }

    pub fn chrome(mut self, chrome: CefWindowChrome) -> Self {
        self.config.frameless = !chrome.uses_native_decorations();
        self.config.chrome = chrome;
        self
    }

    pub fn glass(mut self) -> Self {
        self.config.transparent = true;
        self.config.background_effect = WindowBackgroundEffect::Blur;
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

    pub fn shell_surface(mut self, shell_surface: ShellSurfaceOptions) -> Self {
        self.config.shell_surface = Some(shell_surface);
        self.config.frameless = true;
        self.config.chrome = CefWindowChrome::None;
        self.config.transparent = true;
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

    pub fn runtime(mut self, runtime: RuntimeConfig) -> Self {
        self.config.runtime = runtime;
        self
    }

    pub fn security(mut self, security: WebViewSecurity) -> Self {
        self.config.security = security;
        self
    }

    pub fn bridge_command_descriptor(mut self, descriptor: BridgeCommandDescriptor) -> Self {
        self.config.bridge.register_descriptor(descriptor);
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

    pub fn bridge_handler_async<F, Fut>(
        mut self,
        command_name: impl Into<String>,
        handler: F,
    ) -> Self
    where
        F: Fn(BridgeCommand) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = BridgeResult> + Send + 'static,
    {
        let name = command_name.into();
        self.config.bridge.register(name.clone());
        self.bridge_handlers
            .register(name, move |command| pollster::block_on(handler(command)));
        self
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

    pub fn launch(self) -> CefResult<CefProcess> {
        let runtime = resolve_runtime(&self.config.runtime)?;
        self.launch_with_runtime(runtime)
    }

    pub fn launch_or_install(self) -> CefResult<CefProcess> {
        let runtime = fenestra_runtime::ensure_runtime(&self.config.runtime)?;
        self.launch_with_runtime(runtime)
    }

    pub fn launch_with_runtime(mut self, runtime: RuntimeInfo) -> CefResult<CefProcess> {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = runtime;
            return Err(CefError::MobileSystemWebViewRequired);
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let metrics = LaunchMetrics::new(metrics_label(&self.config));
            metrics.mark("launch.start");
            #[cfg(target_os = "linux")]
            let desktop_services = Some(
                apply_linux_desktop_services(
                    self.config.desktop_services.tray_icon.as_ref(),
                    &self.config.desktop_services.autostart,
                    &self.config.desktop_services.global_shortcuts,
                    &self.config.desktop_services.deep_links,
                    &self.config.desktop_services.native_messaging_hosts,
                    self.config.desktop_services.single_instance_id.as_deref(),
                    self.config.desktop_services.single_instance_policy,
                )
                .map_err(|message| CefError::CreationFailed { message })?,
            );
            #[cfg(not(target_os = "linux"))]
            let desktop_services = None;
            metrics.mark("desktop_services.ready");
            self.ensure_default_bridge_handlers();
            let mut dev_server = self.start_dev_command(&metrics)?;
            let mut url = self.entry_url()?;
            match self.wait_for_dev_server(dev_server.as_mut(), &url) {
                Ok(ready_url) => {
                    url = ready_url;
                    allow_dev_origins(&mut self.config.security, &url);
                    metrics.mark("dev_server.ready");
                }
                Err(error) => {
                    if let Some(child) = dev_server {
                        ManagedChild::new(child).terminate();
                    }
                    return Err(error);
                }
            }
            let mut process = if self.should_use_osr_host() {
                osr::launch_process(
                    runtime.location.path(),
                    &self.config,
                    &self.bridge_handlers,
                    &url,
                    metrics.clone(),
                )?
            } else {
                launch_cef_host(
                    runtime.location.path(),
                    &self.config,
                    &self.bridge_handlers,
                    &url,
                    metrics.clone(),
                )?
            };
            if let Some(dev_server) = dev_server {
                metrics.mark("dev_server.attached");
                process.sidecars.push(ManagedChild::new(dev_server));
            }
            process.desktop_services = desktop_services;
            process.start_desktop_event_forwarder();
            metrics.mark("launch.ready");
            Ok(process)
        }
    }

    fn start_dev_command(&self, metrics: &LaunchMetrics) -> CefResult<Option<Child>> {
        let Some(command) = &self.config.dev_command else {
            return Ok(None);
        };
        if command.trim().is_empty() {
            return Ok(None);
        }
        let mut process = shell_command(command);
        prepare_child_command(&mut process);
        process
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let child = process.spawn().map_err(|error| CefError::CreationFailed {
            message: format!("failed to start dev command `{command}`: {error}"),
        })?;
        metrics.mark("dev_command.spawned");
        Ok(Some(child))
    }

    fn wait_for_dev_server(
        &self,
        mut dev_server: Option<&mut Child>,
        url: &str,
    ) -> CefResult<String> {
        let candidates = dev_server_candidates(url);
        if candidates.is_empty() {
            return Ok(url.to_string());
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut last_error = None;
        while Instant::now() < deadline {
            if let Some(child) = dev_server.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(CefError::CreationFailed {
                        message: format!(
                            "dev command exited before `{url}` became available: {status}"
                        ),
                    });
                }
            }
            for candidate in &candidates {
                match (candidate.host.as_str(), candidate.port).to_socket_addrs() {
                    Ok(addresses) => {
                        for socket in addresses {
                            match TcpStream::connect_timeout(&socket, Duration::from_millis(150)) {
                                Ok(_) => return Ok(candidate.url.clone()),
                                Err(error) => last_error = Some(error),
                            }
                        }
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            thread::sleep(Duration::from_millis(50));
        }

        Err(CefError::CreationFailed {
            message: format!(
                "timed out waiting for dev server `{url}`{}",
                last_error
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default()
            ),
        })
    }

    fn should_use_osr_host(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            if std::env::var("FENESTRA_CEF_BACKEND").is_ok_and(|value| {
                matches!(
                    value.as_str(),
                    "windowed" | "cef-windowed" | "system-window"
                )
            }) {
                return false;
            }
            if self.config.shell_surface.is_some() {
                return true;
            }
            if !self.config.visible {
                return true;
            }
            self.config.chrome != CefWindowChrome::System
                || self.config.frameless
                || self.config.transparent
                || self.config.background_effect != WindowBackgroundEffect::None
        }

        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    fn entry_url(&self) -> CefResult<String> {
        if let Some(url) = &self.config.dev_url {
            return Ok(url.clone());
        }
        let Some(entry) = &self.config.entry else {
            return Err(CefError::CreationFailed {
                message: "CEF window has no entry or dev URL".to_string(),
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

impl Default for CefWindow {
    fn default() -> Self {
        Self::new()
    }
}

fn launch_cef_host(
    runtime_dir: &Path,
    config: &CefConfig,
    bridge_handlers: &BridgeHandlers,
    url: &str,
    metrics: LaunchMetrics,
) -> CefResult<CefProcess> {
    let host_binary = host::ensure_cef_host(runtime_dir)
        .map_err(|message| CefError::CreationFailed { message })?;
    metrics.mark("host.ready");
    let mut command = cef_window_command(runtime_dir, &host_binary, config, url)?;
    prepare_bridge_command(&mut command, bridge_handlers);
    prepare_child_command(&mut command);
    let mut child = command.spawn().map_err(|error| CefError::CreationFailed {
        message: error.to_string(),
    })?;
    metrics.mark(format!("host.spawned.pid.{}", child.id()));
    let bridge_dispatch = spawn_bridge_dispatch(
        &mut child,
        bridge::BridgeRuntime::new(
            bridge_handlers.clone(),
            config.bridge.clone(),
            config.security.clone(),
        ),
    );
    Ok(CefProcess {
        child: ManagedChild::new(child),
        sidecars: Vec::new(),
        bridge_thread: bridge_dispatch.thread,
        bridge_emitter: bridge_dispatch.emitter,
        desktop_services: None,
        desktop_event_thread: None,
        desktop_event_running: None,
        metrics,
    })
}

fn metrics_label(config: &CefConfig) -> String {
    config
        .app_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&config.title)
        .to_string()
}

fn shell_command(command: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut process = Command::new("cmd");
        process.arg("/C").arg(command);
        process
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut process = Command::new("sh");
        process.arg("-lc").arg(command);
        process
    }
}

fn cef_window_command(
    runtime_dir: &Path,
    host_binary: &Path,
    config: &CefConfig,
    url: &str,
) -> CefResult<Command> {
    let release_dir = runtime_dir.join("Release");
    let cache_dir = host::webview_cache_dir(&config.title, url);
    std::fs::create_dir_all(&cache_dir).map_err(|error| CefError::CreationFailed {
        message: format!("failed to create CEF cache dir: {error}"),
    })?;

    let mut command = Command::new(host_binary);
    command
        .arg(format!("--url={url}"))
        .arg(format!("--fenestra-title={}", config.title))
        .arg("--fenestra-ozone-platform=wayland")
        .arg(format!("--fenestra-width={}", config.width.max(1)))
        .arg(format!("--fenestra-height={}", config.height.max(1)))
        .arg(format!(
            "--fenestra-bridge-commands={}",
            config.bridge.commands().join(",")
        ))
        .arg(format!("--root-cache-path={}", cache_dir.display()))
        .arg(format!(
            "--cache-path={}",
            cache_dir.join("browser").display()
        ));
    apply_common_cef_args(&mut command);
    command
        .current_dir(&release_dir)
        .env("GDK_BACKEND", "wayland")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("LD_LIBRARY_PATH", host::ld_library_path(&release_dir));
    if config.transparent {
        command
            .arg("--fenestra-transparent")
            .arg("--enable-transparent-visuals")
            .arg("--transparent-painting-enabled")
            .arg("--default-background-color=0x00000000");
    }
    if config.frameless {
        command.arg("--fenestra-frameless");
    }
    if !config.visible {
        command.arg("--fenestra-hidden");
    }
    Ok(command)
}

fn prepare_bridge_command(command: &mut Command, bridge_handlers: &BridgeHandlers) {
    command.stdin(Stdio::piped());
    if bridge_handlers.is_empty() {
        command.stdout(Stdio::null());
    } else {
        command.stdout(Stdio::piped());
    }
}

struct BridgeDispatch {
    thread: Option<JoinHandle<()>>,
    emitter: Option<BridgeEventEmitter>,
}

fn spawn_bridge_dispatch(
    child: &mut Child,
    bridge_runtime: bridge::BridgeRuntime,
) -> BridgeDispatch {
    let Some(stdin) = child.stdin.take() else {
        return BridgeDispatch {
            thread: None,
            emitter: None,
        };
    };
    let stdin = Arc::new(Mutex::new(stdin));
    let emitter = Some(BridgeEventEmitter {
        stdin: Arc::clone(&stdin),
    });
    if bridge_runtime.is_empty() {
        return BridgeDispatch {
            thread: None,
            emitter,
        };
    }
    let Some(stdout) = child.stdout.take() else {
        return BridgeDispatch {
            thread: None,
            emitter,
        };
    };
    let thread = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(std::result::Result::ok) {
            let Some(request) = BridgeIpcRequest::parse(&line) else {
                continue;
            };
            let response = bridge_runtime.dispatch(request.command);
            let line = BridgeIpcResponse::from_result(request.browser_id, request.id, response);
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
        emitter,
    }
}

pub(crate) fn spawn_native_host_bridge_proxy<F>(child: &mut Child, mut host_control: F)
where
    F: FnMut(String, String) + Send + 'static,
{
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
                if let Some((command, value)) = parse_host_control(&line) {
                    host_control(command.to_string(), value.to_string());
                    continue;
                }
                if line.starts_with(HOST_CONTROL_PREFIX) {
                    continue;
                }
                if writeln!(stdin, "{line}").is_err() {
                    break;
                }
                let _ = stdin.flush();
            }
        });
    }
}

pub(crate) fn parse_host_control(line: &str) -> Option<(&str, &str)> {
    let mut parts = line.splitn(3, '\t');
    if parts.next()? != HOST_CONTROL_PREFIX {
        return None;
    }
    Some((parts.next()?, parts.next().unwrap_or("1")))
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

struct BridgeIpcRequest {
    browser_id: String,
    id: String,
    command: BridgeCommand,
}

impl BridgeIpcRequest {
    fn parse(line: &str) -> Option<Self> {
        let parts = line.splitn(6, '\t').collect::<Vec<_>>();
        if parts.first().copied()? != "FENESTRA_BRIDGE_REQUEST" || parts.len() != 6 {
            return None;
        }
        let params = serde_json::from_str(parts[5]).ok()?;
        Some(Self {
            browser_id: parts[1].to_string(),
            id: parts[2].to_string(),
            command: BridgeCommand {
                origin: Some(parts[3].to_string()).filter(|origin| !origin.is_empty()),
                name: parts[4].to_string(),
                params,
            },
        })
    }
}

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
}

impl std::fmt::Display for BridgeIpcResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status = if self.ok { "ok" } else { "error" };
        let payload = serde_json::to_string(&self.payload).unwrap_or_else(|_| "null".to_string());
        write!(
            formatter,
            "FENESTRA_BRIDGE_RESPONSE\t{}\t{}\t{status}\t{payload}",
            self.browser_id, self.id
        )
    }
}

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

fn canonical_entry(entry: &str) -> CefResult<PathBuf> {
    if entry.trim().is_empty() {
        return Err(CefError::CreationFailed {
            message: "CEF entry path is empty".to_string(),
        });
    }
    let entry_path = PathBuf::from(entry);
    let path = if entry_path.is_absolute() {
        entry_path
    } else {
        std::env::current_dir()
            .map_err(|error| CefError::CreationFailed {
                message: error.to_string(),
            })?
            .join(entry_path)
    };
    path.canonicalize()
        .map_err(|error| CefError::CreationFailed {
            message: format!("failed to resolve CEF entry: {error}"),
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
    suffix: String,
}

#[derive(Clone, Debug)]
struct DevServerCandidate {
    host: String,
    port: u16,
    url: String,
}

fn dev_url_parts(url: &str) -> Option<DevUrlParts> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let suffix = rest[authority_end..].to_string();
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
        suffix,
    })
}

fn dev_server_candidates(url: &str) -> Vec<DevServerCandidate> {
    let Some(parts) = dev_url_parts(url) else {
        return Vec::new();
    };
    let mut hosts = vec![parts.host.clone()];
    if is_loopback_host(&parts.host) || is_unspecified_host(&parts.host) {
        hosts.extend(["localhost", "127.0.0.1", "::1"].map(str::to_string));
    }
    let mut candidates = Vec::new();
    for host in hosts {
        if candidates
            .iter()
            .any(|candidate: &DevServerCandidate| candidate.host == host)
        {
            continue;
        }
        candidates.push(DevServerCandidate {
            url: format_dev_url(&parts.scheme, &host, parts.port, &parts.suffix),
            host,
            port: parts.port,
        });
    }
    candidates
}

fn allow_dev_origins(security: &mut WebViewSecurity, url: &str) {
    let candidates = dev_server_candidates(url);
    if candidates.is_empty() {
        return;
    }
    security.remote_content = true;
    for candidate in candidates {
        let Some(parts) = dev_url_parts(&candidate.url) else {
            continue;
        };
        let origin = format_dev_origin(&parts.scheme, &parts.host, parts.port);
        if !security
            .allowed_origins
            .iter()
            .any(|allowed| allowed == &origin)
        {
            security.allowed_origins.push(origin);
        }
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

fn is_unspecified_host(host: &str) -> bool {
    host == "0.0.0.0" || host == "::"
}

fn format_dev_url(scheme: &str, host: &str, port: u16, suffix: &str) -> String {
    format!("{}://{}:{}{}", scheme, format_url_host(host), port, suffix)
}

fn format_dev_origin(scheme: &str, host: &str, port: u16) -> String {
    format!("{}://{}:{}", scheme, format_url_host(host), port)
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
    fn hidden_window_lifecycle_is_palette_biased() {
        let lifecycle = CefLifecyclePolicy::hidden_window();
        assert_eq!(lifecycle.background_frame_rate, 1);
        assert!(lifecycle.suspend_on_blur);
        assert_eq!(lifecycle.hibernate_after, None);
        assert_eq!(lifecycle.hibernate_grace, Duration::from_millis(150));
    }

    #[test]
    fn memory_saver_hidden_window_hibernates_quickly() {
        let lifecycle = CefLifecyclePolicy::memory_saver_hidden_window();
        assert_eq!(lifecycle.background_frame_rate, 1);
        assert!(lifecycle.suspend_on_blur);
        assert_eq!(lifecycle.hibernate_after, Some(Duration::from_secs(5)));
        assert_eq!(lifecycle.hibernate_grace, Duration::from_millis(150));
    }

    #[test]
    fn hidden_builder_uses_hidden_lifecycle_defaults() {
        let window = CefWindow::new().hidden();
        assert!(!window.config.visible);
        assert_eq!(window.config.lifecycle.background_frame_rate, 1);
        assert!(window.config.lifecycle.suspend_on_blur);
        assert_eq!(window.config.lifecycle.hibernate_after, None);
    }
}
