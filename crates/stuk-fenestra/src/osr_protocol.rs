use std::io::{self, Read};

use serde_json::Value;
use stuk_platform::{WindowRegion, WindowRegionAdaptive, WindowRegionRect, WindowRegions};

const HEADER_LEN: usize = 28;
const MAGIC: &[u8; 4] = b"SKOR";

pub(crate) const MAIN_TEXTURE_ID: &str = "__stuk_fenestra_main";
pub(crate) const POPUP_TEXTURE_ID: &str = "__stuk_fenestra_popup";

#[derive(Debug)]
pub(crate) enum OsrMessage {
    Frame(OsrFrame),
    PopupHidden,
    Cursor(String),
    CloseRequested,
    StartDragRequested,
    MinimizeRequested,
    ToggleMaximizeRequested,
}

#[derive(Debug)]
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

pub(crate) fn read_message(reader: &mut impl Read) -> io::Result<Option<OsrMessage>> {
    let mut header = [0_u8; HEADER_LEN];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    if &header[0..4] != MAGIC {
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

    match kind {
        1 | 2 => Ok(Some(OsrMessage::Frame(OsrFrame {
            surface: if kind == 1 {
                OsrSurface::Main
            } else {
                OsrSurface::Popup
            },
            width,
            height,
            x,
            y,
            bytes: payload,
        }))),
        3 => Ok(Some(OsrMessage::PopupHidden)),
        4 => Ok(Some(OsrMessage::Cursor(
            String::from_utf8(payload).unwrap_or_default(),
        ))),
        5 => Ok(Some(OsrMessage::CloseRequested)),
        6 => Ok(Some(OsrMessage::StartDragRequested)),
        7 => Ok(Some(OsrMessage::MinimizeRequested)),
        8 => Ok(Some(OsrMessage::ToggleMaximizeRequested)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown OSR message kind",
        )),
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

fn region_to_json(region: Option<&WindowRegion>) -> Value {
    let Some(region) = region else {
        return Value::Null;
    };
    serde_json::json!({
        "adaptive": adaptive_to_json(region.adaptive.as_ref()),
        "rects": region.rects.iter().map(|rect| {
            serde_json::json!({
                "x": rect.x,
                "y": rect.y,
                "width": rect.width,
                "height": rect.height,
            })
        }).collect::<Vec<_>>(),
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
