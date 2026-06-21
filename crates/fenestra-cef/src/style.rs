#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    pub const TEXT: Self = Self::rgb(0.95, 0.95, 0.95);
    pub const TEXT_MUTED: Self = Self::rgb(0.58, 0.58, 0.58);
    pub const ACCENT: Self = Self::rgb(0.66, 0.66, 0.66);
    pub const SURFACE: Self = Self::rgb(0.11, 0.11, 0.11);
    pub const WINDOW: Self = Self::rgb(0.08, 0.08, 0.08);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn opacity(self, alpha: f32) -> Self {
        Self {
            a: self.a * alpha.clamp(0.0, 1.0),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberSpacing {
    Proportional,
    Tabular,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextWrap {
    Normal,
    Pretty,
    Balance,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Material {
    Solid(Color),
}
