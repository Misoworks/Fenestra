use std::{
    collections::BTreeSet,
    io::Write,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::Child,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use stuk_platform::{
    WindowBackgroundEffect, WindowChrome as PlatformWindowChrome, WindowEffect, WindowOptions,
    WindowRegionRect, WindowRegions, request_window_effect,
};
use stuk_platform_shell::ShellSurfaceOptions;
use stuk_render::{
    DisplayList, GpuRenderer, ImageCommand, RectCommand, RoundedRectCommand, TextCommand,
};
use stuk_style::{Color, NumberSpacing, TextAlign, TextWrap};
use winit::{
    application::ApplicationHandler,
    cursor::{Cursor, CursorIcon},
    dpi::{LogicalSize, PhysicalSize},
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::Key,
    platform::wayland::WindowAttributesWayland,
    window::{
        ActivationToken, ResizeDirection, UserAttentionType, Window as WinitWindow,
        WindowAttributes, WindowId, WindowLevel,
    },
};

use crate::{
    FenestraLifecyclePolicy, FenestraWindowChrome, FenestraWindowControlRegion, osr,
    osr_frame_buffer::FrameBuffer,
    osr_layer_host,
    osr_protocol::{
        MAIN_TEXTURE_ID, OsrFrame, OsrMessage, OsrPaintBatch, OsrSurface, POPUP_TEXTURE_ID,
        control_regions_from_json, encode_component, lifecycle_from_json, read_message,
        rects_from_json, regions_from_json, shell_surface_from_json,
    },
};

const TITLEBAR_HEIGHT: f32 = 38.0;
const CONTROL_SIZE: f32 = 24.0;
const CONTROL_GAP: f32 = 8.0;
const RESIZE_EDGE: f32 = 7.0;
const CLOSE_GRACE: Duration = Duration::from_millis(300);

const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;
const EVENTFLAG_COMMAND_DOWN: u32 = 1 << 7;
const EVENTFLAG_IS_REPEAT: u32 = 1 << 13;
const EVENTFLAG_PRECISION_SCROLLING_DELTA: u32 = 1 << 14;

pub(crate) fn run(config_path: PathBuf) -> Result<(), String> {
    let config = OsrHostConfig::read(config_path)?;
    if config.shell_surface.is_some() {
        return osr_layer_host::run(config);
    }
    let event_loop = EventLoop::new().map_err(|error| error.to_string())?;
    let proxy = event_loop.create_proxy();
    let (sender, receiver) = mpsc::channel();
    event_loop
        .run_app(OsrNativeHost::new(config, sender, receiver, proxy))
        .map_err(|error| error.to_string())
}

#[derive(Clone, Debug)]
pub(crate) struct OsrHostConfig {
    pub runtime_dir: PathBuf,
    pub host_binary: PathBuf,
    pub url: String,
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
    pub shell_surface: Option<ShellSurfaceOptions>,
    pub background_effect: WindowBackgroundEffect,
    pub chrome: FenestraWindowChrome,
    pub bridge_commands: Vec<String>,
    pub regions: WindowRegions,
    pub drag_regions: Vec<WindowRegionRect>,
    pub drag_exclusion_regions: Vec<WindowRegionRect>,
    pub control_regions: Vec<FenestraWindowControlRegion>,
    pub lifecycle: FenestraLifecyclePolicy,
}

impl OsrHostConfig {
    fn read(config_path: PathBuf) -> Result<Self, String> {
        let text = std::fs::read_to_string(&config_path).map_err(|error| error.to_string())?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| error.to_string())?;
        let _ = std::fs::remove_file(config_path);
        Ok(Self {
            runtime_dir: path_value(&value, "runtime_dir")?,
            host_binary: path_value(&value, "host_binary")?,
            url: string_value(&value, "url")?,
            app_id: value
                .get("app_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            title: value
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Stuk")
                .to_string(),
            width: value
                .get("width")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(900) as u32,
            height: value
                .get("height")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(640) as u32,
            min_width: value
                .get("min_width")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(420) as u32,
            min_height: value
                .get("min_height")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(280) as u32,
            resizable: value
                .get("resizable")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            visible: value
                .get("visible")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            active: value
                .get("active")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            hide_on_blur: value
                .get("hide_on_blur")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            always_on_top: value
                .get("always_on_top")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            transparent: value
                .get("transparent")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            shell_surface: shell_surface_from_json(value.get("shell_surface")),
            background_effect: value
                .get("background_effect")
                .and_then(serde_json::Value::as_str)
                .and_then(WindowBackgroundEffect::parse)
                .unwrap_or(WindowBackgroundEffect::None),
            chrome: value
                .get("chrome")
                .and_then(serde_json::Value::as_str)
                .and_then(FenestraWindowChrome::parse)
                .unwrap_or(FenestraWindowChrome::Frameless),
            bridge_commands: value
                .get("bridge_commands")
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            regions: regions_from_json(value.get("regions")),
            drag_regions: rects_from_json(value.get("drag_regions")),
            drag_exclusion_regions: rects_from_json(value.get("drag_exclusion_regions")),
            control_regions: control_regions_from_json(value.get("control_regions")),
            lifecycle: lifecycle_from_json(value.get("lifecycle")),
        })
    }
}

struct OsrNativeHost {
    config: OsrHostConfig,
    sender: mpsc::Sender<OsrHostEvent>,
    receiver: mpsc::Receiver<OsrHostEvent>,
    proxy: EventLoopProxy,
    window: Option<Arc<dyn WinitWindow>>,
    renderer: Option<GpuRenderer>,
    effect: Option<WindowEffect>,
    child: Option<Child>,
    socket: Option<Arc<Mutex<UnixStream>>>,
    surface_size: PhysicalSize<u32>,
    main_frame: Option<OsrFrame>,
    popup_frame: Option<OsrFrame>,
    main_buffer: FrameBuffer,
    popup_buffer: FrameBuffer,
    hovered_control: Option<TitlebarControl>,
    pressed_control: Option<TitlebarControl>,
    cursor: CursorIcon,
    native_cursor_override: bool,
    modifiers: winit::keyboard::ModifiersState,
    mouse: MouseButtons,
    last_click: Option<ClickMemory>,
    active_click_count: i32,
    cursor_x: f32,
    cursor_y: f32,
    focused: bool,
    occluded: bool,
    lifecycle_state: LifecycleState,
    hibernate_deadline: Option<Instant>,
    hibernate_commit_deadline: Option<Instant>,
    closing_deadline: Option<Instant>,
    activity_hibernation_blockers: BTreeSet<String>,
    presented: bool,
    pending_activation_token: Option<ActivationToken>,
    started: Instant,
}

enum OsrHostEvent {
    Connected(UnixStream),
    Message(OsrMessage),
    HostControl(HostControl),
    Disconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TitlebarControl {
    Minimize,
    Maximize,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifecycleState {
    Active,
    Suspended,
    Hibernating,
    Hibernated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HostControl {
    Show,
    Hide,
    Focus(Option<String>),
    Visible(bool),
    ActivityBegin(HostActivity),
    ActivityEnd(HostActivity),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HostActivity {
    id: String,
    prevents_hibernation: bool,
}

#[derive(Clone, Copy, Debug)]
struct ControlRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl ControlRect {
    fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct MouseButtons {
    left: bool,
    middle: bool,
    right: bool,
}

#[derive(Clone, Copy, Debug)]
struct ClickMemory {
    button: MouseButton,
    x: f32,
    y: f32,
    at: Instant,
    count: i32,
}

impl OsrNativeHost {
    fn new(
        config: OsrHostConfig,
        sender: mpsc::Sender<OsrHostEvent>,
        receiver: mpsc::Receiver<OsrHostEvent>,
        proxy: EventLoopProxy,
    ) -> Self {
        let surface_size = PhysicalSize::new(config.width, config.height);
        let visible = config.visible;
        let focused = visible && config.active;
        let lifecycle_state = if visible {
            LifecycleState::Active
        } else {
            LifecycleState::Suspended
        };
        let hibernate_deadline = if visible {
            None
        } else {
            config
                .lifecycle
                .hibernate_after
                .map(|delay| Instant::now() + delay)
        };
        Self {
            config,
            sender,
            receiver,
            proxy,
            window: None,
            renderer: None,
            effect: None,
            child: None,
            socket: None,
            surface_size,
            main_frame: None,
            popup_frame: None,
            main_buffer: FrameBuffer::new(),
            popup_buffer: FrameBuffer::new(),
            hovered_control: None,
            pressed_control: None,
            cursor: CursorIcon::Default,
            native_cursor_override: false,
            modifiers: Default::default(),
            mouse: MouseButtons::default(),
            last_click: None,
            active_click_count: 1,
            cursor_x: 0.0,
            cursor_y: 0.0,
            focused,
            occluded: false,
            lifecycle_state,
            hibernate_deadline,
            hibernate_commit_deadline: None,
            closing_deadline: None,
            activity_hibernation_blockers: BTreeSet::new(),
            presented: false,
            pending_activation_token: None,
            started: Instant::now(),
        }
    }

    fn launch_child(&mut self) {
        if self.child.is_some() {
            return;
        }
        let socket_path = osr_socket_path();
        let _ = std::fs::remove_file(&socket_path);
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("failed to bind OSR socket: {error}");
                return;
            }
        };
        start_socket_reader(listener, self.sender.clone(), self.proxy.clone());

        let (width, height, scale) = self.content_size_for_cef();
        let mut command = osr::cef_osr_command(
            &self.config.runtime_dir,
            &self.config.host_binary,
            &socket_path,
            &self.config,
            width,
            height,
            scale,
        );
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!("failed to launch CEF OSR child: {error}");
                return;
            }
        };
        self.child = Some(child);
        if let Some(child) = self.child.as_mut() {
            let sender = self.sender.clone();
            let proxy = self.proxy.clone();
            crate::spawn_native_host_bridge_proxy(child, move |command, value| {
                let Some(control) = host_control_from_parts(&command, &value) else {
                    return;
                };
                if sender.send(OsrHostEvent::HostControl(control)).is_ok() {
                    proxy.wake_up();
                }
            });
        }
    }

    fn content_size_for_cef(&self) -> (u32, u32, f64) {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor());
        if !self.config.visible
            && self.window.is_none()
            && self.config.lifecycle.hibernate_after.is_some()
        {
            return (1, 1, scale);
        }
        let logical_width = f64::from(self.surface_size.width) / scale.max(1.0);
        let logical_height = (f64::from(self.surface_size.height) / scale.max(1.0)
            - f64::from(self.titlebar_height()))
        .max(1.0);
        (
            logical_width.round().max(1.0) as u32,
            logical_height.round().max(1.0) as u32,
            scale,
        )
    }

    fn titlebar_height(&self) -> f32 {
        if uses_fenestra_chrome(self.config.chrome) {
            TITLEBAR_HEIGHT
        } else {
            0.0
        }
    }

    fn window_options(&self) -> WindowOptions {
        WindowOptions {
            title: self.config.title.clone(),
            width: self.config.width,
            height: self.config.height,
            min_width: self.config.min_width,
            min_height: self.config.min_height,
            chrome: platform_chrome(self.config.chrome),
            resizable: self.config.resizable,
            visible: self.config.visible,
            active: self.config.active,
            always_on_top: self.config.always_on_top,
            transparent: self.config.transparent,
            background_effect: self.config.background_effect,
            regions: self.config.regions.clone(),
            ..WindowOptions::default()
        }
    }

    fn send_resize(&self) {
        let (width, height, scale) = self.content_size_for_cef();
        self.send_control(&format!("resize\t{width}\t{height}\t{scale:.4}\n"));
    }

    fn send_control(&self, line: &str) {
        let Some(socket) = &self.socket else {
            return;
        };
        if let Ok(mut socket) = socket.lock() {
            let _ = socket.write_all(line.as_bytes());
            let _ = socket.flush();
        }
    }

    fn send_lifecycle(&self, state: LifecycleState, reason: &str) {
        let (name, frame_rate) = match state {
            LifecycleState::Active => ("active", self.config.lifecycle.active_frame_rate.max(1)),
            LifecycleState::Suspended => (
                "suspended",
                self.config.lifecycle.background_frame_rate.max(1),
            ),
            LifecycleState::Hibernating | LifecycleState::Hibernated => (
                "hibernate",
                self.config.lifecycle.background_frame_rate.max(1),
            ),
        };
        self.send_control(&format!(
            "lifecycle\t{name}\t{frame_rate}\t{}\n",
            encode_component(reason)
        ));
        trace_host(
            &self.config,
            format!("lifecycle.{name}.{reason}.fps.{frame_rate}"),
        );
    }

    fn sync_lifecycle(&mut self, reason: &str) {
        if self.closing_deadline.is_some() {
            return;
        }
        let should_suspend = (self.occluded && self.config.lifecycle.suspend_on_occluded)
            || (!self.focused && self.config.lifecycle.suspend_on_blur);
        if should_suspend {
            self.suspend(reason);
        } else {
            self.resume(reason);
        }
    }

    fn suspend(&mut self, reason: &str) {
        if matches!(
            self.lifecycle_state,
            LifecycleState::Suspended | LifecycleState::Hibernating | LifecycleState::Hibernated
        ) {
            return;
        }
        self.lifecycle_state = LifecycleState::Suspended;
        self.hibernate_commit_deadline = None;
        self.schedule_hibernate_deadline();
        self.send_lifecycle(LifecycleState::Suspended, reason);
    }

    fn resume(&mut self, reason: &str) {
        if self.lifecycle_state == LifecycleState::Active {
            return;
        }
        self.lifecycle_state = LifecycleState::Active;
        self.hibernate_deadline = None;
        self.hibernate_commit_deadline = None;
        if self.child.is_none() {
            self.launch_child();
        }
        self.send_lifecycle(LifecycleState::Active, reason);
        if self.config.visible
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn begin_hibernate(&mut self, reason: &str) {
        if self.lifecycle_state != LifecycleState::Suspended
            || self.child.is_none()
            || self.has_hibernation_blockers()
        {
            return;
        }
        self.lifecycle_state = LifecycleState::Hibernating;
        self.hibernate_deadline = None;
        self.hibernate_commit_deadline =
            Some(Instant::now() + self.config.lifecycle.hibernate_grace);
        self.send_lifecycle(LifecycleState::Hibernating, reason);
    }

    fn commit_hibernate(&mut self) {
        if !matches!(self.lifecycle_state, LifecycleState::Hibernating) {
            return;
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.socket = None;
        self.main_frame = None;
        self.popup_frame = None;
        self.main_buffer.release();
        self.popup_buffer.release();
        self.hibernate_commit_deadline = None;
        self.lifecycle_state = LifecycleState::Hibernated;
        if self.config.visible
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn send_current_lifecycle(&self) {
        match self.lifecycle_state {
            LifecycleState::Active => self.send_lifecycle(LifecycleState::Active, "connect"),
            LifecycleState::Suspended => self.send_lifecycle(LifecycleState::Suspended, "connect"),
            LifecycleState::Hibernating | LifecycleState::Hibernated => {}
        }
    }

    fn begin_close(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.closing_deadline.is_some() {
            return;
        }
        if let Some(window) = &self.window {
            window.set_visible(false);
        }
        self.send_control("close\n");
        self.closing_deadline = Some(Instant::now() + CLOSE_GRACE);
        if self.child.is_none() {
            event_loop.exit();
        }
    }

    fn force_close(&mut self, event_loop: &dyn ActiveEventLoop) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        event_loop.exit();
    }

    fn process_osr_events(&mut self, event_loop: &dyn ActiveEventLoop) {
        let mut needs_redraw = false;
        let mut needs_initial_present = false;
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                OsrHostEvent::Connected(stream) => {
                    self.socket = Some(Arc::new(Mutex::new(stream)));
                    self.send_resize();
                    self.send_current_lifecycle();
                }
                OsrHostEvent::Message(OsrMessage::Frame(frame)) => {
                    if self.accepts_paint() {
                        let was_presented = self.presented;
                        needs_redraw |= self.update_frame_texture(frame);
                        needs_initial_present |= !was_presented && self.main_frame.is_some();
                    }
                }
                OsrHostEvent::Message(OsrMessage::PaintBatch(batch)) => {
                    if self.accepts_paint() {
                        let was_presented = self.presented;
                        needs_redraw |= self.update_paint_batch(batch);
                        needs_initial_present |= !was_presented && self.main_frame.is_some();
                    }
                }
                OsrHostEvent::Message(OsrMessage::PopupHidden) => {
                    self.popup_frame = None;
                    self.popup_buffer.release();
                    needs_redraw = true;
                }
                OsrHostEvent::Message(OsrMessage::Cursor(cursor)) => {
                    self.set_content_cursor(cursor_for_cef(&cursor));
                }
                OsrHostEvent::Message(OsrMessage::CloseRequested) => {
                    self.force_close(event_loop);
                    return;
                }
                OsrHostEvent::Message(OsrMessage::StartDragRequested) => {
                    if let Some(window) = &self.window {
                        let _ = window.drag_window();
                    }
                }
                OsrHostEvent::Message(OsrMessage::FileDragRequested(_request)) => {
                    // TODO: implement native file drag-out for OSR windows.
                    // The host needs to use an X11 DnD or Wayland data-device
                    // implementation to start a system drag with the file URIs
                    // so the user can drop files from the app into other apps
                    // (file managers, editors, desktops, etc.). Until that is
                    // wired up, file drag-out silently fails in OSR mode.
                }
                OsrHostEvent::Message(OsrMessage::MinimizeRequested) => {
                    if self.config.lifecycle.suspend_on_minimize {
                        self.suspend("minimize");
                        if self.config.lifecycle.hibernate_after.is_some() {
                            self.begin_hibernate("minimize");
                        }
                    }
                    if let Some(window) = &self.window {
                        window.set_minimized(true);
                    }
                }
                OsrHostEvent::Message(OsrMessage::ToggleMaximizeRequested) => {
                    if let Some(window) = &self.window {
                        window.set_maximized(!window.is_maximized());
                    }
                }
                OsrHostEvent::Message(OsrMessage::ShowRequested) => {
                    self.ensure_window(event_loop);
                    self.show_window("show");
                }
                OsrHostEvent::Message(OsrMessage::HideRequested) => self.hide_window("hide"),
                OsrHostEvent::Message(OsrMessage::FocusRequested) => {
                    self.ensure_window(event_loop);
                    self.focus_window("focus");
                }
                OsrHostEvent::HostControl(HostControl::Show) => {
                    self.ensure_window(event_loop);
                    self.show_window("show");
                }
                OsrHostEvent::HostControl(HostControl::Hide) => self.hide_window("hide"),
                OsrHostEvent::HostControl(HostControl::Focus(token)) => {
                    self.activate_window(event_loop, token);
                }
                OsrHostEvent::HostControl(HostControl::Visible(true)) => {
                    self.ensure_window(event_loop);
                    self.show_window("visible")
                }
                OsrHostEvent::HostControl(HostControl::Visible(false)) => {
                    self.hide_window("hidden")
                }
                OsrHostEvent::HostControl(HostControl::ActivityBegin(activity)) => {
                    self.begin_activity(activity)
                }
                OsrHostEvent::HostControl(HostControl::ActivityEnd(activity)) => {
                    self.end_activity(activity)
                }
                OsrHostEvent::Disconnected => {
                    self.socket = None;
                }
            }
        }
        if needs_initial_present {
            self.render();
            self.present_after_first_frame();
            return;
        }
        if self.config.visible
            && needs_redraw
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn show_window(&mut self, reason: &str) {
        self.config.visible = true;
        if let Some(window) = &self.window {
            if self.presented {
                window.set_visible(true);
                window.set_minimized(false);
                window.request_redraw();
            } else {
                window.set_visible(false);
            }
        }
        self.resume(reason);
        self.send_resize();
    }

    fn accepts_paint(&self) -> bool {
        self.config.visible && self.lifecycle_state == LifecycleState::Active
    }

    fn hide_window(&mut self, reason: &str) {
        self.config.visible = false;
        self.focused = false;
        self.popup_frame = None;
        self.popup_buffer.release();
        self.send_control("focus\t0\n");
        if let Some(window) = &self.window {
            window.set_visible(false);
        }
        self.suspend(reason);
        if self.config.hide_on_blur {
            self.drop_hidden_window();
        }
    }

    fn focus_window(&mut self, reason: &str) {
        self.config.visible = true;
        self.focused = true;
        if let Some(window) = &self.window {
            if self.presented {
                present_window(window);
            } else {
                window.set_visible(false);
            }
        }
        self.send_control("focus\t1\n");
        self.resume(reason);
        self.send_resize();
    }

    fn activate_window(&mut self, event_loop: &dyn ActiveEventLoop, token: Option<String>) {
        if let Some(token) = activation_token_value(token) {
            self.pending_activation_token = Some(ActivationToken::from_raw(token));
            if self.window.is_some() {
                self.drop_presented_window();
            }
        }
        self.ensure_window(event_loop);
        self.focus_window("focus");
    }

    fn update_frame_texture(&mut self, frame: OsrFrame) -> bool {
        let (width, height, _) = self.content_size_for_cef();
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        match frame.surface {
            OsrSurface::Main => {
                let Some(damage) = self.main_buffer.compose(width, height, &frame) else {
                    return false;
                };
                if renderer
                    .update_dynamic_bgra_image_region(
                        MAIN_TEXTURE_ID,
                        width,
                        height,
                        damage.x,
                        damage.y,
                        damage.width,
                        damage.height,
                        self.main_buffer.bytes(),
                    )
                    .is_err()
                {
                    return false;
                }
                self.main_frame = Some(OsrFrame {
                    surface: OsrSurface::Main,
                    width,
                    height,
                    x: 0,
                    y: 0,
                    bytes: Vec::new(),
                });
            }
            OsrSurface::Popup => {
                let Some(damage) = self.popup_buffer.compose(
                    frame.width,
                    frame.height,
                    &popup_local_frame(&frame),
                ) else {
                    return false;
                };
                if renderer
                    .update_dynamic_bgra_image_region(
                        POPUP_TEXTURE_ID,
                        frame.width,
                        frame.height,
                        damage.x,
                        damage.y,
                        damage.width,
                        damage.height,
                        self.popup_buffer.bytes(),
                    )
                    .is_err()
                {
                    return false;
                }
                self.popup_frame = Some(OsrFrame {
                    surface: OsrSurface::Popup,
                    width: frame.width,
                    height: frame.height,
                    x: frame.x,
                    y: frame.y,
                    bytes: Vec::new(),
                });
            }
        }
        true
    }

    fn update_paint_batch(&mut self, batch: OsrPaintBatch) -> bool {
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        if batch.frames.is_empty() {
            return false;
        }
        match batch.surface {
            OsrSurface::Main => {
                let Some(damage) =
                    self.main_buffer
                        .compose_batch(batch.width, batch.height, &batch.frames)
                else {
                    return false;
                };
                if renderer
                    .update_dynamic_bgra_image_region(
                        MAIN_TEXTURE_ID,
                        batch.width,
                        batch.height,
                        damage.x,
                        damage.y,
                        damage.width,
                        damage.height,
                        self.main_buffer.bytes(),
                    )
                    .is_err()
                {
                    return false;
                }
                self.main_frame = Some(OsrFrame {
                    surface: OsrSurface::Main,
                    width: batch.width,
                    height: batch.height,
                    x: 0,
                    y: 0,
                    bytes: Vec::new(),
                });
            }
            OsrSurface::Popup => {
                let Some(damage) =
                    self.popup_buffer
                        .compose_batch(batch.width, batch.height, &batch.frames)
                else {
                    return false;
                };
                if renderer
                    .update_dynamic_bgra_image_region(
                        POPUP_TEXTURE_ID,
                        batch.width,
                        batch.height,
                        damage.x,
                        damage.y,
                        damage.width,
                        damage.height,
                        self.popup_buffer.bytes(),
                    )
                    .is_err()
                {
                    return false;
                }
                self.popup_frame = Some(OsrFrame {
                    surface: OsrSurface::Popup,
                    width: batch.width,
                    height: batch.height,
                    x: batch.x,
                    y: batch.y,
                    bytes: Vec::new(),
                });
            }
        }
        true
    }

    fn present_after_first_frame(&mut self) {
        if self.presented {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        self.presented = true;
        trace_host(&self.config, "first_paint");
        self.effect = request_window_effect(&window, &self.window_options());
        self.update_effect_regions();
        if self.config.visible {
            window.set_visible(true);
            window.set_minimized(false);
            if self.config.active || self.focused {
                present_window(&window);
            }
        }
        if self.config.visible {
            window.request_redraw();
        }
    }

    fn render(&mut self) {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor()) as f32;
        let width = self.surface_size.width as f32 / scale.max(1.0);
        let height = self.surface_size.height as f32 / scale.max(1.0);
        let list = self.display_list(width.max(1.0), height.max(1.0));
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if let Err(error) = renderer.render(&list) {
            eprintln!("webview OSR render failed: {error}");
        }
    }

    fn display_list(&self, width: f32, height: f32) -> DisplayList {
        let ready = self.main_frame.is_some();
        let background = if !ready || self.config.transparent {
            Color::rgba(0.0, 0.0, 0.0, 0.0)
        } else {
            Color::WINDOW
        };
        let mut list = DisplayList::new(background);
        if !ready {
            return list;
        }
        if !self.config.transparent || uses_fenestra_chrome(self.config.chrome) {
            let radius = if self.config.chrome.uses_native_decorations() {
                0.0
            } else {
                12.0
            };
            list.push(RoundedRectCommand {
                x: 0.0,
                y: 0.0,
                width,
                height,
                radius,
                color: Color::rgba(
                    0.08,
                    0.08,
                    0.08,
                    if self.config.transparent { 0.38 } else { 1.0 },
                ),
            });
        }
        self.draw_titlebar(&mut list, width);
        let y = self.titlebar_height();
        if self.main_frame.is_some() {
            list.push(ImageCommand {
                id: MAIN_TEXTURE_ID.to_string(),
                x: 0.0,
                y,
                width,
                height: (height - y).max(1.0),
                opacity: 1.0,
            });
        }
        if let Some(popup) = &self.popup_frame {
            list.push(ImageCommand {
                id: POPUP_TEXTURE_ID.to_string(),
                x: popup.x as f32,
                y: y + popup.y as f32,
                width: popup.width as f32,
                height: popup.height as f32,
                opacity: 1.0,
            });
        }
        list
    }

    fn draw_titlebar(&self, list: &mut DisplayList, width: f32) {
        let titlebar_height = self.titlebar_height();
        if titlebar_height == 0.0 {
            return;
        }
        let titlebar_color = Color::rgba(0.07, 0.07, 0.075, 0.58);
        let titlebar_radius = 12.0;
        list.push(RoundedRectCommand {
            x: 0.0,
            y: 0.0,
            width,
            height: titlebar_height,
            radius: titlebar_radius,
            color: titlebar_color,
        });
        list.push(RectCommand {
            x: 0.0,
            y: (titlebar_height - titlebar_radius).max(0.0),
            width,
            height: titlebar_radius.min(titlebar_height),
            color: titlebar_color,
        });
        list.push(RectCommand {
            x: 0.0,
            y: titlebar_height - 1.0,
            width,
            height: 1.0,
            color: Color::WHITE.opacity(0.10),
        });
        list.push(TextCommand {
            text: self.config.title.clone(),
            x: 0.0,
            y: 8.0,
            width,
            height: 22.0,
            size: 14.0,
            line_height: 20.0,
            color: Color::TEXT,
            wrap: TextWrap::Pretty,
            align: TextAlign::Center,
            number_spacing: NumberSpacing::Proportional,
        });
        for control in [
            TitlebarControl::Minimize,
            TitlebarControl::Maximize,
            TitlebarControl::Close,
        ] {
            draw_control(
                list,
                control_rect(width, titlebar_height, control),
                control,
                self.hovered_control == Some(control),
                self.pressed_control == Some(control),
            );
        }
    }

    fn update_titlebar_hover(&mut self) {
        let width = self.logical_width();
        let next = self.control_at(width, self.cursor_x, self.cursor_y);
        self.hovered_control = next;
    }

    fn logical_width(&self) -> f32 {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor()) as f32;
        self.surface_size.width as f32 / scale.max(1.0)
    }

    fn logical_height(&self) -> f32 {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor()) as f32;
        self.surface_size.height as f32 / scale.max(1.0)
    }

    fn set_cursor(&mut self, cursor: CursorIcon) {
        if self.cursor == cursor {
            return;
        }
        self.cursor = cursor;
        if let Some(window) = &self.window {
            window.set_cursor(Cursor::Icon(cursor));
        }
    }

    fn set_native_cursor(&mut self, cursor: CursorIcon) {
        self.native_cursor_override = true;
        self.set_cursor(cursor);
    }

    fn set_content_cursor(&mut self, cursor: CursorIcon) {
        self.native_cursor_override = false;
        self.set_cursor(cursor);
    }

    fn clear_native_cursor(&mut self) {
        if !self.native_cursor_override {
            return;
        }
        self.native_cursor_override = false;
        self.set_cursor(CursorIcon::Default);
    }

    fn content_position(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let titlebar_height = self.titlebar_height();
        (y >= titlebar_height).then_some((x.max(0.0), (y - titlebar_height).max(0.0)))
    }

    fn control_at(&self, width: f32, x: f32, y: f32) -> Option<TitlebarControl> {
        if let Some(control) = configured_control_at(&self.config.control_regions, width, x, y) {
            return Some(control);
        }
        titlebar_control_at(width, self.titlebar_height(), x, y)
    }

    fn is_drag_region(&self, width: f32, x: f32, y: f32) -> bool {
        if configured_region_at(&self.config.drag_exclusion_regions, width, x, y) {
            return false;
        }
        if !self.config.drag_regions.is_empty() {
            return configured_region_at(&self.config.drag_regions, width, x, y);
        }
        self.titlebar_height() > 0.0 && y <= self.titlebar_height()
    }

    fn ensure_window(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_none() {
            self.create_window(event_loop);
        }
    }

    fn create_window(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let activating = self.pending_activation_token.is_some();
        let defer_visibility = self.config.visible && can_defer_window_visibility() && !activating;
        let mut attributes = WindowAttributes::default()
            .with_title(self.config.title.clone())
            .with_surface_size(LogicalSize::new(
                f64::from(self.config.width),
                f64::from(self.config.height),
            ))
            .with_min_surface_size(LogicalSize::new(
                f64::from(self.config.min_width),
                f64::from(self.config.min_height),
            ))
            .with_resizable(self.config.resizable)
            .with_decorations(self.config.chrome.uses_native_decorations())
            .with_visible(self.config.visible && !defer_visibility)
            .with_active((self.config.active || activating) && !defer_visibility)
            .with_window_level(if self.config.always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            })
            .with_transparent(self.config.transparent)
            .with_blur(self.config.background_effect.requires_transparency());
        let mut wayland_attributes = WindowAttributesWayland::default();
        let mut has_wayland_attributes = false;
        if let Some(app_id) = &self.config.app_id {
            wayland_attributes = wayland_attributes.with_name(app_id, app_id);
            has_wayland_attributes = true;
        }
        if let Some(token) = self.pending_activation_token.take() {
            wayland_attributes = wayland_attributes.with_activation_token(token);
            has_wayland_attributes = true;
        }
        if has_wayland_attributes {
            attributes = attributes.with_platform_attributes(Box::new(wayland_attributes));
        }
        if let Some(position) =
            crate::centered_window_position(event_loop, self.config.width, self.config.height)
        {
            attributes = attributes.with_position(position);
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::<dyn WinitWindow>::from(window),
            Err(error) => {
                eprintln!("failed to create webview OSR host window: {error}");
                event_loop.exit();
                return;
            }
        };
        self.surface_size = window.surface_size();
        let renderer = match pollster::block_on(GpuRenderer::new(window.clone())) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("failed to initialize webview OSR renderer: {error}");
                event_loop.exit();
                return;
            }
        };
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.launch_child();
        self.upload_cached_textures();
        if self.main_frame.is_some() {
            self.present_after_first_frame();
        }
        if self.config.visible
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn drop_hidden_window(&mut self) {
        self.drop_presented_window();
        self.main_frame = None;
        self.popup_frame = None;
        self.main_buffer.release();
        self.popup_buffer.release();
    }

    fn drop_presented_window(&mut self) {
        if let Some(window) = &self.window {
            let scale = window.scale_factor().max(1.0);
            self.config.width = (f64::from(self.surface_size.width) / scale)
                .round()
                .max(f64::from(self.config.min_width)) as u32;
            self.config.height = (f64::from(self.surface_size.height) / scale)
                .round()
                .max(f64::from(self.config.min_height)) as u32;
        }
        self.window = None;
        self.renderer = None;
        self.effect = None;
        self.presented = false;
        self.hovered_control = None;
        self.pressed_control = None;
        self.cursor = CursorIcon::Default;
        self.native_cursor_override = false;
    }

    fn upload_cached_textures(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if let Some(frame) = &self.main_frame {
            let _ = renderer.update_dynamic_bgra_image_region(
                MAIN_TEXTURE_ID,
                frame.width,
                frame.height,
                0,
                0,
                frame.width,
                frame.height,
                self.main_buffer.bytes(),
            );
        }
        if let Some(frame) = &self.popup_frame {
            let _ = renderer.update_dynamic_bgra_image_region(
                POPUP_TEXTURE_ID,
                frame.width,
                frame.height,
                0,
                0,
                frame.width,
                frame.height,
                self.popup_buffer.bytes(),
            );
        }
    }
}

