use std::time::Instant;

use layershellev::{
    reexport::wayland_client::{ButtonState, WEnum},
    xkb_keyboard::{ElementState, KeyEvent},
};

use super::{ClickMemory, OsrLayerHost};
use crate::{
    osr_layer_host::input::{
        EVENTFLAG_ALT_DOWN, EVENTFLAG_COMMAND_DOWN, EVENTFLAG_CONTROL_DOWN, EVENTFLAG_IS_REPEAT,
        EVENTFLAG_LEFT_MOUSE_BUTTON, EVENTFLAG_MIDDLE_MOUSE_BUTTON,
        EVENTFLAG_PRECISION_SCROLLING_DELTA, EVENTFLAG_RIGHT_MOUSE_BUTTON, EVENTFLAG_SHIFT_DOWN,
        cef_mouse_button, key_name, should_send_char_text,
    },
    osr_protocol::encode_component,
};

impl OsrLayerHost {
    pub(super) fn forward_mouse_move(&self, leave: bool) {
        self.send_control(&format!(
            "mouse_move\t{:.2}\t{:.2}\t{}\t{}\n",
            self.cursor_x.max(0.0),
            self.cursor_y.max(0.0),
            self.cef_modifiers(),
            i32::from(leave)
        ));
    }

    pub(super) fn forward_mouse_button(&mut self, button: u32, state: &WEnum<ButtonState>) {
        let pressed = matches!(state, WEnum::Value(ButtonState::Pressed));
        let released = matches!(state, WEnum::Value(ButtonState::Released));
        if !pressed && !released {
            return;
        }
        if released && matches!(button, 0x113 | 0x114) {
            self.forward_navigation_button(button);
            return;
        }
        if pressed {
            self.active_click_count = self.next_click_count(button);
        }
        self.set_mouse_button(button, pressed);
        self.forward_mouse_click(button, released, self.active_click_count);
    }

    pub(super) fn forward_mouse_wheel(&self, dx: i32, dy: i32) {
        self.send_control(&format!(
            "mouse_wheel\t{:.2}\t{:.2}\t{}\t{}\t{}\n",
            self.cursor_x.max(0.0),
            self.cursor_y.max(0.0),
            dx,
            dy,
            self.cef_modifiers() | EVENTFLAG_PRECISION_SCROLLING_DELTA
        ));
    }

    pub(super) fn send_key_event(&self, event: &KeyEvent) {
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

    fn forward_mouse_click(&self, button: u32, up: bool, click_count: i32) {
        let Some(button) = cef_mouse_button(button) else {
            return;
        };
        self.send_control(&format!(
            "mouse_click\t{:.2}\t{:.2}\t{}\t{}\t{}\t{}\n",
            self.cursor_x.max(0.0),
            self.cursor_y.max(0.0),
            button,
            self.cef_modifiers(),
            i32::from(up),
            click_count.max(1)
        ));
    }

    fn forward_navigation_button(&self, button: u32) {
        let button = match button {
            0x113 => 3,
            0x114 => 4,
            _ => return,
        };
        self.send_control(&format!(
            "mouse_navigation\t{:.2}\t{:.2}\t{}\t{}\n",
            self.cursor_x.max(0.0),
            self.cursor_y.max(0.0),
            button,
            self.cef_modifiers()
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

    fn set_mouse_button(&mut self, button: u32, pressed: bool) {
        match button {
            0x110 => self.mouse.left = pressed,
            0x112 => self.mouse.middle = pressed,
            0x111 => self.mouse.right = pressed,
            _ => {}
        }
    }

    fn next_click_count(&mut self, button: u32) -> i32 {
        let now = Instant::now();
        let count = self
            .last_click
            .filter(|last| {
                last.button == button
                    && now.duration_since(last.at).as_millis() <= 500
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
