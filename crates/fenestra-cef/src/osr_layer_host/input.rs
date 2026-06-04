use layershellev::{
    keyboard::{Key, PhysicalKey},
    xkb_keyboard::KeyEvent,
};

pub(super) const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
pub(super) const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
pub(super) const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
pub(super) const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
pub(super) const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
pub(super) const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;
pub(super) const EVENTFLAG_COMMAND_DOWN: u32 = 1 << 7;
pub(super) const EVENTFLAG_IS_REPEAT: u32 = 1 << 13;
pub(super) const EVENTFLAG_PRECISION_SCROLLING_DELTA: u32 = 1 << 14;

pub(super) fn cef_mouse_button(button: u32) -> Option<&'static str> {
    match button {
        0x110 => Some("left"),
        0x112 => Some("middle"),
        0x111 => Some("right"),
        _ => None,
    }
}

pub(super) fn axis_delta(axis: &layershellev::AxisScroll) -> i32 {
    if axis.stop {
        return 0;
    }
    if axis.absolute != 0.0 {
        axis.absolute.round() as i32
    } else {
        axis.discrete * 120
    }
}

pub(super) fn key_name(event: &KeyEvent) -> String {
    match event.logical_key.as_ref() {
        Key::Character(value) if !value.is_empty() => value.to_string(),
        Key::Named(named) => named.to_string(),
        _ => match &event.physical_key {
            PhysicalKey::Code(code) => format!("{code:?}"),
            _ => "Unidentified".to_string(),
        },
    }
}

pub(super) fn should_send_char_text(text: &str) -> bool {
    !matches!(text, "\u{8}" | "\u{7f}" | "\u{1b}")
}

pub(super) fn cursor_shape_for_wayland(cursor: &str) -> &'static str {
    match cursor {
        "pointer" | "hand" => "pointer",
        "text" | "vertical-text" => "text",
        "crosshair" => "crosshair",
        "move" => "move",
        "wait" => "wait",
        "help" => "help",
        "not-allowed" => "not-allowed",
        "col-resize" | "ew-resize" => "ew-resize",
        "row-resize" | "ns-resize" => "ns-resize",
        "ne-resize" => "ne-resize",
        "nw-resize" => "nw-resize",
        "se-resize" => "se-resize",
        "sw-resize" => "sw-resize",
        _ => "default",
    }
}
