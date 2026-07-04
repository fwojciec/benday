//! Sub-cell pixel canvas: 2×4 pseudo-pixels per terminal cell, rendered as
//! braille dots (U+2800 block) or octants (Unicode 16 Symbols for Legacy
//! Computing Supplement).
//!
//! Pixels are stored as a row-major bit pattern per cell: bit `row*2 + col`,
//! row 0 at the top. One foreground color per cell, last write wins.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Braille,
    Octant,
}

pub struct PixelCanvas {
    width_cells: usize,
    height_cells: usize,
    bits: Vec<u8>,
    colors: Vec<Option<Rgb>>,
    marker: Marker,
}

impl PixelCanvas {
    pub fn new(width_cells: usize, height_cells: usize, marker: Marker) -> Self {
        PixelCanvas {
            width_cells,
            height_cells,
            bits: vec![0; width_cells * height_cells],
            colors: vec![None; width_cells * height_cells],
            marker,
        }
    }

    pub fn pixel_width(&self) -> usize {
        self.width_cells * 2
    }

    pub fn pixel_height(&self) -> usize {
        self.height_cells * 4
    }

    pub fn set(&mut self, x: i64, y: i64, color: Rgb) {
        if x < 0 || y < 0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.pixel_width() || y >= self.pixel_height() {
            return;
        }
        let idx = (y / 4) * self.width_cells + x / 2;
        self.bits[idx] |= 1 << ((y % 4) * 2 + (x % 2));
        self.colors[idx] = Some(color);
    }

    /// Bresenham line in pixel coordinates.
    pub fn line(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, color: Rgb) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            self.set(x, y, color);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// The rendered glyph and color for a cell, or None if it's empty.
    pub fn cell(&self, cx: usize, cy: usize) -> Option<(char, Rgb)> {
        let idx = cy * self.width_cells + cx;
        let bits = self.bits[idx];
        if bits == 0 {
            return None;
        }
        let ch = match self.marker {
            Marker::Braille => braille_char(bits),
            Marker::Octant => OCTANTS[bits as usize],
        };
        Some((ch, self.colors[idx].unwrap_or(Rgb(255, 255, 255))))
    }
}

/// Braille dot values by (row, col), per the Unicode braille encoding
/// (dots 1-2-3-7 in the left column, 4-5-6-8 in the right).
const BRAILLE_DOT: [[u16; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

fn braille_char(bits: u8) -> char {
    let mut v: u16 = 0;
    for (row, cols) in BRAILLE_DOT.iter().enumerate() {
        for (col, dot) in cols.iter().enumerate() {
            if bits & (1 << (row * 2 + col)) != 0 {
                v |= dot;
            }
        }
    }
    char::from_u32(0x2800 + u32::from(v)).unwrap()
}

/// Octant glyphs indexed by row-major bit pattern.
/// Table from ratatui (`ratatui-core/src/symbols/pixel.rs`), MIT license.
#[rustfmt::skip]
pub const OCTANTS: [char; 256] = [
    ' ', '𜺨', '𜺫', '🮂', '𜴀', '▘', '𜴁', '𜴂', '𜴃', '𜴄', '▝', '𜴅', '𜴆', '𜴇', '𜴈', '▀', '𜴉', '𜴊', '𜴋',
    '𜴌', '🯦', '𜴍', '𜴎', '𜴏', '𜴐', '𜴑', '𜴒', '𜴓', '𜴔', '𜴕', '𜴖', '𜴗', '𜴘', '𜴙', '𜴚', '𜴛', '𜴜', '𜴝',
    '𜴞', '𜴟', '🯧', '𜴠', '𜴡', '𜴢', '𜴣', '𜴤', '𜴥', '𜴦', '𜴧', '𜴨', '𜴩', '𜴪', '𜴫', '𜴬', '𜴭', '𜴮', '𜴯',
    '𜴰', '𜴱', '𜴲', '𜴳', '𜴴', '𜴵', '🮅', '𜺣', '𜴶', '𜴷', '𜴸', '𜴹', '𜴺', '𜴻', '𜴼', '𜴽', '𜴾', '𜴿', '𜵀',
    '𜵁', '𜵂', '𜵃', '𜵄', '▖', '𜵅', '𜵆', '𜵇', '𜵈', '▌', '𜵉', '𜵊', '𜵋', '𜵌', '▞', '𜵍', '𜵎', '𜵏', '𜵐',
    '▛', '𜵑', '𜵒', '𜵓', '𜵔', '𜵕', '𜵖', '𜵗', '𜵘', '𜵙', '𜵚', '𜵛', '𜵜', '𜵝', '𜵞', '𜵟', '𜵠', '𜵡', '𜵢',
    '𜵣', '𜵤', '𜵥', '𜵦', '𜵧', '𜵨', '𜵩', '𜵪', '𜵫', '𜵬', '𜵭', '𜵮', '𜵯', '𜵰', '𜺠', '𜵱', '𜵲', '𜵳', '𜵴',
    '𜵵', '𜵶', '𜵷', '𜵸', '𜵹', '𜵺', '𜵻', '𜵼', '𜵽', '𜵾', '𜵿', '𜶀', '𜶁', '𜶂', '𜶃', '𜶄', '𜶅', '𜶆', '𜶇',
    '𜶈', '𜶉', '𜶊', '𜶋', '𜶌', '𜶍', '𜶎', '𜶏', '▗', '𜶐', '𜶑', '𜶒', '𜶓', '▚', '𜶔', '𜶕', '𜶖', '𜶗', '▐',
    '𜶘', '𜶙', '𜶚', '𜶛', '▜', '𜶜', '𜶝', '𜶞', '𜶟', '𜶠', '𜶡', '𜶢', '𜶣', '𜶤', '𜶥', '𜶦', '𜶧', '𜶨', '𜶩',
    '𜶪', '𜶫', '▂', '𜶬', '𜶭', '𜶮', '𜶯', '𜶰', '𜶱', '𜶲', '𜶳', '𜶴', '𜶵', '𜶶', '𜶷', '𜶸', '𜶹', '𜶺', '𜶻',
    '𜶼', '𜶽', '𜶾', '𜶿', '𜷀', '𜷁', '𜷂', '𜷃', '𜷄', '𜷅', '𜷆', '𜷇', '𜷈', '𜷉', '𜷊', '𜷋', '𜷌', '𜷍', '𜷎',
    '𜷏', '𜷐', '𜷑', '𜷒', '𜷓', '𜷔', '𜷕', '𜷖', '𜷗', '𜷘', '𜷙', '𜷚', '▄', '𜷛', '𜷜', '𜷝', '𜷞', '▙', '𜷟',
    '𜷠', '𜷡', '𜷢', '▟', '𜷣', '▆', '𜷤', '𜷥', '█',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_bit_mapping() {
        assert_eq!(braille_char(0b0000_0001), '⠁'); // top-left dot
        assert_eq!(braille_char(0b1111_1111), '⣿'); // all eight dots
    }

    #[test]
    fn octant_table_landmarks() {
        assert_eq!(OCTANTS[0b0000_1111], '▀'); // top four pixels = upper half
        assert_eq!(OCTANTS[255], '█');
    }

    #[test]
    fn canvas_maps_pixels_to_cells() {
        let mut c = PixelCanvas::new(2, 1, Marker::Braille);
        c.set(0, 0, Rgb(255, 0, 0));
        assert_eq!(c.cell(0, 0), Some(('⠁', Rgb(255, 0, 0))));
        assert_eq!(c.cell(1, 0), None);
    }
}