impl ApplicationHandler for OsrNativeHost {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if !self.config.visible {
            self.launch_child();
            return;
        }
        self.create_window(event_loop);
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.process_osr_events(event_loop);
    }

    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if id != window.id() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => self.begin_close(event_loop),
            WindowEvent::Destroyed if self.config.visible || self.closing_deadline.is_some() => {
                self.begin_close(event_loop)
            }
            WindowEvent::Destroyed => self.drop_hidden_window(),
            WindowEvent::SurfaceResized(size) => {
                self.surface_size = size;
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height, window.scale_factor() as f32);
                }
                self.update_effect_regions();
                self.send_resize();
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = window.surface_size();
                self.surface_size = size;
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height, scale_factor as f32);
                }
                self.update_effect_regions();
                self.send_resize();
                window.request_redraw();
            }
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                self.send_control(if focused { "focus\t1\n" } else { "focus\t0\n" });
                if !focused && self.config.hide_on_blur && self.config.visible {
                    self.hide_window("blur");
                } else {
                    self.sync_lifecycle(if focused { "focus" } else { "blur" });
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                self.sync_lifecycle(if occluded { "occluded" } else { "visible" });
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                self.send_key_event(&event);
            }
            WindowEvent::RedrawRequested if self.config.visible && self.presented => self.render(),
            WindowEvent::RedrawRequested => {}
            WindowEvent::PointerMoved {
                position, primary, ..
            } if primary => {
                let scale = window.scale_factor() as f32;
                self.cursor_x = position.x as f32 / scale.max(1.0);
                self.cursor_y = position.y as f32 / scale.max(1.0);
                self.update_titlebar_hover();
                if let Some(direction) = resize_direction_at(
                    self.cursor_x,
                    self.cursor_y,
                    self.logical_width(),
                    self.logical_height(),
                ) {
                    self.set_native_cursor(CursorIcon::from(direction));
                } else if self.hovered_control.is_some() {
                    self.set_native_cursor(CursorIcon::Pointer);
                    self.forward_mouse_move(false);
                } else if self
                    .content_position(self.cursor_x, self.cursor_y)
                    .is_some()
                {
                    self.clear_native_cursor();
                    self.forward_mouse_move(false);
                } else {
                    self.set_native_cursor(CursorIcon::Default);
                }
                window.request_redraw();
            }
            WindowEvent::PointerLeft {
                position, primary, ..
            } if primary => {
                if let Some(position) = position {
                    let scale = window.scale_factor() as f32;
                    self.cursor_x = position.x as f32 / scale.max(1.0);
                    self.cursor_y = position.y as f32 / scale.max(1.0);
                }
                self.hovered_control = None;
                self.forward_mouse_move(true);
                self.set_native_cursor(CursorIcon::Default);
                window.request_redraw();
            }
            WindowEvent::PointerButton {
                state,
                primary,
                position,
                button,
                ..
            } if primary => {
                let scale = window.scale_factor() as f32;
                self.cursor_x = position.x as f32 / scale.max(1.0);
                self.cursor_y = position.y as f32 / scale.max(1.0);
                let button = button.clone().mouse_button();
                match state {
                    ElementState::Pressed => {
                        if matches!(button, Some(MouseButton::Back | MouseButton::Forward)) {
                            return;
                        }
                        if let Some(direction) = resize_direction_at(
                            self.cursor_x,
                            self.cursor_y,
                            self.logical_width(),
                            self.logical_height(),
                        ) {
                            let _ = window.drag_resize_window(direction);
                            return;
                        }
                        let width = self.logical_width();
                        if let Some(control) = self.control_at(width, self.cursor_x, self.cursor_y)
                        {
                            self.pressed_control = Some(control);
                            window.request_redraw();
                            return;
                        }
                        if self.is_drag_region(width, self.cursor_x, self.cursor_y) {
                            let _ = window.drag_window();
                            return;
                        }
                        self.active_click_count = self.next_click_count(button);
                        self.set_mouse_button(button, true);
                        self.forward_mouse_click(button, false, self.active_click_count);
                    }
                    ElementState::Released => {
                        if let Some(pressed) = self.pressed_control.take() {
                            let released =
                                self.control_at(self.logical_width(), self.cursor_x, self.cursor_y);
                            if released == Some(pressed) {
                                activate_control(self, event_loop, &window, pressed);
                            }
                            window.request_redraw();
                            return;
                        }
                        if matches!(button, Some(MouseButton::Back | MouseButton::Forward)) {
                            self.forward_navigation_button(button);
                            return;
                        }
                        self.set_mouse_button(button, false);
                        self.forward_mouse_click(button, true, self.active_click_count);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.forward_mouse_wheel(delta);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        if let Some(child) = self.child.as_mut()
            && matches!(child.try_wait(), Ok(Some(_)))
        {
            self.child = None;
            self.socket = None;
            if matches!(
                self.lifecycle_state,
                LifecycleState::Hibernating | LifecycleState::Hibernated
            ) {
                self.lifecycle_state = LifecycleState::Hibernated;
                return;
            }
            event_loop.exit();
            return;
        }
        if let Some(deadline) = self.closing_deadline {
            if Instant::now() >= deadline {
                self.force_close(event_loop);
                return;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            return;
        }
        if let Some(deadline) = self.hibernate_commit_deadline {
            if Instant::now() >= deadline {
                self.commit_hibernate();
                return;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            return;
        }
        if let Some(deadline) = self.hibernate_deadline {
            if self.has_hibernation_blockers() {
                self.hibernate_deadline = None;
                return;
            }
            if Instant::now() >= deadline {
                self.begin_hibernate("idle");
                return;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            return;
        }
        if self.started.elapsed() > Duration::from_secs(2)
            && self.child.is_none()
            && self.lifecycle_state != LifecycleState::Hibernated
        {
            event_loop.exit();
        }
    }
}

impl OsrNativeHost {
    fn begin_activity(&mut self, activity: HostActivity) {
        if !activity.prevents_hibernation {
            return;
        }
        self.activity_hibernation_blockers.insert(activity.id);
        self.hibernate_deadline = None;
        if self.lifecycle_state == LifecycleState::Hibernating {
            self.lifecycle_state = LifecycleState::Suspended;
            self.hibernate_commit_deadline = None;
            self.send_lifecycle(LifecycleState::Suspended, "activity");
        }
    }

    fn end_activity(&mut self, activity: HostActivity) {
        if !activity.prevents_hibernation {
            return;
        }
        self.activity_hibernation_blockers.remove(&activity.id);
        if self.lifecycle_state == LifecycleState::Suspended {
            self.schedule_hibernate_deadline();
        }
    }

    fn has_hibernation_blockers(&self) -> bool {
        !self.activity_hibernation_blockers.is_empty()
    }

    fn schedule_hibernate_deadline(&mut self) {
        self.hibernate_deadline = if self.has_hibernation_blockers() {
            None
        } else {
            self.config
                .lifecycle
                .hibernate_after
                .map(|delay| Instant::now() + delay)
        };
    }
}

impl OsrNativeHost {
    fn update_effect_regions(&self) {
        let Some(effect) = &self.effect else {
            return;
        };
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor());
        let width = (f64::from(self.surface_size.width) / scale.max(1.0)).round() as i32;
        let height = (f64::from(self.surface_size.height) / scale.max(1.0)).round() as i32;
        let _ = effect.update(&self.window_options(), width.max(1), height.max(1));
    }

    fn forward_mouse_move(&self, leave: bool) {
        if let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) {
            self.send_control(&format!(
                "mouse_move\t{:.2}\t{:.2}\t{}\t{}\n",
                x,
                y,
                self.cef_modifiers(),
                i32::from(leave)
            ));
        }
    }

    fn forward_mouse_click(&self, button: Option<MouseButton>, up: bool, click_count: i32) {
        let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) else {
            return;
        };
        let Some(button) = cef_mouse_button(button) else {
            return;
        };
        self.send_control(&format!(
            "mouse_click\t{:.2}\t{:.2}\t{}\t{}\t{}\t{}\n",
            x,
            y,
            button,
            self.cef_modifiers(),
            i32::from(up),
            click_count.max(1)
        ));
    }

    fn forward_navigation_button(&self, button: Option<MouseButton>) {
        let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) else {
            return;
        };
        let button = match button {
            Some(MouseButton::Back) => 3,
            Some(MouseButton::Forward) => 4,
            _ => return,
        };
        self.send_control(&format!(
            "mouse_navigation\t{:.2}\t{:.2}\t{}\t{}\n",
            x,
            y,
            button,
            self.cef_modifiers()
        ));
    }

    fn forward_mouse_wheel(&self, delta: MouseScrollDelta) {
        let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) else {
            return;
        };
        let (dx, dy, precision) = match delta {
            MouseScrollDelta::LineDelta(x, y) => ((x * 120.0) as i32, (y * 120.0) as i32, false),
            MouseScrollDelta::PixelDelta(position) => (position.x as i32, position.y as i32, true),
        };
        self.send_control(&format!(
            "mouse_wheel\t{:.2}\t{:.2}\t{}\t{}\t{}\n",
            x,
            y,
            dx,
            dy,
            self.cef_modifiers()
                | if precision {
                    EVENTFLAG_PRECISION_SCROLLING_DELTA
                } else {
                    0
                }
        ));
    }

    fn send_key_event(&self, event: &KeyEvent) {
        let pressed = event.state == ElementState::Pressed;
        let text = if pressed {
            event
                .text
                .as_deref()
                .filter(|text| should_send_char_text(text))
                .unwrap_or("")
        } else {
            ""
        };
        self.send_control(&format!(
            "key\t{}\t{}\t{}\t{}\t{}\n",
            i32::from(pressed),
            encode_component(&key_name(event)),
            encode_component(text),
            self.cef_modifiers() | if event.repeat { EVENTFLAG_IS_REPEAT } else { 0 },
            i32::from(event.repeat)
        ));
    }

    fn cef_modifiers(&self) -> u32 {
        let mut modifiers = 0;
        if self.modifiers.shift_key() {
            modifiers |= EVENTFLAG_SHIFT_DOWN;
        }
        if self.modifiers.control_key() {
            modifiers |= EVENTFLAG_CONTROL_DOWN;
        }
        if self.modifiers.alt_key() {
            modifiers |= EVENTFLAG_ALT_DOWN;
        }
        if self.modifiers.meta_key() {
            modifiers |= EVENTFLAG_COMMAND_DOWN;
        }
        if self.mouse.left {
            modifiers |= EVENTFLAG_LEFT_MOUSE_BUTTON;
        }
        if self.mouse.middle {
            modifiers |= EVENTFLAG_MIDDLE_MOUSE_BUTTON;
        }
        if self.mouse.right {
            modifiers |= EVENTFLAG_RIGHT_MOUSE_BUTTON;
        }
        modifiers
    }

    fn set_mouse_button(&mut self, button: Option<MouseButton>, pressed: bool) {
        match button {
            Some(MouseButton::Left) => self.mouse.left = pressed,
            Some(MouseButton::Middle) => self.mouse.middle = pressed,
            Some(MouseButton::Right) => self.mouse.right = pressed,
            _ => {}
        }
    }

    fn next_click_count(&mut self, button: Option<MouseButton>) -> i32 {
        let Some(button) = button else {
            return 1;
        };
        let now = Instant::now();
        let count = self
            .last_click
            .filter(|last| {
                last.button == button
                    && now.duration_since(last.at) <= Duration::from_millis(500)
                    && (last.x - self.cursor_x).abs() <= 4.0
                    && (last.y - self.cursor_y).abs() <= 4.0
            })
            .map(|last| (last.count + 1).min(3))
            .unwrap_or(1);
        self.last_click = Some(ClickMemory {
            button,
            x: self.cursor_x,
            y: self.cursor_y,
            at: now,
            count,
        });
        count
    }
}

