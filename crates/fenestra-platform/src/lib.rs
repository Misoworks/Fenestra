mod shell;
#[path = "platform/wayland_background_effect.rs"]
mod wayland_background_effect;

use std::{path::PathBuf, sync::Arc};

use winit::window::Window;

pub use shell::{
    ShellSurfaceAnchor, ShellSurfaceKeyboardInteractivity, ShellSurfaceLayer, ShellSurfaceMargin,
    ShellSurfaceOptions,
};
pub use wayland_background_effect::WaylandEffect as WindowEffect;

pub fn request_window_effect(
    window: &Arc<dyn Window>,
    options: &WindowOptions,
) -> Option<WindowEffect> {
    wayland_background_effect::request(window, options)
}

pub fn request_surface_effect<W>(
    window: &W,
    options: &WindowOptions,
    width: i32,
    height: i32,
) -> Option<WindowEffect>
where
    W: raw_window_handle::HasDisplayHandle + raw_window_handle::HasWindowHandle + ?Sized,
{
    wayland_background_effect::request_surface(window, options, width, height)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlatformOs {
    Linux,
    Windows,
    Macos,
    Android,
    Ios,
    Web,
    #[default]
    Unknown,
}

pub fn current_desktop_os() -> PlatformOs {
    if cfg!(target_os = "linux") {
        PlatformOs::Linux
    } else if cfg!(target_os = "windows") {
        PlatformOs::Windows
    } else if cfg!(target_os = "macos") {
        PlatformOs::Macos
    } else {
        PlatformOs::Unknown
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowChrome {
    #[default]
    System,
    Fenestra,
    Frameless,
    None,
}

impl WindowChrome {
    pub fn uses_native_decorations(self) -> bool {
        matches!(self, Self::System)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowBackgroundEffect {
    #[default]
    None,
    Blur,
    Glass,
    Acrylic,
    Mica,
    MicaAlt,
    Vibrancy,
    HudWindow,
    Sidebar,
    UnderWindowBackground,
}

impl WindowBackgroundEffect {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "blur" => Some(Self::Blur),
            "glass" => Some(Self::Glass),
            "acrylic" => Some(Self::Acrylic),
            "mica" => Some(Self::Mica),
            "mica-alt" => Some(Self::MicaAlt),
            "vibrancy" => Some(Self::Vibrancy),
            "hud-window" => Some(Self::HudWindow),
            "sidebar" => Some(Self::Sidebar),
            "under-window-background" => Some(Self::UnderWindowBackground),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Blur => "blur",
            Self::Glass => "glass",
            Self::Acrylic => "acrylic",
            Self::Mica => "mica",
            Self::MicaAlt => "mica-alt",
            Self::Vibrancy => "vibrancy",
            Self::HudWindow => "hud-window",
            Self::Sidebar => "sidebar",
            Self::UnderWindowBackground => "under-window-background",
        }
    }

    pub fn requires_transparency(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub chrome: WindowChrome,
    pub resizable: bool,
    pub visible: bool,
    pub active: bool,
    pub always_on_top: bool,
    pub transparent: bool,
    pub background_effect: WindowBackgroundEffect,
    pub regions: WindowRegions,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Fenestra".to_string(),
            width: 760,
            height: 520,
            min_width: 420,
            min_height: 280,
            chrome: WindowChrome::System,
            resizable: true,
            visible: true,
            active: true,
            always_on_top: false,
            transparent: false,
            background_effect: WindowBackgroundEffect::None,
            regions: WindowRegions::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRegionRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl WindowRegionRect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width: width.max(0),
            height: height.max(0),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowRegion {
    pub rects: Vec<WindowRegionRect>,
    pub adaptive: Option<WindowRegionAdaptive>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowRegionAdaptive {
    Full,
    RoundedRect {
        radius: i32,
    },
    RoundedLeft {
        width: i32,
        radius: i32,
    },
    TitlebarAndSidebar {
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    },
    ContentAfterSidebar {
        sidebar_width: i32,
        titlebar_height: i32,
    },
    ContentAfterSidebarRoundedRight {
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    },
}

impl WindowRegion {
    pub fn empty() -> Self {
        Self {
            rects: Vec::new(),
            adaptive: None,
        }
    }

    pub fn rect(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self::empty().add_rect(x, y, width, height)
    }

    pub fn full(width: i32, height: i32) -> Self {
        Self::rect(0, 0, width, height)
    }

    pub fn adaptive_full() -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::Full),
        }
    }

    pub fn rounded_rect(width: i32, height: i32, radius: i32) -> Self {
        let mut region = Self::empty();
        let radius = radius.min(width / 2).min(height / 2).max(0);
        if radius == 0 {
            return Self::full(width, height);
        }
        for y in 0..height.max(0) {
            let inset = rounded_region_row_inset(y, height, radius);
            region = region.add_rect(inset, y, width - inset * 2, 1);
        }
        region
    }

    pub fn adaptive_rounded_rect(radius: i32) -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::RoundedRect { radius }),
        }
    }

    pub fn rounded_left(width: i32, height: i32, radius: i32) -> Self {
        let mut region = Self::empty();
        let radius = radius.min(width).min(height / 2).max(0);
        if radius == 0 {
            return Self::full(width, height);
        }
        for y in 0..height.max(0) {
            let inset = rounded_region_row_inset(y, height, radius);
            region = region.add_rect(inset, y, width - inset, 1);
        }
        region
    }

    pub fn adaptive_rounded_left(width: i32, radius: i32) -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::RoundedLeft { width, radius }),
        }
    }

    pub fn adaptive_titlebar_sidebar(
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    ) -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::TitlebarAndSidebar {
                sidebar_width,
                titlebar_height,
                radius,
            }),
        }
    }

    pub fn adaptive_content_after_sidebar(sidebar_width: i32, titlebar_height: i32) -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::ContentAfterSidebar {
                sidebar_width,
                titlebar_height,
            }),
        }
    }

    pub fn adaptive_content_after_sidebar_rounded_right(
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    ) -> Self {
        Self {
            rects: Vec::new(),
            adaptive: Some(WindowRegionAdaptive::ContentAfterSidebarRoundedRight {
                sidebar_width,
                titlebar_height,
                radius,
            }),
        }
    }

    pub fn add_rect(mut self, x: i32, y: i32, width: i32, height: i32) -> Self {
        let rect = WindowRegionRect::new(x, y, width, height);
        if !rect.is_empty() {
            self.rects.push(rect);
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty() && self.adaptive.is_none()
    }

    pub fn resolved_rects(&self, width: i32, height: i32) -> Vec<WindowRegionRect> {
        match self.adaptive {
            Some(WindowRegionAdaptive::Full) => WindowRegion::full(width, height).rects,
            Some(WindowRegionAdaptive::RoundedRect { radius }) => {
                WindowRegion::rounded_rect(width, height, radius).rects
            }
            Some(WindowRegionAdaptive::RoundedLeft {
                width: sidebar_width,
                radius,
            }) => WindowRegion::rounded_left(sidebar_width, height, radius).rects,
            Some(WindowRegionAdaptive::TitlebarAndSidebar {
                sidebar_width,
                titlebar_height,
                radius,
            }) => {
                WindowRegion::titlebar_sidebar(
                    width,
                    height,
                    sidebar_width,
                    titlebar_height,
                    radius,
                )
                .rects
            }
            Some(WindowRegionAdaptive::ContentAfterSidebar {
                sidebar_width,
                titlebar_height,
            }) => {
                WindowRegion::content_after_sidebar(width, height, sidebar_width, titlebar_height)
                    .rects
            }
            Some(WindowRegionAdaptive::ContentAfterSidebarRoundedRight {
                sidebar_width,
                titlebar_height,
                radius,
            }) => {
                WindowRegion::content_after_sidebar_rounded_right(
                    width,
                    height,
                    sidebar_width,
                    titlebar_height,
                    radius,
                )
                .rects
            }
            None => self.rects.clone(),
        }
    }

    fn titlebar_sidebar(
        width: i32,
        height: i32,
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    ) -> Self {
        let mut region = Self::empty();
        let width = width.max(0);
        let height = height.max(0);
        let sidebar_width = sidebar_width.clamp(0, width);
        let titlebar_height = titlebar_height.clamp(0, height);
        let radius = radius.min(width / 2).min(height / 2).max(0);

        for y in 0..titlebar_height {
            let inset = if radius > 0 && y < radius {
                rounded_region_row_inset(y, radius * 2, radius)
            } else {
                0
            };
            region = region.add_rect(inset, y, width - inset * 2, 1);
        }

        for y in titlebar_height..height {
            let inset = if radius > 0 && y >= height - radius {
                rounded_region_row_inset(y, height, radius)
            } else {
                0
            };
            region = region.add_rect(inset, y, sidebar_width - inset, 1);
        }

        region
    }

    fn content_after_sidebar(
        width: i32,
        height: i32,
        sidebar_width: i32,
        titlebar_height: i32,
    ) -> Self {
        let width = width.max(0);
        let height = height.max(0);
        let sidebar_width = sidebar_width.clamp(0, width);
        let titlebar_height = titlebar_height.clamp(0, height);
        Self::rect(
            sidebar_width,
            titlebar_height,
            width - sidebar_width,
            height - titlebar_height,
        )
    }

    fn content_after_sidebar_rounded_right(
        width: i32,
        height: i32,
        sidebar_width: i32,
        titlebar_height: i32,
        radius: i32,
    ) -> Self {
        let mut region = Self::empty();
        let width = width.max(0);
        let height = height.max(0);
        let sidebar_width = sidebar_width.clamp(0, width);
        let titlebar_height = titlebar_height.clamp(0, height);
        let radius = radius
            .min((width - sidebar_width) / 2)
            .min(height / 2)
            .max(0);
        if radius == 0 {
            return Self::content_after_sidebar(width, height, sidebar_width, titlebar_height);
        }

        for y in titlebar_height..height {
            let inset = rounded_region_row_inset(y, height, radius);
            region = region.add_rect(sidebar_width, y, width - sidebar_width - inset, 1);
        }

        region
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowRegions {
    pub blur: Option<WindowRegion>,
    pub opaque: Option<WindowRegion>,
    pub input: Option<WindowRegion>,
}

impl WindowRegions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn blur(mut self, region: WindowRegion) -> Self {
        self.blur = Some(region);
        self
    }

    pub fn opaque(mut self, region: WindowRegion) -> Self {
        self.opaque = Some(region);
        self
    }

    pub fn input(mut self, region: WindowRegion) -> Self {
        self.input = Some(region);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.blur.as_ref().is_none_or(WindowRegion::is_empty)
            && self.opaque.as_ref().is_none_or(WindowRegion::is_empty)
            && self.input.as_ref().is_none_or(WindowRegion::is_empty)
    }
}

fn rounded_region_row_inset(y: i32, height: i32, radius: i32) -> i32 {
    let top = y < radius;
    let bottom = y >= height - radius;
    if !top && !bottom {
        return 0;
    }

    let center_y = if top { radius } else { height - radius - 1 };
    let dy = (y - center_y).abs() as f64;
    let radius = radius as f64;
    (radius - (radius * radius - dy * dy).max(0.0).sqrt()).ceil() as i32
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayIcon {
    pub id: String,
    pub title: String,
    pub icon_path: Option<PathBuf>,
    pub tooltip: Option<String>,
    pub menu: Vec<TrayMenuItem>,
}

impl TrayIcon {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            icon_path: None,
            tooltip: None,
            menu: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayMenuItem {
    pub id: String,
    pub label: String,
    pub action: Option<String>,
    pub enabled: bool,
    pub separator: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutostartEntry {
    pub id: String,
    pub name: String,
    pub command: String,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShortcutModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: ShortcutModifiers,
    pub key: String,
}

impl Shortcut {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            modifiers: ShortcutModifiers::default(),
            key: key.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalShortcutRegistration {
    pub id: String,
    pub shortcut: Shortcut,
    pub action: String,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub description: Option<String>,
    pub desktop_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepLinkRegistration {
    pub id: String,
    pub schemes: Vec<String>,
}

impl DeepLinkRegistration {
    pub fn new(
        id: impl Into<String>,
        schemes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            schemes: schemes.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeMessagingHost {
    pub id: String,
    pub name: String,
    pub executable: PathBuf,
    pub allowed_origins: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SingleInstancePolicy {
    #[default]
    AllowMultiple,
    ReuseExisting,
    FocusExisting,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayActivation {
    pub tray_id: String,
    pub item_id: Option<String>,
    pub action: Option<String>,
}

impl TrayActivation {
    pub fn new(tray_id: impl Into<String>) -> Self {
        Self {
            tray_id: tray_id.into(),
            item_id: None,
            action: None,
        }
    }

    pub fn item(
        tray_id: impl Into<String>,
        item_id: impl Into<String>,
        action: Option<String>,
    ) -> Self {
        Self {
            tray_id: tray_id.into(),
            item_id: Some(item_id.into()),
            action,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalShortcutActivation {
    pub id: String,
    pub action: String,
    pub activation_token: Option<String>,
}

impl GlobalShortcutActivation {
    pub fn new(id: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            action: action.into(),
            activation_token: None,
        }
    }

    pub fn activation_token(mut self, token: impl Into<String>) -> Self {
        self.activation_token = Some(token.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SingleInstanceActivation {
    pub policy: SingleInstancePolicy,
    pub arguments: Vec<String>,
    pub working_directory: Option<PathBuf>,
    pub activation_token: Option<String>,
}

impl SingleInstanceActivation {
    pub fn new(policy: SingleInstancePolicy, arguments: Vec<String>) -> Self {
        Self {
            policy,
            arguments,
            working_directory: None,
            activation_token: None,
        }
    }

    pub fn working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }

    pub fn activation_token(mut self, token: impl Into<String>) -> Self {
        self.activation_token = Some(token.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformEvent {
    Tray(TrayActivation),
    GlobalShortcut(GlobalShortcutActivation),
    SingleInstance(SingleInstanceActivation),
}
