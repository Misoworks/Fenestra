use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::{fd::AsFd, unix::net::UnixStream},
    process::Child,
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use fenestra_platform::{
    ShellSurfaceKeyboardInteractivity, WindowBackgroundEffect, WindowOptions, WindowRegion,
    WindowRegions, request_surface_effect,
};
use layershellev::{
    DispatchMessage, LayerShellEvent, NewPopUpSettings, ReturnData, WindowState,
    calloop::channel::{self, Sender},
    id,
    reexport::wl_shm::{self, WlShm},
};
use wayland_client::{QueueHandle, protocol::wl_buffer::WlBuffer};

use crate::{
    osr,
    osr_frame_buffer::buffer_len,
    osr_host::OsrHostConfig,
    osr_protocol::{OsrFrame, OsrMessage, OsrPaintBatch, OsrSurface},
};

mod alpha;
mod buffer;
mod forward;
mod input;
mod shell;
mod socket;

use alpha::LayerAlphaModifier;
use buffer::{DamageRect, paint_buffer_file, paint_frames_buffer_file};
use input::{axis_delta, cursor_shape_for_wayland};
use shell::{anchor_for_shell, keyboard_for_shell, layer_for_shell};
use socket::{LayerHostEvent, open_socket_reader, spawn_layer_bridge_proxy};

pub(crate) fn run(config: OsrHostConfig) -> Result<(), String> {
    let shell_surface = config
        .shell_surface
        .clone()
        .ok_or_else(|| "missing Fenestra shell surface options".to_string())?;
    let mut window_state = WindowState::new(&shell_surface.namespace)
        .with_option_size(shell_surface.size)
        .with_layer(layer_for_shell(shell_surface.layer))
        .with_anchor(anchor_for_shell(shell_surface.anchor))
        .with_margin((
            shell_surface.margin.top,
            shell_surface.margin.right,
            shell_surface.margin.bottom,
            shell_surface.margin.left,
        ))
        .with_keyboard_interacivity(keyboard_for_shell(shell_surface.keyboard_interactivity))
        .with_events_transparent(shell_surface.events_transparent);
    if let Some(exclusive_zone) = shell_surface.exclusive_zone {
        window_state = window_state.with_exclusive_zone(exclusive_zone);
    }
    let window_state: WindowState<()> = window_state.build().map_err(|error| error.to_string())?;

    let (sender, receiver) = channel::channel();
    let mut host = OsrLayerHost::new(config, sender);
    window_state
        .running_with_proxy(receiver, move |event, state, id| {
            host.handle(event, state, id)
        })
        .map_err(|error| error.to_string())
}

struct OsrLayerHost {
    config: OsrHostConfig,
    sender: Sender<LayerHostEvent>,
    child: Option<Child>,
    socket: Option<Arc<Mutex<UnixStream>>>,
    buffer_file: Option<File>,
    shm: Option<WlShm>,
    queue_handle: Option<QueueHandle<WindowState<()>>>,
    wayland_buffer: Option<WlBuffer>,
    buffer_size: (u32, u32),
    surface_size: (u32, u32),
    scale: f64,
    main_frame: Option<OsrFrame>,
    main_frame_surface_size: Option<(u32, u32)>,
    popup: Option<PopupSurface>,
    main_buffer: Vec<u8>,
    scratch: Vec<u8>,
    surface_mapped: bool,
    visible: bool,
    cursor_shape: String,
    cursor_x: f32,
    cursor_y: f32,
    pointer_inside: bool,
    modifiers: layershellev::keyboard::ModifiersState,
    mouse: MouseButtons,
    last_click: Option<ClickMemory>,
    active_click_count: i32,
    focused: bool,
    lifecycle_state: LayerLifecycleState,
    alpha_modifier: Option<LayerAlphaModifier>,
    surface_alpha: f32,
}

struct PopupSurface {
    id: id::Id,
    position: (i32, i32),
    size: (u32, u32),
    frame: Option<OsrFrame>,
    pending_frames: Vec<OsrFrame>,
    buffer_file: Option<File>,
    wayland_buffer: Option<WlBuffer>,
    buffer: Vec<u8>,
    scratch: Vec<u8>,
    mapped: bool,
    effect: Option<fenestra_platform::WindowEffect>,
}