fn start_socket_reader(
    listener: UnixListener,
    sender: mpsc::Sender<OsrHostEvent>,
    proxy: EventLoopProxy,
) {
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        if let Ok(writer) = stream.try_clone() {
            let _ = sender.send(OsrHostEvent::Connected(writer));
            proxy.wake_up();
        }
        loop {
            match read_message(&mut stream) {
                Ok(Some(message)) => {
                    if sender.send(OsrHostEvent::Message(message)).is_err() {
                        break;
                    }
                    proxy.wake_up();
                }
                Ok(None) => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                    ) =>
                {
                    break;
                }
                Err(error) => {
                    eprintln!("webview OSR socket read failed: {error}");
                    break;
                }
            }
        }
        let _ = sender.send(OsrHostEvent::Disconnected);
        proxy.wake_up();
    });
}

fn host_control_from_parts(command: &str, value: &str) -> Option<HostControl> {
    match command {
        "visible" => bool_control_value(value).map(HostControl::Visible),
        "show" => Some(HostControl::Show),
        "hide" => Some(HostControl::Hide),
        "focus" => Some(HostControl::Focus(activation_token_value(Some(
            value.to_string(),
        )))),
        "activity.begin" => activity_control_value(value).map(HostControl::ActivityBegin),
        "activity.end" => activity_control_value(value).map(HostControl::ActivityEnd),
        _ => None,
    }
}

