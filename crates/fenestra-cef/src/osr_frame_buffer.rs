use crate::osr_protocol::OsrFrame;

pub(crate) struct FrameBuffer {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameDamage {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl FrameBuffer {
    pub(crate) fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            bytes: Vec::new(),
        }
    }

    pub(crate) fn release(&mut self) {
        self.width = 0;
        self.height = 0;
        self.bytes = Vec::new();
    }

    pub(crate) fn compose(
        &mut self,
        width: u32,
        height: u32,
        frame: &OsrFrame,
    ) -> Option<FrameDamage> {
        if width == 0 || height == 0 {
            return None;
        }
        let damage = frame_damage(frame, width, height)?;
        if !frame_payload_is_valid(frame, width, height) {
            return None;
        }
        self.ensure_size(width, height);
        compose_frame(frame, &mut self.bytes, width, height, false).then_some(damage)
    }

    pub(crate) fn compose_batch<'a>(
        &mut self,
        width: u32,
        height: u32,
        frames: impl IntoIterator<Item = &'a OsrFrame>,
    ) -> Option<FrameDamage> {
        if width == 0 || height == 0 {
            return None;
        }
        let frames = frames.into_iter().collect::<Vec<_>>();
        let mut damage: Option<FrameDamage> = None;
        for frame in &frames {
            let frame_damage = frame_damage(frame, width, height)?;
            if !frame_payload_is_valid(frame, width, height) {
                return None;
            }
            damage = Some(match damage {
                Some(damage) => damage.union(frame_damage),
                None => frame_damage,
            });
        }
        self.ensure_size(width, height);
        for frame in frames {
            if !compose_frame(frame, &mut self.bytes, width, height, false) {
                return None;
            }
        }
        damage
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn ensure_size(&mut self, width: u32, height: u32) {
        if self.width == width
            && self.height == height
            && self.bytes.len() == buffer_len(width, height)
        {
            return;
        }
        self.width = width;
        self.height = height;
        self.bytes.clear();
        self.bytes.resize(buffer_len(width, height), 0);
    }
}

impl FrameDamage {
    pub(crate) fn union(self, other: Self) -> Self {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        Self {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        }
    }
}

