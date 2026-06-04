use layershellev::reexport::{Anchor, KeyboardInteractivity, Layer};
use stuk_platform_shell::{
    ShellSurfaceAnchor, ShellSurfaceKeyboardInteractivity, ShellSurfaceLayer,
};

pub(super) fn layer_for_shell(layer: ShellSurfaceLayer) -> Layer {
    match layer {
        ShellSurfaceLayer::Background => Layer::Background,
        ShellSurfaceLayer::Bottom => Layer::Bottom,
        ShellSurfaceLayer::Top => Layer::Top,
        ShellSurfaceLayer::Overlay => Layer::Overlay,
    }
}

pub(super) fn anchor_for_shell(anchor: ShellSurfaceAnchor) -> Anchor {
    let mut value = Anchor::empty();
    if anchor.top {
        value |= Anchor::Top;
    }
    if anchor.right {
        value |= Anchor::Right;
    }
    if anchor.bottom {
        value |= Anchor::Bottom;
    }
    if anchor.left {
        value |= Anchor::Left;
    }
    value
}

pub(super) fn keyboard_for_shell(
    keyboard: ShellSurfaceKeyboardInteractivity,
) -> KeyboardInteractivity {
    match keyboard {
        ShellSurfaceKeyboardInteractivity::None => KeyboardInteractivity::None,
        ShellSurfaceKeyboardInteractivity::OnDemand => KeyboardInteractivity::OnDemand,
        ShellSurfaceKeyboardInteractivity::Exclusive => KeyboardInteractivity::Exclusive,
    }
}