fn bool_control_value(value: &str) -> Option<bool> {
    match value {
        "1" | "true" | "yes" | "show" | "visible" => Some(true),
        "0" | "false" | "no" | "hide" | "hidden" => Some(false),
        _ => None,
    }
}

fn activation_token_value(token: Option<String>) -> Option<String> {
    token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .filter(|token| bool_control_value(token).is_none())
}

fn activity_control_value(value: &str) -> Option<HostActivity> {
    let value = serde_json::from_str::<serde_json::Value>(value).ok()?;
    Some(HostActivity {
        id: value.get("id")?.as_str()?.to_string(),
        prevents_hibernation: value
            .get("preventsHibernation")
            .or_else(|| value.get("prevents_hibernation"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    })
}

fn path_value(value: &serde_json::Value, key: &str) -> Result<PathBuf, String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| format!("OSR host config missing {key}"))
}

fn string_value(value: &serde_json::Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| format!("OSR host config missing {key}"))
}

fn osr_socket_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("fenestra-osr-{}-{nanos}.sock", std::process::id()))
}

fn popup_local_frame(frame: &OsrFrame) -> OsrFrame {
    OsrFrame {
        surface: OsrSurface::Popup,
        width: frame.width,
        height: frame.height,
        x: 0,
        y: 0,
        bytes: frame.bytes.clone(),
    }
}

