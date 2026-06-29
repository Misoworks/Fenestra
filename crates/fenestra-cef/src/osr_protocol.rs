use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::net::UnixStream,
    },
};

use fenestra_platform::{
    ShellSurfaceAnchor, ShellSurfaceKeyboardInteractivity, ShellSurfaceLayer, ShellSurfaceMargin,
    ShellSurfaceOptions,
};
use fenestra_platform::{WindowRegion, WindowRegionAdaptive, WindowRegionRect, WindowRegions};
use serde_json::Value;

use crate::{FenestraLifecyclePolicy, FenestraWindowControlAction, FenestraWindowControlRegion};

use std::time::Duration;

const HEADER_LEN: usize = 28;
const MAGIC: &[u8; 4] = b"SKOR";
const KIND_MAIN_FRAME: u32 = 1;
const KIND_POPUP_FRAME: u32 = 2;
const KIND_POPUP_HIDDEN: u32 = 3;
const KIND_CURSOR: u32 = 4;
const KIND_CLOSE_REQUESTED: u32 = 5;
const KIND_START_DRAG_REQUESTED: u32 = 6;
const KIND_MINIMIZE_REQUESTED: u32 = 7;
const KIND_TOGGLE_MAXIMIZE_REQUESTED: u32 = 8;
const KIND_SHOW_REQUESTED: u32 = 9;
const KIND_HIDE_REQUESTED: u32 = 10;
const KIND_FOCUS_REQUESTED: u32 = 11;
const KIND_MAIN_BATCH: u32 = 12;
const KIND_POPUP_BATCH: u32 = 13;
const KIND_MAIN_SHARED_BATCH: u32 = 14;
const KIND_POPUP_SHARED_BATCH: u32 = 15;
const KIND_FILE_DRAG_REQUESTED: u32 = 16;
const BATCH_ENTRY_LEN: usize = 28;

pub(crate) const MAIN_TEXTURE_ID: &str = "__stuk_fenestra_main";
pub(crate) const POPUP_TEXTURE_ID: &str = "__stuk_fenestra_popup";

#[derive(Debug)]
pub(crate) enum OsrMessage {
    Frame(OsrFrame),
    PaintBatch(OsrPaintBatch),
    PopupHidden,
    Cursor(String),
    CloseRequested,
    StartDragRequested,
    MinimizeRequested,
    ToggleMaximizeRequested,
    ShowRequested,
    HideRequested,
    FocusRequested,
    FileDragRequested(FileDragRequest),
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct FileDragRequest {
    pub paths: Vec<String>,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct OsrPaintBatch {
    pub surface: OsrSurface,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub frames: Vec<OsrFrame>,
}

#[derive(Clone, Debug)]
pub(crate) struct OsrFrame {
    pub surface: OsrSurface,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OsrSurface {
    Main,
    Popup,
}

pub(crate) fn read_message(reader: &mut UnixStream) -> io::Result<Option<OsrMessage>> {
    let Some((header, fd)) = read_header(reader)? else {
        return Ok(None);
    };
    if &header[0..4] != MAGIC {
        close_optional_fd(fd);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid OSR message magic",
        ));
    }

    let kind = read_u32(&header[4..8]);
    let width = read_u32(&header[8..12]);
    let height = read_u32(&header[12..16]);
    let x = read_i32(&header[16..20]);
    let y = read_i32(&header[20..24]);
    let payload_len = read_u32(&header[24..28]) as usize;
    let mut payload = vec![0_u8; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload)?;
    }