#[derive(Clone, Copy, Debug, Default)]
struct MouseButtons {
    left: bool,
    middle: bool,
    right: bool,
}

#[derive(Clone, Copy, Debug)]
struct ClickMemory {
    button: u32,
    x: f32,
    y: f32,
    at: Instant,
    count: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayerLifecycleState {
    Active,
    Suspended,
}

impl OsrLayerHost {
    fn new(config: OsrHostConfig, sender: Sender<LayerHostEvent>) -> Self {
        let surface_size = (config.width.max(1), config.height.max(1));
        let visible = config.visible;
        let surface_alpha = if visible {
            config.shell_surface_alpha.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let focused = config.active;
        let lifecycle_state = if visible {
            LayerLifecycleState::Active
        } else {
            LayerLifecycleState::Suspended
        };
        Self {
            config,
            sender,
            child: None,
            socket: None,
            buffer_file: None,
            shm: None,
            queue_handle: None,
            wayland_buffer: None,
            buffer_size: surface_size,
            surface_size,
            scale: 1.0,
            main_frame: None,
            main_frame_surface_size: None,
            popup: None,
            main_buffer: Vec::new(),
            scratch: Vec::new(),
            surface_mapped: false,
            visible,
            cursor_shape: "default".to_string(),
            cursor_x: 0.0,
            cursor_y: 0.0,
            pointer_inside: false,
            modifiers: Default::default(),
            mouse: MouseButtons::default(),
            last_click: None,
            active_click_count: 1,
            focused,
            lifecycle_state,
            alpha_modifier: None,
            surface_alpha,
        }
    }

    fn handle(
        &mut self,
        event: LayerShellEvent<(), LayerHostEvent>,
        state: &mut WindowState<()>,
        id: Option<layershellev::id::Id>,
    ) -> ReturnData<()> {
        match event {
            LayerShellEvent::RequestBuffer(file, shm, qh, width, height) => {
                let width = width.max(1);
                let height = height.max(1);
                if self
                    .popup
                    .as_ref()
                    .is_some_and(|popup| Some(popup.id) == id)
                {
                    let buffer = self.install_popup_buffer(file, shm, qh, width, height);
                    self.ensure_popup_effect(state);
                    self.commit_popup_surface(state, DamageRect::full(width, height));
                    return ReturnData::WlBuffer(buffer);
                }
                self.shm = Some(shm.clone());
                self.queue_handle = Some(qh.clone());
                let size_changed = self.surface_size != (width, height);
                self.surface_size = (width, height);
                if size_changed {
                    self.clear_frames();
                }
                ReturnData::WlBuffer(self.install_wayland_buffer(file, shm, qh, width, height))
            }
            LayerShellEvent::RequestMessages(message) => self.handle_message(message, state, id),
            LayerShellEvent::UserEvent(event) => self.handle_host_event(event, state, id),
            _ => ReturnData::None,
        }
    }

    fn handle_message(
        &mut self,
        message: &DispatchMessage,
        state: &mut WindowState<()>,
        id: Option<layershellev::id::Id>,
    ) -> ReturnData<()> {
        match message {
            DispatchMessage::RequestRefresh {
                width,
                height,
                scale_float,
                ..
            } => {
                if self
                    .popup
                    .as_ref()
                    .is_some_and(|popup| Some(popup.id) == id)
                {
                    if let Some(popup) = self.popup.as_mut() {
                        popup.size = ((*width).max(1), (*height).max(1));
                        popup.mapped = false;
                    }
                    self.ensure_popup_effect(state);
                    self.refresh_popup_surface(state);
                    return ReturnData::None;
                }
                let surface_size = ((*width).max(1), (*height).max(1));
                let size_changed = self.surface_size != surface_size;
                self.surface_size = surface_size;
                self.scale = scale_float.max(1.0);
                if size_changed {
                    self.recreate_wayland_buffer(surface_size.0, surface_size.1);
                }
                self.ensure_child();
                self.send_resize();
                if self.visible && self.main_frame_ready() {
                    self.refresh_surface(state, id);
                } else {
                    self.hide_surface(state);
                }
            }
            DispatchMessage::Focused(_) if self.visible => {
                self.focused = true;
                self.send_control("focus\t1\n");
                self.resume("focus");
            }
            DispatchMessage::Focused(_) => {
                self.focused = false;
                self.send_control("focus\t0\n");
                self.suspend("hidden");
            }
            DispatchMessage::Unfocus => {
                self.focused = false;
                self.send_control("focus\t0\n");
                if !self.visible {
                    self.suspend("hidden");
                }
            }
            DispatchMessage::ModifiersChanged(modifiers) => {
                self.modifiers = *modifiers;
            }
            DispatchMessage::KeyboardInput {
                event,
                is_synthetic: false,
            } if self.visible => self.send_key_event(event),
            DispatchMessage::MouseEnter {
                pointer,
                surface_x,
                surface_y,
                ..
            } if self.visible => {
                self.pointer_inside = true;
                (self.cursor_x, self.cursor_y) =
                    self.pointer_position_for_unit(id, *surface_x, *surface_y);
                self.forward_mouse_move(false);
                return ReturnData::RequestSetCursorShape((
                    cursor_shape_for_wayland(&self.cursor_shape).to_string(),
                    pointer.clone(),
                ));
            }
            DispatchMessage::MouseMotion {
                surface_x,
                surface_y,
                ..
            } if self.visible => {
                self.pointer_inside = true;
                (self.cursor_x, self.cursor_y) =
                    self.pointer_position_for_unit(id, *surface_x, *surface_y);
                self.forward_mouse_move(false);
            }
            DispatchMessage::MouseLeave if self.visible => {
                self.pointer_inside = false;
                self.forward_mouse_move(true);
            }
            DispatchMessage::MouseButton { state, button, .. } if self.visible => {
                self.forward_mouse_button(*button, state);
            }
            DispatchMessage::Axis {
                horizontal,
                vertical,
                ..
            } if self.visible => {
                self.forward_mouse_wheel(axis_delta(horizontal), axis_delta(vertical))
            }
            DispatchMessage::Closed => {
                if self
                    .popup
                    .as_ref()
                    .is_some_and(|popup| Some(popup.id) == id)
                {
                    self.popup = None;
                    return ReturnData::None;
                }
                self.begin_close();
                return ReturnData::RequestExit;
            }
            _ => {}
        }
        ReturnData::None
    }

    fn handle_host_event(
        &mut self,
        event: LayerHostEvent,
        state: &mut WindowState<()>,
        id: Option<layershellev::id::Id>,
    ) -> ReturnData<()> {
        match event {
            LayerHostEvent::Connected(stream) => {
                self.socket = Some(Arc::new(Mutex::new(stream)));
                self.set_surface_alpha(self.surface_alpha, state);
                self.send_resize();
                self.force_current_lifecycle("connect");
                if !self.visible {
                    self.hide_surface(state);
                }
            }
            LayerHostEvent::Message(OsrMessage::Frame(frame)) => {
                if self.visible {
                    match frame.surface {
                        OsrSurface::Main => {
                            let frame_size = (frame.width, frame.height);
                            if self.main_frame_surface_size != Some(frame_size) {
                                self.close_popup(state);
                            }
                            self.main_frame_surface_size = Some(frame_size);
                            self.main_frame = Some(frame);
                        }
                        OsrSurface::Popup => {
                            if let Some(return_data) = self.update_popup_frame(frame, state, id) {
                                return return_data;
                            }
                        }
                    }
                    if self.main_frame_ready() {
                        self.restore_keyboard(state);
                        self.force_resume("first-paint");
                        self.refresh_surface(state, id);
                    } else {
                        self.hide_surface(state);
                    }
                }
            }
            LayerHostEvent::Message(OsrMessage::PaintBatch(batch)) => {
                if self.visible {
                    if let Some(return_data) = self.refresh_batch_surface(batch, state, id) {
                        return return_data;
                    }
                }
            }
            LayerHostEvent::Message(OsrMessage::PopupHidden) => {
                self.close_popup(state);
            }
            LayerHostEvent::Message(OsrMessage::Cursor(cursor)) => {
                self.cursor_shape = cursor;
            }
            LayerHostEvent::Message(OsrMessage::CloseRequested) => {
                return ReturnData::RequestExit;
            }
            LayerHostEvent::Message(OsrMessage::StartDragRequested) => {}
            LayerHostEvent::Message(OsrMessage::FileDragRequested(_)) => {
                // TODO: implement native file drag-out using the layer-shell
                // DnD protocol or an X11/Wayland data-device backend.
            }
            LayerHostEvent::Message(OsrMessage::MinimizeRequested) => {}
            LayerHostEvent::Message(OsrMessage::ToggleMaximizeRequested) => {}
            LayerHostEvent::Message(OsrMessage::ShowRequested) => {
                self.set_surface_visible(true, state)
            }
            LayerHostEvent::Message(OsrMessage::HideRequested) => {
                self.set_surface_visible(false, state)
            }
            LayerHostEvent::Message(OsrMessage::FocusRequested) => {
                self.set_surface_visible(true, state)
            }
            LayerHostEvent::Visible(visible) => self.set_surface_visible(visible, state),
            LayerHostEvent::Alpha(alpha) => self.set_surface_alpha(alpha, state),
            LayerHostEvent::Margin(margin) => self.set_surface_margin(margin, state),
            LayerHostEvent::Disconnected => {
                self.socket = None;
                return ReturnData::RequestExit;
            }
        }
        ReturnData::None
    }

    fn ensure_child(&mut self) {
        if self.child.is_some() {
            return;
        }
        let Some(socket_path) = open_socket_reader(self.sender.clone()) else {
            return;
        };

        let (width, height, scale) = self.content_size_for_cef();
        let mut command = osr::cef_osr_command(
            &self.config.runtime_dir,
            &self.config.host_binary,
            &socket_path,
            &self.config,
            width,
            height,
            scale,
            self.active_frame_rate(),
        );
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!("failed to launch Fenestra layer OSR child: {error}");
                return;
            }
        };
        self.child = Some(child);
        if !self.config.bridge_commands.is_empty()
            && let Some(child) = self.child.as_mut()
        {
            spawn_layer_bridge_proxy(child, self.sender.clone());
        }
    }