fn uses_fenestra_chrome(chrome: FenestraWindowChrome) -> bool {
    matches!(chrome, FenestraWindowChrome::Fenestra)
}

fn trace_host(config: &OsrHostConfig, stage: impl AsRef<str>) {
    let enabled = std::env::var(crate::FENESTRA_TRACE_ENV).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "trace"
        )
    });
    if !enabled {
        return;
    }
    let label = config.app_id.as_deref().unwrap_or(&config.title);
    eprintln!(
        "fenestra trace [{label}] osr-host pid={} {}",
        std::process::id(),
        stage.as_ref()
    );
}

fn can_defer_window_visibility() -> bool {
    true
}

fn platform_chrome(chrome: FenestraWindowChrome) -> PlatformWindowChrome {
    match chrome {
        FenestraWindowChrome::System => PlatformWindowChrome::System,
        FenestraWindowChrome::Fenestra => PlatformWindowChrome::Stuk,
        FenestraWindowChrome::Frameless | FenestraWindowChrome::None => PlatformWindowChrome::None,
    }
}

fn configured_control_at(
    controls: &[FenestraWindowControlRegion],
    width: f32,
    x: f32,
    y: f32,
) -> Option<TitlebarControl> {
    controls.iter().find_map(|region| {
        rect_region_contains(&region.rect, width, x, y).then(|| match region.action {
            crate::FenestraWindowControlAction::Minimize => TitlebarControl::Minimize,
            crate::FenestraWindowControlAction::Maximize => TitlebarControl::Maximize,
            crate::FenestraWindowControlAction::Close => TitlebarControl::Close,
        })
    })
}