pub(crate) fn frame_damage(frame: &OsrFrame, width: u32, height: u32) -> Option<FrameDamage> {
    let left = i64::from(frame.x).max(0);
    let top = i64::from(frame.y).max(0);
    let right = (i64::from(frame.x) + i64::from(frame.width)).min(i64::from(width));
    let bottom = (i64::from(frame.y) + i64::from(frame.height)).min(i64::from(height));
    if right <= left || bottom <= top {
        return None;
    }
    Some(FrameDamage {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

pub(crate) fn buffer_len(width: u32, height: u32) -> usize {
    width as usize * height as usize * 4
}

pub(crate) fn ensure_buffer(buffer: &mut Vec<u8>, byte_len: usize) -> bool {
    if buffer.len() == byte_len {
        return false;
    }
    buffer.clear();
    buffer.resize(byte_len, 0);
    true
}

pub(crate) fn compose_frame(
    frame: &OsrFrame,
    target: &mut [u8],
    width: u32,
    height: u32,
    blend: bool,
) -> bool {
    if frame.width == 0 || frame.height == 0 {
        return false;
    }
    let source_x = frame.x.min(0).unsigned_abs();
    let source_y = frame.y.min(0).unsigned_abs();
    let x_offset = frame.x.max(0) as u32;
    let y_offset = frame.y.max(0) as u32;
    let draw_width = (width.saturating_sub(x_offset)).min(frame.width.saturating_sub(source_x));
    let draw_height = (height.saturating_sub(y_offset)).min(frame.height.saturating_sub(source_y));
    if draw_width == 0 || draw_height == 0 {
        return false;
    }
    if !blend {
        let row_bytes = (draw_width * 4) as usize;
        for y in 0..draw_height {
            let source_index = (((y + source_y) * frame.width + source_x) * 4) as usize;
            let target_index = (((y + y_offset) * width + x_offset) * 4) as usize;
            let Some(source) = frame.bytes.get(source_index..source_index + row_bytes) else {
                return false;
            };
            let Some(destination) = target.get_mut(target_index..target_index + row_bytes) else {
                return false;
            };
            destination.copy_from_slice(source);
        }
        return true;
    }
    for y in 0..draw_height {
        for x in 0..draw_width {
            let source_index = (((y + source_y) * frame.width + x + source_x) * 4) as usize;
            let target_index = (((y + y_offset) * width + x + x_offset) * 4) as usize;
            let Some(source) = frame.bytes.get(source_index..source_index + 4) else {
                return false;
            };
            let Some(destination) = target.get_mut(target_index..target_index + 4) else {
                return false;
            };
            blend_bgra(source, destination);
        }
    }
    true
}

fn frame_payload_is_valid(frame: &OsrFrame, width: u32, height: u32) -> bool {
    if frame.width == 0 || frame.height == 0 {
        return false;
    }
    let source_x = frame.x.min(0).unsigned_abs();
    let source_y = frame.y.min(0).unsigned_abs();
    let x_offset = frame.x.max(0) as u32;
    let y_offset = frame.y.max(0) as u32;
    let draw_width = (width.saturating_sub(x_offset)).min(frame.width.saturating_sub(source_x));
    let draw_height = (height.saturating_sub(y_offset)).min(frame.height.saturating_sub(source_y));
    if draw_width == 0 || draw_height == 0 {
        return false;
    }
    let row_bytes = (draw_width * 4) as usize;
    let last_row = source_y + draw_height - 1;
    let last_source_index = ((last_row * frame.width + source_x) * 4) as usize;
    last_source_index
        .checked_add(row_bytes)
        .is_some_and(|end| end <= frame.bytes.len())
}

fn blend_bgra(source: &[u8], destination: &mut [u8]) {
    let alpha = source[3] as u16;
    if alpha == 255 {
        destination.copy_from_slice(source);
        return;
    }
    if alpha == 0 {
        return;
    }
    let inverse = 255 - alpha;
    destination[0] = (source[0] as u16 + destination[0] as u16 * inverse / 255).min(255) as u8;
    destination[1] = (source[1] as u16 + destination[1] as u16 * inverse / 255).min(255) as u8;
    destination[2] = (source[2] as u16 + destination[2] as u16 * inverse / 255).min(255) as u8;
    destination[3] = (alpha + destination[3] as u16 * inverse / 255).min(255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osr_protocol::OsrSurface;

    #[test]
    fn dirty_rect_patches_backing_store_at_offset() {
        let mut buffer = FrameBuffer::new();
        let full = OsrFrame {
            surface: OsrSurface::Main,
            width: 3,
            height: 2,
            x: 0,
            y: 0,
            bytes: vec![
                1, 1, 1, 255, 2, 2, 2, 255, 3, 3, 3, 255, 4, 4, 4, 255, 5, 5, 5, 255, 6, 6, 6, 255,
            ],
        };
        let dirty = OsrFrame {
            surface: OsrSurface::Main,
            width: 1,
            height: 1,
            x: 1,
            y: 0,
            bytes: vec![9, 9, 9, 255],
        };

        assert_eq!(
            buffer.compose(3, 2, &full),
            Some(FrameDamage {
                x: 0,
                y: 0,
                width: 3,
                height: 2
            })
        );
        assert_eq!(
            buffer.compose(3, 2, &dirty),
            Some(FrameDamage {
                x: 1,
                y: 0,
                width: 1,
                height: 1
            })
        );

        assert_eq!(&buffer.bytes()[0..4], &[1, 1, 1, 255]);
        assert_eq!(&buffer.bytes()[4..8], &[9, 9, 9, 255]);
        assert_eq!(&buffer.bytes()[8..12], &[3, 3, 3, 255]);
        assert_eq!(&buffer.bytes()[20..24], &[6, 6, 6, 255]);
    }

    #[test]
    fn batch_composition_returns_union_damage() {
        let mut buffer = FrameBuffer::new();
        let left = OsrFrame {
            surface: OsrSurface::Main,
            width: 1,
            height: 1,
            x: 0,
            y: 0,
            bytes: vec![1, 1, 1, 255],
        };
        let right = OsrFrame {
            surface: OsrSurface::Main,
            width: 1,
            height: 1,
            x: 3,
            y: 2,
            bytes: vec![2, 2, 2, 255],
        };

        assert_eq!(
            buffer.compose_batch(4, 3, [&left, &right]),
            Some(FrameDamage {
                x: 0,
                y: 0,
                width: 4,
                height: 3,
            })
        );
        assert_eq!(&buffer.bytes()[0..4], &[1, 1, 1, 255]);
        assert_eq!(&buffer.bytes()[44..48], &[2, 2, 2, 255]);
    }

    #[test]
    fn negative_dirty_rect_is_cropped_into_target() {
        let mut buffer = FrameBuffer::new();
        let frame = OsrFrame {
            surface: OsrSurface::Main,
            width: 2,
            height: 2,
            x: -1,
            y: -1,
            bytes: vec![1, 1, 1, 255, 2, 2, 2, 255, 3, 3, 3, 255, 4, 4, 4, 255],
        };

        assert_eq!(
            buffer.compose(2, 2, &frame),
            Some(FrameDamage {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            })
        );
        assert_eq!(&buffer.bytes()[0..4], &[4, 4, 4, 255]);
        assert_eq!(&buffer.bytes()[4..8], &[0, 0, 0, 0]);
    }

    #[test]
    fn invalid_dirty_payload_does_not_update_backing_store() {
        let mut buffer = FrameBuffer::new();
        let full = OsrFrame {
            surface: OsrSurface::Main,
            width: 1,
            height: 1,
            x: 0,
            y: 0,
            bytes: vec![7, 7, 7, 255],
        };
        let invalid = OsrFrame {
            surface: OsrSurface::Main,
            width: 2,
            height: 2,
            x: 0,
            y: 0,
            bytes: vec![9, 9, 9, 255],
        };

        assert!(buffer.compose(1, 1, &full).is_some());
        assert!(buffer.compose(2, 2, &invalid).is_none());
        assert_eq!(&buffer.bytes()[0..4], &[7, 7, 7, 255]);
    }
}