    fn install_wayland_buffer(
        &mut self,
        file: &mut File,
        shm: &WlShm,
        qh: &QueueHandle<WindowState<()>>,
        width: u32,
        height: u32,
    ) -> WlBuffer {
        if self.buffer_size != (width, height) {
            self.main_buffer.clear();
            self.scratch.clear();
        }
        self.buffer_size = (width, height);
        self.surface_mapped = false;
        if let Ok(clone) = file.try_clone() {
            self.buffer_file = Some(clone);
        }
        let byte_len = buffer_len(width, height);
        if paint_buffer_file(
            file,
            width,
            height,
            self.main_frame.as_ref(),
            None,
            &mut self.main_buffer,
            &mut self.scratch,
        )
        .is_err()
        {
            let _ = file.set_len(byte_len as u64);
        }
        let pool = shm.create_pool(file.as_fd(), byte_len as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            (width * 4) as i32,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );
        self.wayland_buffer = Some(buffer.clone());
        buffer
    }

    fn install_popup_buffer(
        &mut self,
        file: &mut File,
        shm: &WlShm,
        qh: &QueueHandle<WindowState<()>>,
        width: u32,
        height: u32,
    ) -> WlBuffer {
        let Some(popup) = self.popup.as_mut() else {
            return create_buffer(file, shm, qh, width, height);
        };
        popup.size = (width, height);
        popup.mapped = false;
        if let Ok(clone) = file.try_clone() {
            popup.buffer_file = Some(clone);
        }
        let byte_len = buffer_len(width, height);
        let paint_result = if popup.pending_frames.is_empty() {
            paint_buffer_file(
                file,
                width,
                height,
                popup.frame.as_ref(),
                None,
                &mut popup.buffer,
                &mut popup.scratch,
            )
        } else {
            let frames = popup.pending_frames.iter().collect::<Vec<_>>();
            paint_frames_buffer_file(
                file,
                width,
                height,
                &frames,
                &[],
                &mut popup.buffer,
                &mut popup.scratch,
            )
        };
        popup.pending_frames.clear();
        if paint_result.is_err() {
            let _ = file.set_len(byte_len as u64);
        }
        let buffer = create_buffer(file, shm, qh, width, height);
        popup.wayland_buffer = Some(buffer.clone());
        buffer
    }