fn configured_region_at(regions: &[WindowRegionRect], width: f32, x: f32, y: f32) -> bool {
    regions
        .iter()
        .any(|region| rect_region_contains(region, width, x, y))
}

fn rect_region_contains(region: &WindowRegionRect, width: f32, x: f32, y: f32) -> bool {
    let region_x = if region.x < 0 {
        width + region.x as f32
    } else {
        region.x as f32
    };
    let region_width = if region.width == i32::MAX {
        width - region_x
    } else {
        region.width as f32
    };
    let rect = ControlRect::new(
        region_x,
        region.y as f32,
        region_width.max(0.0),
        region.height as f32,
    );
    rect_contains(rect, x, y)
}

fn control_rect(width: f32, titlebar_height: f32, control: TitlebarControl) -> ControlRect {
    let right = width - 12.0;
    let y = (titlebar_height - CONTROL_SIZE) * 0.5;
    let index = match control {
        TitlebarControl::Close => 0.0,
        TitlebarControl::Maximize => 1.0,
        TitlebarControl::Minimize => 2.0,
    };
    ControlRect::new(
        right - CONTROL_SIZE * (index + 1.0) - CONTROL_GAP * index,
        y,
        CONTROL_SIZE,
        CONTROL_SIZE,
    )
}

fn titlebar_control_at(
    width: f32,
    titlebar_height: f32,
    x: f32,
    y: f32,
) -> Option<TitlebarControl> {
    if titlebar_height == 0.0 || y < 0.0 || y > titlebar_height {
        return None;
    }
    [
        TitlebarControl::Minimize,
        TitlebarControl::Maximize,
        TitlebarControl::Close,
    ]
    .into_iter()
    .find(|control| rect_contains(control_rect(width, titlebar_height, *control), x, y))
}