    let message = match kind {
        KIND_MAIN_FRAME | KIND_POPUP_FRAME => {
            close_optional_fd(fd);
            OsrMessage::Frame(OsrFrame {
                surface: if kind == KIND_MAIN_FRAME {
                    OsrSurface::Main
                } else {
                    OsrSurface::Popup
                },
                width,
                height,
                x,
                y,
                bytes: payload,
            })
        }
        KIND_MAIN_BATCH | KIND_POPUP_BATCH => {
            close_optional_fd(fd);
            OsrMessage::PaintBatch(parse_paint_batch(
                kind, width, height, x, y, &payload, None,
            )?)
        }
        KIND_MAIN_SHARED_BATCH | KIND_POPUP_SHARED_BATCH => {
            OsrMessage::PaintBatch(parse_paint_batch(kind, width, height, x, y, &payload, fd)?)
        }
        KIND_POPUP_HIDDEN => {
            close_optional_fd(fd);
            OsrMessage::PopupHidden
        }
        KIND_CURSOR => {
            close_optional_fd(fd);
            OsrMessage::Cursor(String::from_utf8(payload).unwrap_or_default())
        }
        KIND_CLOSE_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::CloseRequested
        }
        KIND_START_DRAG_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::StartDragRequested
        }
        KIND_MINIMIZE_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::MinimizeRequested
        }
        KIND_TOGGLE_MAXIMIZE_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::ToggleMaximizeRequested
        }
        KIND_SHOW_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::ShowRequested
        }
        KIND_HIDE_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::HideRequested
        }
        KIND_FOCUS_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::FocusRequested
        }
        KIND_FILE_DRAG_REQUESTED => {
            close_optional_fd(fd);
            match parse_file_drag_request(&payload, x, y) {
                Some(request) => OsrMessage::FileDragRequested(request),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid file drag request payload",
                    ));
                }
            }
        }
        _ => {
            close_optional_fd(fd);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown OSR message kind",
            ));
        }
    };
    Ok(Some(message))
}

fn read_header(reader: &mut UnixStream) -> io::Result<Option<([u8; HEADER_LEN], Option<i32>)>> {
    let mut header = [0_u8; HEADER_LEN];
    let mut filled = 0;
    let mut fd = None;
    while filled < HEADER_LEN {
        if filled == 0 {
            match recv_header_start(reader, &mut header)? {
                Some((read, received_fd)) => {
                    filled = read.min(HEADER_LEN);
                    fd = received_fd;
                }
                None => return Ok(None),
            }
        } else {
            match reader.read_exact(&mut header[filled..]) {
                Ok(()) => filled = HEADER_LEN,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(error) => return Err(error),
            }
        }
    }
    Ok(Some((header, fd)))
}

fn recv_header_start(
    reader: &UnixStream,
    header: &mut [u8; HEADER_LEN],
) -> io::Result<Option<(usize, Option<i32>)>> {
    let mut iov = libc::iovec {
        iov_base: header.as_mut_ptr().cast(),
        iov_len: HEADER_LEN,
    };
    let mut control = [0_u8; 64];
    let mut message = libc::msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: control.as_mut_ptr().cast(),
        msg_controllen: control.len(),
        msg_flags: 0,
    };
    let result = unsafe { libc::recvmsg(reader.as_raw_fd(), &mut message, 0) };
    if result == 0 {
        return Ok(None);
    }
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { received_fd(&message) };
    Ok(Some((result as usize, fd)))
}

unsafe fn received_fd(message: &libc::msghdr) -> Option<i32> {
    let mut control = unsafe { libc::CMSG_FIRSTHDR(message) };
    while !control.is_null() {
        let header = unsafe { &*control };
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
            return Some(unsafe { *(libc::CMSG_DATA(control).cast::<i32>()) });
        }
        control = unsafe { libc::CMSG_NXTHDR(message, control) };
    }
    None
}

fn parse_paint_batch(
    kind: u32,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    payload: &[u8],
    fd: Option<i32>,
) -> io::Result<OsrPaintBatch> {
    let shared = matches!(kind, KIND_MAIN_SHARED_BATCH | KIND_POPUP_SHARED_BATCH);
    let surface = if matches!(kind, KIND_MAIN_BATCH | KIND_MAIN_SHARED_BATCH) {
        OsrSurface::Main
    } else {
        OsrSurface::Popup
    };
    let source_bytes;
    let source = if shared {
        let fd = fd.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "shared OSR paint batch missing file descriptor",
            )
        })?;
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.seek(SeekFrom::Start(0))?;
        source_bytes = read_shared_bytes(file)?;
        source_bytes.as_slice()
    } else {
        close_optional_fd(fd);
        let count = payload_count(payload)?;
        let blob_start = 4 + count * BATCH_ENTRY_LEN;
        payload
            .get(blob_start..)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid OSR paint batch"))?
    };
    let count = payload_count(payload)?;
    let entries_end = 4 + count * BATCH_ENTRY_LEN;
    let entries = payload
        .get(4..entries_end)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated OSR paint batch"))?;
    let mut frames = Vec::with_capacity(count);
    for entry in entries.chunks_exact(BATCH_ENTRY_LEN) {
        let rect_x = read_i32(&entry[0..4]);
        let rect_y = read_i32(&entry[4..8]);
        let rect_width = read_u32(&entry[8..12]);
        let rect_height = read_u32(&entry[12..16]);
        let offset = read_u64(&entry[16..24]) as usize;
        let len = read_u32(&entry[24..28]) as usize;
        let expected_len = rect_width as usize * rect_height as usize * 4;
        if len != expected_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid OSR paint rect byte length",
            ));
        }
        let bytes = source
            .get(offset..offset + len)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "truncated OSR paint rect bytes")
            })?
            .to_vec();
        frames.push(OsrFrame {
            surface,
            width: rect_width,
            height: rect_height,
            x: rect_x,
            y: rect_y,
            bytes,
        });
    }
    Ok(OsrPaintBatch {
        surface,
        width,
        height,
        x,
        y,
        frames,
    })
}

