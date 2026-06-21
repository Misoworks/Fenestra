use crate::style::{Color, Material, NumberSpacing, TextAlign, TextWrap};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayList {
    pub background: Color,
    pub commands: Vec<DisplayCommand>,
    pub hovered_region: Option<String>,
    pub pressed_region: Option<String>,
    pub focused_region: Option<String>,
}

impl DisplayList {
    pub fn new(background: Color) -> Self {
        Self {
            background,
            commands: Vec::new(),
            hovered_region: None,
            pressed_region: None,
            focused_region: None,
        }
    }

    pub fn push(&mut self, command: impl Into<DisplayCommand>) {
        self.commands.push(command.into());
    }

    pub fn paint_bounds(&self) -> Option<Rect> {
        union_bounds(
            self.commands
                .iter()
                .filter_map(DisplayCommand::paint_bounds),
        )
    }

    pub fn initial_damage(&self) -> DisplayDamage {
        DisplayDamage::full()
    }

    pub fn damage_since(&self, previous: &Self) -> DisplayDamage {
        if self.background != previous.background {
            return DisplayDamage::full();
        }

        let mut damage = DisplayDamage::empty();
        let max_len = self.commands.len().max(previous.commands.len());
        for index in 0..max_len {
            let current = self.commands.get(index);
            let old = previous.commands.get(index);
            if current == old {
                continue;
            }

            if !damage_changed_command(current, &mut damage)
                || !damage_changed_command(old, &mut damage)
            {
                return DisplayDamage::full();
            }
        }
        damage
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DisplayCommand {
    Rect(RectCommand),
    RoundedRect(RoundedRectCommand),
    Border(BorderCommand),
    Shadow(ShadowCommand),
    Text(TextCommand),
    Image(ImageCommand),
    Svg(SvgCommand),
    Clip(ClipCommand),
    Transform(TransformCommand),
    Material(MaterialCommand),
}

impl DisplayCommand {
    pub fn paint_bounds(&self) -> Option<Rect> {
        match self {
            Self::Rect(command) => Some(command.bounds()),
            Self::RoundedRect(command) => Some(command.bounds()),
            Self::Border(command) => Some(command.bounds()),
            Self::Shadow(command) => Some(command.bounds()),
            Self::Text(command) => Some(command.bounds()),
            Self::Image(command) => Some(command.bounds()),
            Self::Svg(command) => Some(command.bounds()),
            Self::Clip(_) | Self::Transform(_) => None,
            Self::Material(command) => Some(command.bounds()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RectCommand {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Color,
}

impl RectCommand {
    pub fn bounds(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoundedRectCommand {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub color: Color,
}

impl RoundedRectCommand {
    pub fn bounds(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BorderCommand {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub thickness: f32,
    pub color: Color,
}

impl BorderCommand {
    pub fn bounds(&self) -> Rect {
        let outset = self.thickness * 0.5;
        Rect::new(
            self.x - outset,
            self.y - outset,
            self.width + self.thickness,
            self.height + self.thickness,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShadowCommand {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
}

impl ShadowCommand {
    pub fn bounds(&self) -> Rect {
        let outset = self.blur + self.spread.abs();
        Rect::new(
            self.x + self.offset_x - outset,
            self.y + self.offset_y - outset,
            self.width + outset * 2.0,
            self.height + outset * 2.0,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextCommand {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub size: f32,
    pub line_height: f32,
    pub color: Color,
    pub wrap: TextWrap,
    pub align: TextAlign,
    pub number_spacing: NumberSpacing,
}

impl TextCommand {
    pub fn bounds(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageCommand {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub opacity: f32,
}

impl ImageCommand {
    pub fn bounds(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SvgCommand {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub tint: Option<Color>,
    pub opacity: f32,
}

impl SvgCommand {
    pub fn bounds(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipCommand {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransformCommand {
    pub matrix: [f32; 6],
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialCommand {
    pub material: Material,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub fallback: Color,
}

impl MaterialCommand {
    pub fn bounds(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayDamage {
    full: bool,
    bounds: Option<Rect>,
}

impl DisplayDamage {
    pub fn empty() -> Self {
        Self {
            full: false,
            bounds: None,
        }
    }

    pub fn full() -> Self {
        Self {
            full: true,
            bounds: None,
        }
    }

    pub fn rect(rect: Rect) -> Self {
        let mut damage = Self::empty();
        damage.push(rect);
        damage
    }

    pub fn is_empty(&self) -> bool {
        !self.full && self.bounds.is_none()
    }

    pub fn is_full(&self) -> bool {
        self.full
    }

    pub fn bounds(&self) -> Option<Rect> {
        self.bounds
    }

    pub fn push(&mut self, rect: Rect) {
        if self.full || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        self.bounds = Some(match self.bounds {
            Some(bounds) => union_rect(bounds, rect),
            None => rect,
        });
    }
}

impl From<RectCommand> for DisplayCommand {
    fn from(command: RectCommand) -> Self {
        Self::Rect(command)
    }
}

impl From<RoundedRectCommand> for DisplayCommand {
    fn from(command: RoundedRectCommand) -> Self {
        Self::RoundedRect(command)
    }
}

impl From<BorderCommand> for DisplayCommand {
    fn from(command: BorderCommand) -> Self {
        Self::Border(command)
    }
}

impl From<ShadowCommand> for DisplayCommand {
    fn from(command: ShadowCommand) -> Self {
        Self::Shadow(command)
    }
}

impl From<TextCommand> for DisplayCommand {
    fn from(command: TextCommand) -> Self {
        Self::Text(command)
    }
}

impl From<ImageCommand> for DisplayCommand {
    fn from(command: ImageCommand) -> Self {
        Self::Image(command)
    }
}

impl From<SvgCommand> for DisplayCommand {
    fn from(command: SvgCommand) -> Self {
        Self::Svg(command)
    }
}

impl From<ClipCommand> for DisplayCommand {
    fn from(command: ClipCommand) -> Self {
        Self::Clip(command)
    }
}

impl From<TransformCommand> for DisplayCommand {
    fn from(command: TransformCommand) -> Self {
        Self::Transform(command)
    }
}

impl From<MaterialCommand> for DisplayCommand {
    fn from(command: MaterialCommand) -> Self {
        Self::Material(command)
    }
}

fn damage_changed_command(command: Option<&DisplayCommand>, damage: &mut DisplayDamage) -> bool {
    match command {
        Some(command) => match command.paint_bounds() {
            Some(bounds) => {
                damage.push(bounds);
                true
            }
            None => false,
        },
        None => true,
    }
}

fn union_bounds(mut bounds: impl Iterator<Item = Rect>) -> Option<Rect> {
    let first = bounds.next()?;
    Some(bounds.fold(first, union_rect))
}

fn union_rect(a: Rect, b: Rect) -> Rect {
    let left = a.x.min(b.x);
    let top = a.y.min(b.y);
    let right = (a.x + a.width).max(b.x + b.width);
    let bottom = (a.y + a.height).max(b.y + b.height);
    Rect::new(left, top, right - left, bottom - top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_paint_bounds_across_commands() {
        let mut list = DisplayList::new(Color::WINDOW);
        list.push(RectCommand {
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
            color: Color::WHITE,
        });
        list.push(ShadowCommand {
            x: 80.0,
            y: 90.0,
            width: 20.0,
            height: 30.0,
            radius: 12.0,
            offset_x: 2.0,
            offset_y: 3.0,
            blur: 8.0,
            spread: 1.0,
            color: Color::WHITE.opacity(0.2),
        });

        assert_eq!(
            list.paint_bounds(),
            Some(Rect::new(10.0, 20.0, 101.0, 112.0))
        );
    }

    #[test]
    fn damages_only_changed_command_bounds() {
        let mut before = DisplayList::new(Color::WINDOW);
        before.push(RectCommand {
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
            color: Color::WHITE,
        });

        let mut after = before.clone();
        after.commands[0] = RectCommand {
            x: 20.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
            color: Color::WHITE,
        }
        .into();

        let damage = after.damage_since(&before);

        assert!(!damage.is_full());
        assert_eq!(damage.bounds(), Some(Rect::new(10.0, 20.0, 40.0, 40.0)));
    }

    #[test]
    fn background_or_unknown_state_changes_damage_full_frame() {
        let before = DisplayList::new(Color::WINDOW);
        let after = DisplayList::new(Color::SURFACE);
        assert!(after.damage_since(&before).is_full());

        let mut clipped = DisplayList::new(Color::WINDOW);
        clipped.push(ClipCommand {
            x: 0.0,
            y: 0.0,
            width: 40.0,
            height: 40.0,
            radius: 8.0,
        });
        assert!(clipped.damage_since(&before).is_full());
    }

    #[test]
    fn display_list_tracks_command_count() {
        let mut list = DisplayList::new(Color::WINDOW);
        assert_eq!(list.commands.len(), 0);
        list.push(RectCommand {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            color: Color::WHITE,
        });
        assert_eq!(list.commands.len(), 1);
        list.push(RoundedRectCommand {
            x: 10.0,
            y: 10.0,
            width: 80.0,
            height: 80.0,
            radius: 8.0,
            color: Color::ACCENT,
        });
        assert_eq!(list.commands.len(), 2);
    }

    #[test]
    fn damage_uses_union_of_old_and_new_bounds() {
        let mut before = DisplayList::new(Color::WINDOW);
        before.push(RectCommand {
            x: 100.0,
            y: 100.0,
            width: 50.0,
            height: 50.0,
            color: Color::WHITE,
        });

        let mut after = before.clone();
        after.commands[0] = RectCommand {
            x: 120.0,
            y: 120.0,
            width: 50.0,
            height: 50.0,
            color: Color::ACCENT,
        }
        .into();

        let damage = after.damage_since(&before);
        assert!(!damage.is_full());
        let bounds = damage.bounds().unwrap();
        assert_eq!(bounds.x, 100.0);
        assert_eq!(bounds.y, 100.0);
        assert!(bounds.width >= 70.0);
        assert!(bounds.height >= 70.0);
    }

    #[test]
    fn display_list_includes_all_command_types() {
        let mut list = DisplayList::new(Color::WINDOW);
        list.push(RectCommand {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            color: Color::WHITE,
        });
        list.push(RoundedRectCommand {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            radius: 4.0,
            color: Color::ACCENT,
        });
        list.push(BorderCommand {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            radius: 4.0,
            thickness: 1.0,
            color: Color::TEXT,
        });
        list.push(ShadowCommand {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            radius: 4.0,
            offset_x: 2.0,
            offset_y: 2.0,
            blur: 8.0,
            spread: 0.0,
            color: Color::WHITE.opacity(0.2),
        });
        assert_eq!(list.commands.len(), 4);
    }
}