    fn recreate_wayland_buffer(&mut self, width: u32, height: u32) {
        let (Some(shm), Some(qh)) = (self.shm.clone(), self.queue_handle.clone()) else {
            return;
        };
        let Ok(mut file) = temporary_buffer_file() else {
            return;
        };
        self.clear_frames();
        self.install_wayland_buffer(&mut file, &shm, &qh, width.max(1), height.max(1));
    }

    fn content_size_for_cef(&self) -> (u32, u32, f64) {
        (
            self.surface_size.0.max(1),
            self.surface_size.1.max(1),
            self.scale.max(1.0),
        )
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

    fn send_lifecycle(&self, state: LayerLifecycleState, reason: &str) {
        let (name, frame_rate) = match state {
            LayerLifecycleState::Active => ("active", self.active_frame_rate()),
            LayerLifecycleState::Suspended => (
                "suspended",
                self.config.lifecycle.background_frame_rate.max(1),
            ),
        };
        self.send_control(&format!(
            "lifecycle\t{name}\t{frame_rate}\t{}\n",
            crate::osr_protocol::encode_component(reason)
        ));
    }

    fn active_frame_rate(&self) -> u32 {
        if self.config.lifecycle.active_frame_rate > 0 {
            self.config.lifecycle.active_frame_rate
        } else {
            60
        }
    }

    fn suspend(&mut self, reason: &str) {
        if self.lifecycle_state == LayerLifecycleState::Suspended {
            return;
        }
        self.force_suspend(reason);
    }

    fn resume(&mut self, reason: &str) {
        if self.lifecycle_state == LayerLifecycleState::Active {
            return;
        }
        self.force_resume(reason);
    }

    fn force_suspend(&mut self, reason: &str) {
        self.lifecycle_state = LayerLifecycleState::Suspended;
        self.send_lifecycle(LayerLifecycleState::Suspended, reason);
    }

    fn force_resume(&mut self, reason: &str) {
        self.lifecycle_state = LayerLifecycleState::Active;
        self.send_lifecycle(LayerLifecycleState::Active, reason);
    }

    fn force_current_lifecycle(&mut self, reason: &str) {
        match self.lifecycle_state {
            LayerLifecycleState::Active => self.force_resume(reason),
            LayerLifecycleState::Suspended => self.force_suspend(reason),
        }
    }

    fn set_surface_visible(&mut self, visible: bool, state: &mut WindowState<()>) {
        self.visible = visible;
        if visible {
            self.show_surface(state);
        } else {
            self.hide_shell_surface(state);
        }
    }

    fn show_surface(&mut self, state: &mut WindowState<()>) {
        self.restore_keyboard(state);
        self.force_resume("visible");
        self.send_resize();
        if self.pointer_inside {
            self.forward_mouse_move(false);
        }
        if self.main_frame_ready() {
            self.refresh_surface(state, None);
        } else {
            self.hide_surface(state);
        }
    }

    fn hide_shell_surface(&mut self, state: &mut WindowState<()>) {
        self.close_popup(state);
        if self.pointer_inside {
            self.forward_mouse_move(true);
            self.pointer_inside = false;
        }
        self.send_control("focus\t0\n");
        self.force_suspend("hidden");
        self.send_resize();
        self.set_surface_alpha(0.0, state);
        self.hide_surface(state);
        if !self.config.lifecycle.retain_hidden_frame {
            self.release_hidden_frame_memory();
        }
    }

    fn set_surface_alpha(&mut self, alpha: f32, state: &WindowState<()>) {
        let alpha = alpha.clamp(0.0, 1.0);
        if self.alpha_modifier.is_some() && (self.surface_alpha - alpha).abs() <= 0.001 {
            return;
        }
        if self.alpha_modifier.is_none() {
            self.alpha_modifier = LayerAlphaModifier::bind(state);
        }
        self.surface_alpha = alpha;
        if let Some(modifier) = &self.alpha_modifier {
            let _ = modifier.set_alpha(alpha);
        }
    }

    fn set_surface_margin(
        &mut self,
        margin: fenestra_platform::ShellSurfaceMargin,
        state: &WindowState<()>,
    ) {
        let Some(shell_surface) = self.config.shell_surface.as_mut() else {
            return;
        };
        if shell_surface.margin == margin {
            return;
        }
        shell_surface.margin = margin;
        state
            .main_window()
            .set_margin((margin.top, margin.right, margin.bottom, margin.left));
    }

    fn hide_surface(&mut self, state: &mut WindowState<()>) {
        let unit = state.main_window();
        unit.set_keyboard_interactivity(keyboard_for_shell(
            ShellSurfaceKeyboardInteractivity::None,
        ));
        unit.get_wlsurface().attach(None, 0, 0);
        unit.get_wlsurface().commit();
        self.surface_mapped = false;
    }

    fn restore_keyboard(&self, state: &mut WindowState<()>) {
        let Some(shell_surface) = self.config.shell_surface.as_ref() else {
            return;
        };
        state
            .main_window()
            .set_keyboard_interactivity(keyboard_for_shell(shell_surface.keyboard_interactivity));
    }

    fn begin_close(&mut self) {
        self.send_control("close\n");
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn refresh_surface(&mut self, state: &mut WindowState<()>, id: Option<layershellev::id::Id>) {
        if !self.visible || !self.main_frame_ready() {
            return;
        }
        let Some(file) = &self.buffer_file else {
            return;
        };
        let Ok(mut file) = file.try_clone() else {
            return;
        };
        let damage = match paint_buffer_file(
            &mut file,
            self.buffer_size.0,
            self.buffer_size.1,
            self.main_frame.as_ref(),
            None,
            &mut self.main_buffer,
            &mut self.scratch,
        ) {
            Ok(damage) => damage,
            Err(_) => return,
        };
        if let Some(id) = id {
            if let Some(unit) = state.get_unit_with_id(id) {
                self.commit_surface(unit, damage);
                return;
            }
        }
        self.commit_surface(state.main_window(), damage);
    }

    fn refresh_batch_surface(
        &mut self,
        batch: OsrPaintBatch,
        state: &mut WindowState<()>,
        id: Option<layershellev::id::Id>,
    ) -> Option<ReturnData<()>> {
        if batch.surface == OsrSurface::Main && (batch.width, batch.height) != self.surface_size {
            self.main_frame = None;
            self.main_frame_surface_size = Some((batch.width, batch.height));
            self.close_popup(state);
            self.hide_surface(state);
            return None;
        }
        if batch.surface == OsrSurface::Popup {
            return self.update_popup_batch(batch, state, id);
        }
        let Some(file) = &self.buffer_file else {
            return None;
        };
        let Ok(mut file) = file.try_clone() else {
            return None;
        };
        let main_frames = batch.frames.iter().collect::<Vec<_>>();
        let damage = match paint_frames_buffer_file(
            &mut file,
            self.buffer_size.0,
            self.buffer_size.1,
            &main_frames,
            &[],
            &mut self.main_buffer,
            &mut self.scratch,
        ) {
            Ok(damage) => damage,
            Err(_) => return None,
        };
        match batch.surface {
            OsrSurface::Main => {
                self.main_frame = batch.frames.last().cloned();
                self.main_frame_surface_size = Some((batch.width, batch.height));
            }
            OsrSurface::Popup => {}
        }
        if !self.main_frame_ready() {
            return None;
        }
        self.restore_keyboard(state);
        self.force_resume("first-paint");
        if let Some(id) = id
            && let Some(unit) = state.get_unit_with_id(id)
        {
            self.commit_surface(unit, damage);
            return None;
        }
        self.commit_surface(state.main_window(), damage);
        None
    }

    fn update_popup_frame(
        &mut self,
        frame: OsrFrame,
        state: &mut WindowState<()>,
        parent_id: Option<id::Id>,
    ) -> Option<ReturnData<()>> {
        if self.main_frame.is_none() {
            return None;
        }
        let position = (frame.x, frame.y);
        let size = (frame.width.max(1), frame.height.max(1));
        let local_frame = local_popup_frame(frame);
        if self
            .popup
            .as_ref()
            .is_none_or(|popup| popup.position != position || popup.size != size)
        {
            return Some(self.create_popup_surface(position, size, local_frame, state, parent_id));
        }
        if let Some(popup) = self.popup.as_mut() {
            popup.frame = Some(local_frame);
        }
        self.refresh_popup_surface(state);
        None
    }

    fn update_popup_batch(
        &mut self,
        batch: OsrPaintBatch,
        state: &mut WindowState<()>,
        parent_id: Option<id::Id>,
    ) -> Option<ReturnData<()>> {
        if self.main_frame.is_none() {
            return None;
        }
        let position = (batch.x, batch.y);
        let size = (batch.width.max(1), batch.height.max(1));
        if self
            .popup
            .as_ref()
            .is_none_or(|popup| popup.position != position || popup.size != size)
        {
            let local_frames = batch
                .frames
                .iter()
                .cloned()
                .map(local_popup_frame)
                .collect::<Vec<_>>();
            let frame = local_frames
                .last()
                .cloned()
                .unwrap_or_else(|| empty_popup_frame(size));
            let return_data = self.create_popup_surface(position, size, frame, state, parent_id);
            if let Some(popup) = self.popup.as_mut() {
                popup.pending_frames = local_frames;
                popup.frame = popup.pending_frames.last().cloned();
            }
            return Some(return_data);
        }
        self.paint_popup_batch(&batch, state);
        None
    }

    fn create_popup_surface(
        &mut self,
        position: (i32, i32),
        size: (u32, u32),
        frame: OsrFrame,
        state: &mut WindowState<()>,
        parent_id: Option<id::Id>,
    ) -> ReturnData<()> {
        self.close_popup(state);
        let parent_id = parent_id.unwrap_or_else(|| state.main_window().id());
        let mut popup_id = id::Id::unique();
        if popup_id == parent_id {
            popup_id = id::Id::unique();
        }
        self.popup = Some(PopupSurface {
            id: popup_id,
            position,
            size,
            frame: Some(frame),
            pending_frames: Vec::new(),
            buffer_file: None,
            wayland_buffer: None,
            buffer: Vec::new(),
            scratch: Vec::new(),
            mapped: false,
            effect: None,
        });
        ReturnData::NewPopUp((
            NewPopUpSettings {
                size,
                position,
                id: parent_id,
            },
            popup_id,
            None,
        ))
    }

    fn refresh_popup_surface(&mut self, state: &mut WindowState<()>) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let Some(file) = &popup.buffer_file else {
            return;
        };
        let Ok(mut file) = file.try_clone() else {
            return;
        };
        let damage = match paint_buffer_file(
            &mut file,
            popup.size.0,
            popup.size.1,
            popup.frame.as_ref(),
            None,
            &mut popup.buffer,
            &mut popup.scratch,
        ) {
            Ok(damage) => damage,
            Err(_) => return,
        };
        self.commit_popup_surface(state, damage);
    }

    fn paint_popup_batch(&mut self, batch: &OsrPaintBatch, state: &mut WindowState<()>) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let Some(file) = &popup.buffer_file else {
            if let Some(frame) = batch.frames.last().cloned() {
                popup.frame = Some(frame);
            }
            return;
        };
        let Ok(mut file) = file.try_clone() else {
            return;
        };
        let local_frames = batch
            .frames
            .iter()
            .cloned()
            .map(local_popup_frame)
            .collect::<Vec<_>>();
        let frames = local_frames.iter().collect::<Vec<_>>();
        let damage = match paint_frames_buffer_file(
            &mut file,
            popup.size.0,
            popup.size.1,
            &frames,
            &[],
            &mut popup.buffer,
            &mut popup.scratch,
        ) {
            Ok(damage) => damage,
            Err(_) => return,
        };
        popup.frame = local_frames.last().cloned();
        self.commit_popup_surface(state, damage);
    }

