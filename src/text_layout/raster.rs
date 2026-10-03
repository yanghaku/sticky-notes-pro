//! Editor drawing colors and safe straight-alpha RGBA blending.

use cosmic_text::Color;

// Cap each immutable editor image at 32 MiB and reduce raster scale at extreme DPI.
pub(super) const MAX_PIXELS: u32 = 8_388_608;
pub(super) const MAX_PIXEL_SIDE: f32 = 8192.0;

pub(super) struct EditorPalette {
    pub(super) body: Color,
    pub(super) marker: Color,
    pub(super) selection: [u8; 4],
    pub(super) ime_underline: [u8; 4],
}

impl EditorPalette {
    pub(super) fn for_dark(dark: bool) -> Self {
        if dark {
            Self {
                body: Color::rgb(235, 238, 242),
                marker: Color::rgb(190, 196, 205),
                selection: [92, 157, 255, 105],
                ime_underline: [235, 238, 242, 220],
            }
        } else {
            Self {
                body: Color::rgb(52, 48, 43),
                marker: Color::rgb(83, 77, 65),
                selection: [65, 125, 210, 82],
                ime_underline: [47, 45, 40, 220],
            }
        }
    }
}

pub(super) struct Canvas<'a> {
    pub(super) bytes: &'a mut [u8],
    pub(super) width: u32,
    pub(super) height: u32,
}
impl Canvas<'_> {
    /// Glyph coordinates are already raster pixels. Reject transparent/outside
    /// pixels before indexing; no float conversion or rectangle loop per pixel.
    pub(super) fn pixel(&mut self, x: i32, y: i32, color: [u8; 4]) {
        if color[3] == 0 || x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return;
        }
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        blend(&mut self.bytes[offset..offset + 4], color);
    }
    pub(super) fn rect(&mut self, x: f32, y: f32, width: f32, height: f32, color: [u8; 4]) {
        let left = x.round().clamp(0.0, self.width as f32) as usize;
        let top = y.round().clamp(0.0, self.height as f32) as usize;
        let right = (x + width).round().clamp(0.0, self.width as f32) as usize;
        let bottom = (y + height).round().clamp(0.0, self.height as f32) as usize;
        let alpha = u32::from(color[3]);
        if alpha == 0 {
            return;
        }
        for row in top..bottom {
            for column in left..right {
                let offset = (row * self.width as usize + column) * 4;
                let target = &mut self.bytes[offset..offset + 4];
                blend(target, color);
            }
        }
    }
}

fn blend(target: &mut [u8], color: [u8; 4]) {
    if color[3] == 255 || target[3] == 0 {
        target.copy_from_slice(&color);
        return;
    }
    let alpha = u32::from(color[3]);
    let old_alpha = u32::from(target[3]);
    let remainder = 255 - alpha;
    let out_alpha = alpha + old_alpha * remainder / 255;
    for channel in 0..3 {
        let value = u32::from(color[channel]) * alpha
            + u32::from(target[channel]) * old_alpha * remainder / 255;
        target[channel] = (value / out_alpha) as u8;
    }
    target[3] = out_alpha as u8;
}