fn draw_control(
    list: &mut DisplayList,
    rect: ControlRect,
    control: TitlebarControl,
    hovered: bool,
    pressed: bool,
) {
    let fill_alpha = if pressed {
        0.24
    } else if hovered {
        0.15
    } else {
        0.10
    };
    list.push(RoundedRectCommand {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        radius: 999.0,
        color: Color::TEXT.opacity(fill_alpha),
    });
    let icon = Color::TEXT.opacity(if hovered || pressed { 0.95 } else { 0.68 });
    match control {
        TitlebarControl::Minimize => list.push(RectCommand {
            x: rect.x + (rect.width - 9.0) * 0.5,
            y: rect.y + rect.height * 0.5 - 0.75,
            width: 9.0,
            height: 1.5,
            color: icon,
        }),
        TitlebarControl::Maximize => draw_maximize(list, rect, icon),
        TitlebarControl::Close => draw_close(list, rect, icon),
    }
}

fn draw_maximize(list: &mut DisplayList, rect: ControlRect, color: Color) {
    let x = rect.x + (rect.width - 9.0) * 0.5;
    let y = rect.y + (rect.height - 9.0) * 0.5;
    for command in [
        RectCommand {
            x,
            y,
            width: 9.0,
            height: 1.5,
            color,
        },
        RectCommand {
            x,
            y: y + 7.5,
            width: 9.0,
            height: 1.5,
            color,
        },
        RectCommand {
            x,
            y,
            width: 1.5,
            height: 9.0,
            color,
        },
        RectCommand {
            x: x + 7.5,
            y,
            width: 1.5,
            height: 9.0,
            color,
        },
    ] {
        list.push(command);
    }
}

