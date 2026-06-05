use std::{
    io::Write,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::Child,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use stuk::prelude::{
    Color, DisplayList, ImageCommand, NumberSpacing, RectCommand, RoundedRectCommand, TextAlign,
    TextCommand, TextWrap,
};
use stuk_platform::{
    WindowBackgroundEffect, WindowChrome, WindowEffect, WindowOptions, WindowRegions,
    request_window_effect,
};
use stuk_render::GpuRenderer;
use winit::{
    application::ApplicationHandler,
    cursor::{Cursor, CursorIcon},
    dpi::{LogicalSize, PhysicalSize},
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::Key,
    window::{ResizeDirection, Window as WinitWindow, WindowAttributes, WindowId, WindowLevel},
};

use crate::{
    osr,
    osr_protocol::{
        MAIN_TEXTURE_ID, OsrFrame, OsrMessage, OsrSurface, POPUP_TEXTURE_ID, encode_component,
        read_message, regions_from_json,
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
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub resizable: bool,
    pub visible: bool,
    pub active: bool,
    pub always_on_top: bool,
    pub transparent: bool,
    pub background_effect: WindowBackgroundEffect,
    pub chrome: WindowChrome,
    pub bridge_commands: Vec<String>,
    pub regions: WindowRegions,
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
            always_on_top: value
                .get("always_on_top")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            transparent: value
                .get("transparent")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            background_effect: value
                .get("background_effect")
                .and_then(serde_json::Value::as_str)
                .and_then(WindowBackgroundEffect::parse)
                .unwrap_or(WindowBackgroundEffect::None),
            chrome: value
                .get("chrome")
                .and_then(serde_json::Value::as_str)
                .and_then(WindowChrome::parse)
                .unwrap_or(WindowChrome::Stuk),
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
    hovered_control: Option<TitlebarControl>,
    pressed_control: Option<TitlebarControl>,
    cursor: CursorIcon,
    modifiers: winit::keyboard::ModifiersState,
    mouse: MouseButtons,
    last_click: Option<ClickMemory>,
    active_click_count: i32,
    cursor_x: f32,
    cursor_y: f32,
    closing_deadline: Option<Instant>,
    presented: bool,
    started: Instant,
}

enum OsrHostEvent {
    Connected(UnixStream),
    Message(OsrMessage),
    Disconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TitlebarControl {
    Minimize,
    Maximize,
    Close,
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
            hovered_control: None,
            pressed_control: None,
            cursor: CursorIcon::Default,
            modifiers: Default::default(),
            mouse: MouseButtons::default(),
            last_click: None,
            active_click_count: 1,
            cursor_x: 0.0,
            cursor_y: 0.0,
            closing_deadline: None,
            presented: false,
            started: Instant::now(),
        }
    }

    fn launch_child(&mut self) {
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
        if !self.config.bridge_commands.is_empty()
            && let Some(child) = self.child.as_mut()
        {
            crate::spawn_native_host_bridge_proxy(child);
        }
    }

    fn content_size_for_cef(&self) -> (u32, u32, f64) {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor());
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
        if uses_stuk_chrome(self.config.chrome) {
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
            chrome: self.config.chrome,
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
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                OsrHostEvent::Connected(stream) => {
                    self.socket = Some(Arc::new(Mutex::new(stream)));
                    self.send_resize();
                }
                OsrHostEvent::Message(OsrMessage::Frame(frame)) => {
                    self.update_frame_texture(frame);
                }
                OsrHostEvent::Message(OsrMessage::PopupHidden) => {
                    self.popup_frame = None;
                }
                OsrHostEvent::Message(OsrMessage::Cursor(cursor)) => {
                    self.set_cursor(cursor_for_cef(&cursor));
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
                OsrHostEvent::Message(OsrMessage::MinimizeRequested) => {
                    if let Some(window) = &self.window {
                        window.set_minimized(true);
                    }
                }
                OsrHostEvent::Message(OsrMessage::ToggleMaximizeRequested) => {
                    if let Some(window) = &self.window {
                        window.set_maximized(!window.is_maximized());
                    }
                }
                OsrHostEvent::Disconnected => {
                    self.socket = None;
                }
            }
        }
        if self.presented
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn update_frame_texture(&mut self, frame: OsrFrame) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let id = match frame.surface {
            OsrSurface::Main => MAIN_TEXTURE_ID,
            OsrSurface::Popup => POPUP_TEXTURE_ID,
        };
        if renderer
            .set_dynamic_bgra_image(id, frame.width, frame.height, &frame.bytes)
            .is_err()
        {
            return;
        }
        let should_present = frame.surface == OsrSurface::Main && !self.presented;
        match frame.surface {
            OsrSurface::Main => {
                self.main_frame = Some(frame);
            }
            OsrSurface::Popup => self.popup_frame = Some(frame),
        }
        if should_present {
            self.render();
            self.present_after_first_frame();
        }
    }

    fn present_after_first_frame(&mut self) {
        if self.presented {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        self.presented = true;
        self.effect = request_window_effect(&window, &self.window_options());
        self.update_effect_regions();
        if self.config.visible {
            window.set_visible(true);
            if self.config.active {
                window.focus_window();
            }
        }
        window.request_redraw();
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
        if !self.config.transparent || uses_stuk_chrome(self.config.chrome) {
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
        let next = titlebar_control_at(width, self.titlebar_height(), self.cursor_x, self.cursor_y);
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

    fn content_position(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let titlebar_height = self.titlebar_height();
        (y >= titlebar_height).then_some((x.max(0.0), (y - titlebar_height).max(0.0)))
    }
}

impl ApplicationHandler for OsrNativeHost {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let defer_visibility = self.config.visible && can_defer_window_visibility();
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
            .with_active(self.config.active && !defer_visibility)
            .with_window_level(if self.config.always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            })
            .with_transparent(self.config.transparent)
            .with_blur(self.config.background_effect.requires_transparency());
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
        if !self.config.visible {
            self.presented = true;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
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
            WindowEvent::CloseRequested | WindowEvent::Destroyed => self.begin_close(event_loop),
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
                self.send_control(if focused { "focus\t1\n" } else { "focus\t0\n" });
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
                    self.set_cursor(CursorIcon::from(direction));
                } else if self.hovered_control.is_some() {
                    self.set_cursor(CursorIcon::Pointer);
                } else if self
                    .content_position(self.cursor_x, self.cursor_y)
                    .is_some()
                {
                    self.forward_mouse_move(false);
                } else {
                    self.set_cursor(CursorIcon::Default);
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
                self.set_cursor(CursorIcon::Default);
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
                        if let Some(control) = titlebar_control_at(
                            self.logical_width(),
                            self.titlebar_height(),
                            self.cursor_x,
                            self.cursor_y,
                        ) {
                            self.pressed_control = Some(control);
                            window.request_redraw();
                            return;
                        }
                        if self.titlebar_height() > 0.0 && self.cursor_y <= self.titlebar_height() {
                            let _ = window.drag_window();
                            return;
                        }
                        self.active_click_count = self.next_click_count(button);
                        self.set_mouse_button(button, true);
                        self.forward_mouse_click(button, false, self.active_click_count);
                    }
                    ElementState::Released => {
                        if let Some(pressed) = self.pressed_control.take() {
                            let released = titlebar_control_at(
                                self.logical_width(),
                                self.titlebar_height(),
                                self.cursor_x,
                                self.cursor_y,
                            );
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
        if self.started.elapsed() > Duration::from_secs(2) && self.child.is_none() {
            event_loop.exit();
        }
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
    std::env::temp_dir().join(format!(
        "stuk-fenestra-osr-{}-{nanos}.sock",
        std::process::id()
    ))
}

fn uses_stuk_chrome(chrome: WindowChrome) -> bool {
    matches!(
        chrome,
        WindowChrome::Stuk | WindowChrome::Compact | WindowChrome::Sidebar
    )
}

fn can_defer_window_visibility() -> bool {
    true
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
        TitlebarControl::Minimize => window.set_minimized(true),
        TitlebarControl::Maximize => window.set_maximized(!window.is_maximized()),
        TitlebarControl::Close => host.begin_close(event_loop),
    }
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
    !matches!(text, "\u{8}" | "\u{7f}" | "\u{1b}")
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