fn parse_file_drag_request(payload: &[u8], x: i32, y: i32) -> Option<FileDragRequest> {
    let value: Value = serde_json::from_slice(payload).ok()?;
    let paths = value
        .get("paths")?
        .as_array()?
        .iter()
        .filter_map(|item| item.as_str().map(String::from))
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return None;
    }
    Some(FileDragRequest { paths, x, y })
}

fn payload_count(payload: &[u8]) -> io::Result<usize> {
    let Some(count) = payload.get(0..4).map(read_u32) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated OSR paint batch header",
        ));
    };
    let count = count as usize;
    let entries_end = 4 + count * BATCH_ENTRY_LEN;
    if payload.len() < entries_end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated OSR paint batch entries",
        ));
    }
    Ok(count)
}

fn read_shared_bytes(mut file: File) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn close_optional_fd(fd: Option<i32>) {
    if let Some(fd) = fd {
        unsafe {
            libc::close(fd);
        }
    }
}

pub(crate) fn regions_to_json(regions: &WindowRegions) -> Value {
    serde_json::json!({
        "blur": region_to_json(regions.blur.as_ref()),
        "opaque": region_to_json(regions.opaque.as_ref()),
        "input": region_to_json(regions.input.as_ref()),
    })
}

pub(crate) fn regions_from_json(value: Option<&Value>) -> WindowRegions {
    let Some(value) = value else {
        return WindowRegions::default();
    };
    WindowRegions {
        blur: region_from_json(value.get("blur")),
        opaque: region_from_json(value.get("opaque")),
        input: region_from_json(value.get("input")),
    }
}

pub(crate) fn rects_to_json(rects: &[WindowRegionRect]) -> Value {
    Value::Array(rects.iter().map(rect_to_json).collect())
}

pub(crate) fn rects_from_json(value: Option<&Value>) -> Vec<WindowRegionRect> {
    value
        .and_then(Value::as_array)
        .map(|rects| rects.iter().filter_map(rect_from_json).collect())
        .unwrap_or_default()
}

pub(crate) fn control_regions_to_json(regions: &[FenestraWindowControlRegion]) -> Value {
    Value::Array(
        regions
            .iter()
            .map(|region| {
                serde_json::json!({
                    "action": region.action.as_str(),
                    "rect": rect_to_json(&region.rect),
                })
            })
            .collect(),
    )
}

