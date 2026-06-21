#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellSurfaceOptions {
    pub namespace: String,
    pub size: Option<(u32, u32)>,
    pub layer: ShellSurfaceLayer,
    pub anchor: ShellSurfaceAnchor,
    pub margin: ShellSurfaceMargin,
    pub exclusive_zone: Option<i32>,
    pub keyboard_interactivity: ShellSurfaceKeyboardInteractivity,
    pub events_transparent: bool,
}

impl ShellSurfaceOptions {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            size: None,
            layer: ShellSurfaceLayer::Top,
            anchor: ShellSurfaceAnchor::default(),
            margin: ShellSurfaceMargin::default(),
            exclusive_zone: None,
            keyboard_interactivity: ShellSurfaceKeyboardInteractivity::OnDemand,
            events_transparent: false,
        }
    }

    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = Some((width, height));
        self
    }

    pub fn layer(mut self, layer: ShellSurfaceLayer) -> Self {
        self.layer = layer;
        self
    }

    pub fn anchor(mut self, anchor: ShellSurfaceAnchor) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn margin(mut self, margin: ShellSurfaceMargin) -> Self {
        self.margin = margin;
        self
    }

    pub fn exclusive_zone(mut self, exclusive_zone: i32) -> Self {
        self.exclusive_zone = Some(exclusive_zone);
        self
    }

    pub fn keyboard_interactivity(
        mut self,
        keyboard_interactivity: ShellSurfaceKeyboardInteractivity,
    ) -> Self {
        self.keyboard_interactivity = keyboard_interactivity;
        self
    }

    pub fn events_transparent(mut self, events_transparent: bool) -> Self {
        self.events_transparent = events_transparent;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShellSurfaceLayer {
    Background,
    Bottom,
    #[default]
    Top,
    Overlay,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShellSurfaceAnchor {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl ShellSurfaceAnchor {
    pub const TOP: Self = Self {
        top: true,
        right: false,
        bottom: false,
        left: false,
    };
    pub const RIGHT: Self = Self {
        top: false,
        right: true,
        bottom: false,
        left: false,
    };
    pub const BOTTOM: Self = Self {
        top: false,
        right: false,
        bottom: true,
        left: false,
    };
    pub const LEFT: Self = Self {
        top: false,
        right: false,
        bottom: false,
        left: true,
    };
    pub const ALL: Self = Self {
        top: true,
        right: true,
        bottom: true,
        left: true,
    };
}

impl std::ops::BitOr for ShellSurfaceAnchor {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self {
            top: self.top || rhs.top,
            right: self.right || rhs.right,
            bottom: self.bottom || rhs.bottom,
            left: self.left || rhs.left,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShellSurfaceMargin {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

impl ShellSurfaceMargin {
    pub const fn new(top: i32, right: i32, bottom: i32, left: i32) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShellSurfaceKeyboardInteractivity {
    None,
    Exclusive,
    #[default]
    OnDemand,
}