    fn commit_popup_surface(&mut self, state: &mut WindowState<()>, damage: DamageRect) {
        self.ensure_popup_effect(state);
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let Some(unit) = state.get_unit_with_id(popup.id) else {
            return;
        };
        let Some(buffer) = popup.wayland_buffer.as_ref() else {
            unit.refresh();
            return;
        };
        let damage = if popup.mapped {
            damage
        } else {
            DamageRect::full(popup.size.0, popup.size.1)
        };
        let surface = unit.get_wlsurface();
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(
            damage.x as i32,
            damage.y as i32,
            damage.width as i32,
            damage.height as i32,
        );
        surface.commit();
        popup.mapped = true;
    }

    fn ensure_popup_effect(&mut self, state: &WindowState<()>) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        if popup.effect.is_some() {
            return;
        }
        let Some(unit) = state.get_unit_with_id(popup.id) else {
            return;
        };
        let options = popup_effect_options(popup.size);
        popup.effect =
            request_surface_effect(unit, &options, popup.size.0 as i32, popup.size.1 as i32);
    }

    fn close_popup(&mut self, state: &mut WindowState<()>) {
        if let Some(popup) = self.popup.take() {
            state.request_close(popup.id);
        }
    }

    fn pointer_position_for_unit(
        &self,
        id: Option<id::Id>,
        surface_x: f64,
        surface_y: f64,
    ) -> (f32, f32) {
        if let Some(popup) = &self.popup
            && Some(popup.id) == id
        {
            return (
                surface_x as f32 + popup.position.0 as f32,
                surface_y as f32 + popup.position.1 as f32,
            );
        }
        (surface_x as f32, surface_y as f32)
    }

    fn main_frame_ready(&self) -> bool {
        self.main_frame.is_some() && self.main_frame_surface_size == Some(self.surface_size)
    }

    fn clear_frames(&mut self) {
        self.main_frame = None;
        self.main_frame_surface_size = None;
        self.popup = None;
    }

    fn release_hidden_frame_memory(&mut self) {
        self.clear_frames();
        self.main_buffer = Vec::new();
        self.scratch = Vec::new();
    }

    fn commit_surface(&mut self, unit: &layershellev::WindowStateUnit<()>, damage: DamageRect) {
        let Some(buffer) = self.wayland_buffer.as_ref() else {
            unit.refresh();
            self.surface_mapped = true;
            return;
        };
        let damage = if self.surface_mapped {
            damage
        } else {
            DamageRect::full(self.buffer_size.0, self.buffer_size.1)
        };
        let surface = unit.get_wlsurface();
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(
            damage.x as i32,
            damage.y as i32,
            damage.width as i32,
            damage.height as i32,
        );
        surface.commit();
        self.surface_mapped = true;
    }
}

