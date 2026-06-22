use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
};

use crate::{
    osr_frame_buffer::{buffer_len, compose_frame, ensure_buffer},
    osr_protocol::OsrFrame,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl DamageRect {
    pub(super) fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    pub(super) fn is_full(self, width: u32, height: u32) -> bool {
        self.x == 0 && self.y == 0 && self.width >= width && self.height >= height
    }

    fn union(self, other: Self) -> Self {
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

pub(super) fn paint_buffer_file(
    file: &mut File,
    width: u32,
    height: u32,
    main_frame: Option<&OsrFrame>,
    popup_frame: Option<&OsrFrame>,
    main_buffer: &mut Vec<u8>,
    scratch: &mut Vec<u8>,
) -> std::io::Result<DamageRect> {
    let main_frames = main_frame.into_iter().collect::<Vec<_>>();
    let popup_frames = popup_frame.into_iter().collect::<Vec<_>>();
    paint_frames_buffer_file(
        file,
        width,
        height,
        &main_frames,
        &popup_frames,
        main_buffer,
        scratch,
    )
}

pub(super) fn paint_frames_buffer_file(
    file: &mut File,
    width: u32,
    height: u32,
    main_frames: &[&OsrFrame],
    popup_frames: &[&OsrFrame],
    main_buffer: &mut Vec<u8>,
    scratch: &mut Vec<u8>,
) -> std::io::Result<DamageRect> {
    let byte_len = buffer_len(width, height);
    file.set_len(byte_len as u64)?;
    let resized = ensure_buffer(main_buffer, byte_len);
    let mut damage = resized.then_some(DamageRect::full(width, height));

    for frame in main_frames {
        compose_frame(frame, main_buffer, width, height, false);
        if let Some(frame_damage) = damage_from_frame(frame, width, height) {
            damage = Some(match damage {
                Some(damage) => damage.union(frame_damage),
                None => frame_damage,
            });
        }
    }

    if !popup_frames.is_empty() {
        scratch.clear();
        scratch.extend_from_slice(main_buffer);
        for frame in popup_frames {
            compose_frame(frame, scratch, width, height, true);
        }
        write_full(file, scratch)?;
        return Ok(DamageRect::full(width, height));
    }

    if !scratch.is_empty() {
        scratch.clear();
        scratch.shrink_to_fit();
        write_full(file, main_buffer)?;
        file.flush()?;
        return Ok(DamageRect::full(width, height));
    }

    let damage = damage.unwrap_or_else(|| DamageRect::full(width, height));
    if damage.is_full(width, height) {
        write_full(file, main_buffer)?;
    } else {
        write_rect(file, width, main_buffer, damage)?;
    }
    file.flush()?;
    Ok(damage)
}

fn damage_from_frame(frame: &OsrFrame, width: u32, height: u32) -> Option<DamageRect> {
    let left = i64::from(frame.x).max(0);
    let top = i64::from(frame.y).max(0);
    let right = (i64::from(frame.x) + i64::from(frame.width)).min(i64::from(width));
    let bottom = (i64::from(frame.y) + i64::from(frame.height)).min(i64::from(height));
    if right <= left || bottom <= top {
        return None;
    }
    Some(DamageRect {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

fn write_full(file: &mut File, buffer: &[u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    file.write_all(buffer)
}

fn write_rect(
    file: &mut File,
    surface_width: u32,
    buffer: &[u8],
    damage: DamageRect,
) -> std::io::Result<()> {
    let row_bytes = (damage.width * 4) as usize;
    for row in 0..damage.height {
        let offset = (((damage.y + row) * surface_width + damage.x) * 4) as usize;
        let Some(bytes) = buffer.get(offset..offset + row_bytes) else {
            return Ok(());
        };
        file.seek(SeekFrom::Start(offset as u64))?;
        file.write_all(bytes)?;
    }
    Ok(())
}