pub(crate) fn control_regions_from_json(value: Option<&Value>) -> Vec<FenestraWindowControlRegion> {
    value
        .and_then(Value::as_array)
        .map(|regions| {
            regions
                .iter()
                .filter_map(|region| {
                    Some(FenestraWindowControlRegion::new(
                        FenestraWindowControlAction::parse(region.get("action")?.as_str()?)?,
                        rect_from_json(region.get("rect")?)?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn lifecycle_to_json(lifecycle: &FenestraLifecyclePolicy) -> Value {
    serde_json::json!({
        "active_frame_rate": lifecycle.active_frame_rate,
        "background_frame_rate": lifecycle.background_frame_rate.max(1),
        "suspend_on_minimize": lifecycle.suspend_on_minimize,
        "suspend_on_occluded": lifecycle.suspend_on_occluded,
        "suspend_on_blur": lifecycle.suspend_on_blur,
        "hibernate_after_ms": lifecycle.hibernate_after.map(duration_millis),
        "hibernate_grace_ms": duration_millis(lifecycle.hibernate_grace),
        "retain_hidden_frame": lifecycle.retain_hidden_frame,
    })
}

pub(crate) fn lifecycle_from_json(value: Option<&Value>) -> FenestraLifecyclePolicy {
    let Some(value) = value else {
        return FenestraLifecyclePolicy::default();
    };
    let mut lifecycle = FenestraLifecyclePolicy::default();
    lifecycle.active_frame_rate = value
        .get("active_frame_rate")
        .and_then(Value::as_u64)
        .map(|value| value.min(u32::MAX as u64) as u32)
        .unwrap_or(lifecycle.active_frame_rate);
    lifecycle.background_frame_rate = value
        .get("background_frame_rate")
        .and_then(Value::as_u64)
        .map(|value| value.max(1) as u32)
        .unwrap_or(lifecycle.background_frame_rate);
    lifecycle.suspend_on_minimize = value
        .get("suspend_on_minimize")
        .and_then(Value::as_bool)
        .unwrap_or(lifecycle.suspend_on_minimize);
    lifecycle.suspend_on_occluded = value
        .get("suspend_on_occluded")
        .and_then(Value::as_bool)
        .unwrap_or(lifecycle.suspend_on_occluded);
    lifecycle.suspend_on_blur = value
        .get("suspend_on_blur")
        .and_then(Value::as_bool)
        .unwrap_or(lifecycle.suspend_on_blur);
    lifecycle.hibernate_after = value
        .get("hibernate_after_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .map(Duration::from_millis);
    lifecycle.hibernate_grace = value
        .get("hibernate_grace_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .unwrap_or(lifecycle.hibernate_grace);
    lifecycle.retain_hidden_frame = value
        .get("retain_hidden_frame")
        .and_then(Value::as_bool)
        .unwrap_or(lifecycle.retain_hidden_frame);
    lifecycle
}

pub(crate) fn shell_surface_to_json(shell_surface: Option<&ShellSurfaceOptions>) -> Value {
    let Some(shell_surface) = shell_surface else {
        return Value::Null;
    };
    let size = shell_surface
        .size
        .map(|(width, height)| serde_json::json!({ "width": width, "height": height }))
        .unwrap_or(Value::Null);
    serde_json::json!({
        "namespace": shell_surface.namespace,
        "size": size,
        "layer": shell_surface_layer_to_str(shell_surface.layer),
        "anchor": {
            "top": shell_surface.anchor.top,
            "right": shell_surface.anchor.right,
            "bottom": shell_surface.anchor.bottom,
            "left": shell_surface.anchor.left,
        },
        "margin": {
            "top": shell_surface.margin.top,
            "right": shell_surface.margin.right,
            "bottom": shell_surface.margin.bottom,
            "left": shell_surface.margin.left,
        },
        "exclusive_zone": shell_surface.exclusive_zone,
        "keyboard_interactivity": shell_surface_keyboard_to_str(shell_surface.keyboard_interactivity),
        "events_transparent": shell_surface.events_transparent,
    })
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

pub(crate) fn shell_surface_from_json(value: Option<&Value>) -> Option<ShellSurfaceOptions> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let namespace = value.get("namespace")?.as_str()?.to_string();
    if namespace.is_empty() {
        return None;
    }
    let mut options = ShellSurfaceOptions::new(namespace);
    options.size = value.get("size").and_then(shell_surface_size_from_json);
    options.layer = value
        .get("layer")
        .and_then(Value::as_str)
        .and_then(shell_surface_layer_from_str)
        .unwrap_or(ShellSurfaceLayer::Top);
    options.anchor = value
        .get("anchor")
        .map(shell_surface_anchor_from_json)
        .unwrap_or_default();
    options.margin = value
        .get("margin")
        .map(shell_surface_margin_from_json)
        .unwrap_or_default();
    options.exclusive_zone = value
        .get("exclusive_zone")
        .and_then(Value::as_i64)
        .map(|value| value as i32);
    options.keyboard_interactivity = value
        .get("keyboard_interactivity")
        .and_then(Value::as_str)
        .and_then(shell_surface_keyboard_from_str)
        .unwrap_or(ShellSurfaceKeyboardInteractivity::OnDemand);
    options.events_transparent = value
        .get("events_transparent")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(options)
}

pub(crate) fn encode_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn shell_surface_layer_to_str(layer: ShellSurfaceLayer) -> &'static str {
    match layer {
        ShellSurfaceLayer::Background => "background",
        ShellSurfaceLayer::Bottom => "bottom",
        ShellSurfaceLayer::Top => "top",
        ShellSurfaceLayer::Overlay => "overlay",
    }
}

fn shell_surface_layer_from_str(value: &str) -> Option<ShellSurfaceLayer> {
    match value {
        "background" => Some(ShellSurfaceLayer::Background),
        "bottom" => Some(ShellSurfaceLayer::Bottom),
        "top" => Some(ShellSurfaceLayer::Top),
        "overlay" => Some(ShellSurfaceLayer::Overlay),
        _ => None,
    }
}

fn shell_surface_keyboard_to_str(keyboard: ShellSurfaceKeyboardInteractivity) -> &'static str {
    match keyboard {
        ShellSurfaceKeyboardInteractivity::None => "none",
        ShellSurfaceKeyboardInteractivity::OnDemand => "on-demand",
        ShellSurfaceKeyboardInteractivity::Exclusive => "exclusive",
    }
}

fn shell_surface_keyboard_from_str(value: &str) -> Option<ShellSurfaceKeyboardInteractivity> {
    match value {
        "none" => Some(ShellSurfaceKeyboardInteractivity::None),
        "on-demand" => Some(ShellSurfaceKeyboardInteractivity::OnDemand),
        "exclusive" => Some(ShellSurfaceKeyboardInteractivity::Exclusive),
        _ => None,
    }
}

fn shell_surface_size_from_json(value: &Value) -> Option<(u32, u32)> {
    if value.is_null() {
        return None;
    }
    Some((
        value.get("width")?.as_u64()? as u32,
        value.get("height")?.as_u64()? as u32,
    ))
}

fn shell_surface_anchor_from_json(value: &Value) -> ShellSurfaceAnchor {
    ShellSurfaceAnchor {
        top: value.get("top").and_then(Value::as_bool).unwrap_or(false),
        right: value.get("right").and_then(Value::as_bool).unwrap_or(false),
        bottom: value
            .get("bottom")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        left: value.get("left").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn shell_surface_margin_from_json(value: &Value) -> ShellSurfaceMargin {
    ShellSurfaceMargin {
        top: value.get("top").and_then(Value::as_i64).unwrap_or(0) as i32,
        right: value.get("right").and_then(Value::as_i64).unwrap_or(0) as i32,
        bottom: value.get("bottom").and_then(Value::as_i64).unwrap_or(0) as i32,
        left: value.get("left").and_then(Value::as_i64).unwrap_or(0) as i32,
    }
}

fn region_to_json(region: Option<&WindowRegion>) -> Value {
    let Some(region) = region else {
        return Value::Null;
    };
    serde_json::json!({
        "adaptive": adaptive_to_json(region.adaptive.as_ref()),
        "rects": rects_to_json(&region.rects),
    })
}

fn rect_to_json(rect: &WindowRegionRect) -> Value {
    serde_json::json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height,
    })
}

fn adaptive_to_json(adaptive: Option<&WindowRegionAdaptive>) -> Value {
    match adaptive {
        Some(WindowRegionAdaptive::Full) => serde_json::json!({ "kind": "full" }),
        Some(WindowRegionAdaptive::RoundedRect { radius }) => {
            serde_json::json!({ "kind": "rounded_rect", "radius": radius })
        }
        Some(WindowRegionAdaptive::RoundedLeft { width, radius }) => {
            serde_json::json!({ "kind": "rounded_left", "width": width, "radius": radius })
        }
        Some(WindowRegionAdaptive::TitlebarAndSidebar {
            sidebar_width,
            titlebar_height,
            radius,
        }) => {
            serde_json::json!({
                "kind": "titlebar_sidebar",
                "sidebar_width": sidebar_width,
                "titlebar_height": titlebar_height,
                "radius": radius,
            })
        }
        Some(WindowRegionAdaptive::ContentAfterSidebar {
            sidebar_width,
            titlebar_height,
        }) => {
            serde_json::json!({
                "kind": "content_after_sidebar",
                "sidebar_width": sidebar_width,
                "titlebar_height": titlebar_height,
            })
        }
        Some(WindowRegionAdaptive::ContentAfterSidebarRoundedRight {
            sidebar_width,
            titlebar_height,
            radius,
        }) => {
            serde_json::json!({
                "kind": "content_after_sidebar_rounded_right",
                "sidebar_width": sidebar_width,
                "titlebar_height": titlebar_height,
                "radius": radius,
            })
        }
        None => Value::Null,
    }
}

fn region_from_json(value: Option<&Value>) -> Option<WindowRegion> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let adaptive = adaptive_from_json(value.get("adaptive"));
    let rects = value
        .get("rects")
        .and_then(Value::as_array)
        .map(|rects| rects.iter().filter_map(rect_from_json).collect::<Vec<_>>())
        .unwrap_or_default();
    Some(WindowRegion { rects, adaptive })
}

fn adaptive_from_json(value: Option<&Value>) -> Option<WindowRegionAdaptive> {
    let value = value?;
    match value.get("kind").and_then(Value::as_str)? {
        "full" => Some(WindowRegionAdaptive::Full),
        "rounded_rect" => Some(WindowRegionAdaptive::RoundedRect {
            radius: value.get("radius").and_then(Value::as_i64).unwrap_or(0) as i32,
        }),
        "rounded_left" => Some(WindowRegionAdaptive::RoundedLeft {
            width: value.get("width").and_then(Value::as_i64).unwrap_or(0) as i32,
            radius: value.get("radius").and_then(Value::as_i64).unwrap_or(0) as i32,
        }),
        "titlebar_sidebar" => Some(WindowRegionAdaptive::TitlebarAndSidebar {
            sidebar_width: value
                .get("sidebar_width")
                .and_then(Value::as_i64)
                .unwrap_or(0) as i32,
            titlebar_height: value
                .get("titlebar_height")
                .and_then(Value::as_i64)
                .unwrap_or(0) as i32,
            radius: value.get("radius").and_then(Value::as_i64).unwrap_or(0) as i32,
        }),
        "content_after_sidebar" => Some(WindowRegionAdaptive::ContentAfterSidebar {
            sidebar_width: value
                .get("sidebar_width")
                .and_then(Value::as_i64)
                .unwrap_or(0) as i32,
            titlebar_height: value
                .get("titlebar_height")
                .and_then(Value::as_i64)
                .unwrap_or(0) as i32,
        }),
        "content_after_sidebar_rounded_right" => {
            Some(WindowRegionAdaptive::ContentAfterSidebarRoundedRight {
                sidebar_width: value
                    .get("sidebar_width")
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i32,
                titlebar_height: value
                    .get("titlebar_height")
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i32,
                radius: value.get("radius").and_then(Value::as_i64).unwrap_or(0) as i32,
            })
        }
        _ => None,
    }
}

fn rect_from_json(value: &Value) -> Option<WindowRegionRect> {
    Some(WindowRegionRect::new(
        value.get("x")?.as_i64()? as i32,
        value.get("y")?.as_i64()? as i32,
        value.get("width")?.as_i64()? as i32,
        value.get("height")?.as_i64()? as i32,
    ))
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("slice length checked"))
}

fn read_i32(bytes: &[u8]) -> i32 {
    i32::from_le_bytes(bytes.try_into().expect("slice length checked"))
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("slice length checked"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn inline_paint_batch_parses_multiple_rects() {
        let (mut reader, mut writer) = UnixStream::pair().expect("socket pair");
        let mut payload = Vec::new();
        payload.extend_from_slice(&2_u32.to_le_bytes());
        push_entry(&mut payload, 0, 0, 1, 1, 0, 4);
        push_entry(&mut payload, 2, 1, 1, 1, 4, 4);
        payload.extend_from_slice(&[1, 1, 1, 255, 2, 2, 2, 255]);
        let mut header = vec![0_u8; HEADER_LEN];
        header[0..4].copy_from_slice(MAGIC);
        header[4..8].copy_from_slice(&KIND_MAIN_BATCH.to_le_bytes());
        header[8..12].copy_from_slice(&3_u32.to_le_bytes());
        header[12..16].copy_from_slice(&2_u32.to_le_bytes());
        header[24..28].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        writer.write_all(&header).expect("header");
        writer.write_all(&payload).expect("payload");

        let message = read_message(&mut reader).expect("read").expect("message");
        let OsrMessage::PaintBatch(batch) = message else {
            panic!("expected paint batch");
        };
        assert_eq!(batch.surface, OsrSurface::Main);
        assert_eq!((batch.width, batch.height), (3, 2));
        assert_eq!(batch.frames.len(), 2);
        assert_eq!((batch.frames[1].x, batch.frames[1].y), (2, 1));
        assert_eq!(batch.frames[1].bytes, vec![2, 2, 2, 255]);
    }

    fn push_entry(
        payload: &mut Vec<u8>,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        offset: u64,
        len: u32,
    ) {
        payload.extend_from_slice(&x.to_le_bytes());
        payload.extend_from_slice(&y.to_le_bytes());
        payload.extend_from_slice(&width.to_le_bytes());
        payload.extend_from_slice(&height.to_le_bytes());
        payload.extend_from_slice(&offset.to_le_bytes());
        payload.extend_from_slice(&len.to_le_bytes());
    }
}