fn create_buffer(
    file: &mut File,
    shm: &WlShm,
    qh: &QueueHandle<WindowState<()>>,
    width: u32,
    height: u32,
) -> WlBuffer {
    let byte_len = buffer_len(width, height);
    let pool = shm.create_pool(file.as_fd(), byte_len as i32, qh, ());
    pool.create_buffer(
        0,
        width as i32,
        height as i32,
        (width * 4) as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    )
}

fn local_popup_frame(mut frame: OsrFrame) -> OsrFrame {
    frame.x = 0;
    frame.y = 0;
    frame
}

fn empty_popup_frame(size: (u32, u32)) -> OsrFrame {
    OsrFrame {
        surface: OsrSurface::Popup,
        width: size.0,
        height: size.1,
        x: 0,
        y: 0,
        bytes: vec![0; buffer_len(size.0, size.1)],
    }
}

fn popup_effect_options(size: (u32, u32)) -> WindowOptions {
    WindowOptions {
        width: size.0,
        height: size.1,
        transparent: true,
        background_effect: WindowBackgroundEffect::Blur,
        regions: WindowRegions::new().blur(WindowRegion::adaptive_full()),
        ..WindowOptions::default()
    }
}

fn temporary_buffer_file() -> std::io::Result<File> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!(
        "fenestra-layer-buffer-{}-{nanos}.shm",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    let _ = std::fs::remove_file(path);
    Ok(file)
}

impl Drop for OsrLayerHost {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