fn draw_close(list: &mut DisplayList, rect: ControlRect, color: Color) {
    let center_x = rect.x + rect.width * 0.5;
    let center_y = rect.y + rect.height * 0.5;
    for (dx, dy) in [
        (-4.0, -4.0),
        (-2.0, -2.0),
        (0.0, 0.0),
        (2.0, 2.0),
        (4.0, 4.0),
        (-4.0, 4.0),
        (-2.0, 2.0),
        (2.0, -2.0),
        (4.0, -4.0),
    ] {
        list.push(RectCommand {
            x: center_x + dx - 0.9,
            y: center_y + dy - 0.9,
            width: 1.8,
            height: 1.8,
            color,
        });
    }
}

fn rect_contains(rect: ControlRect, x: f32, y: f32) -> bool {
    x >= rect.x && x <= rect.x + rect.width && y >= rect.y && y <= rect.y + rect.height
}

fn resize_direction_at(x: f32, y: f32, width: f32, height: f32) -> Option<ResizeDirection> {
    let left = x <= RESIZE_EDGE;
    let right = x >= width - RESIZE_EDGE;
    let top = y <= RESIZE_EDGE;
    let bottom = y >= height - RESIZE_EDGE;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (_, true, true, _) => Some(ResizeDirection::NorthEast),
        (true, _, _, true) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, _, _, _) => Some(ResizeDirection::West),
        (_, true, _, _) => Some(ResizeDirection::East),
        (_, _, true, _) => Some(ResizeDirection::North),
        (_, _, _, true) => Some(ResizeDirection::South),
        _ => None,
    }
}

fn activate_control(
    host: &mut OsrNativeHost,
    event_loop: &dyn ActiveEventLoop,
    window: &Arc<dyn WinitWindow>,
    control: TitlebarControl,
) {
    match control {
        TitlebarControl::Minimize => {
            if host.config.lifecycle.suspend_on_minimize {
                host.suspend("minimize");
            }
            window.set_minimized(true);
        }
        TitlebarControl::Maximize => window.set_maximized(!window.is_maximized()),
        TitlebarControl::Close => host.begin_close(event_loop),
    }
}

fn present_window(window: &Arc<dyn WinitWindow>) {
    window.set_visible(true);
    window.set_minimized(false);
    window.request_user_attention(Some(UserAttentionType::Informational));
    window.focus_window();
    window.request_redraw();
}

fn cef_mouse_button(button: Option<MouseButton>) -> Option<&'static str> {
    match button {
        Some(MouseButton::Left) => Some("left"),
        Some(MouseButton::Middle) => Some("middle"),
        Some(MouseButton::Right) => Some("right"),
        _ => None,
    }
}

fn key_name(event: &KeyEvent) -> String {
    match event.logical_key.as_ref() {
        Key::Character(value) if !value.is_empty() => value.to_string(),
        Key::Named(named) => named.to_string(),
        _ => match &event.physical_key {
            winit::keyboard::PhysicalKey::Code(code) => format!("{code:?}"),
            _ => "Unidentified".to_string(),
        },
    }
}

fn should_send_char_text(text: &str) -> bool {
    !text.chars().any(char::is_control)
}

fn cursor_for_cef(cursor: &str) -> CursorIcon {
    match cursor {
        "pointer" | "hand" => CursorIcon::Pointer,
        "text" | "vertical-text" => CursorIcon::Text,
        "crosshair" => CursorIcon::Crosshair,
        "move" => CursorIcon::Move,
        "wait" => CursorIcon::Wait,
        "help" => CursorIcon::Help,
        "not-allowed" => CursorIcon::NotAllowed,
        "col-resize" | "ew-resize" => CursorIcon::EwResize,
        "row-resize" | "ns-resize" => CursorIcon::NsResize,
        "ne-resize" => CursorIcon::NeResize,
        "nw-resize" => CursorIcon::NwResize,
        "se-resize" => CursorIcon::SeResize,
        "sw-resize" => CursorIcon::SwResize,
        _ => CursorIcon::Default,
    }
}
