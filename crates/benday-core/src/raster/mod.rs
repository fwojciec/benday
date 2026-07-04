//! Sub-cell pixel canvas: 2×4 pseudo-pixels per terminal cell, rendered as
//! braille dots (U+2800 block) or octants (Unicode 16 Symbols for Legacy
//! Computing Supplement).
//!
//! Pixels are stored as a row-major bit pattern per cell: bit `row*2 + col`,
//! row 0 at the top. One foreground color per cell, last write wins.
//!
//! This module also owns `rasterize()`: a compiled `Scene` in, glyphs out. It
//! never sees a `Theme` — every color it stamps comes from the Scene.

use crate::ansi::Buffer;
use crate::render::{BarStyle, Rendered};
use crate::scene::{Scene, SceneMark};

/// Bar-fill glyph ramp for block bars, indexed 0..8 by eighths of a cell.
const EIGHTHS: [char; 8] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];

/// Everything the rasterizer needs beyond the Scene itself. The theme is
/// deliberately absent: colors are compile-time facts baked into the Scene.
pub struct RasterOptions {
    pub marker: Marker,
    pub bar_style: BarStyle,
    pub color: bool,
}

/// Turn a compiled Scene into a `Rendered` (ANSI text + --meta payload).
/// Reproduces the pre-refactor cell buffer exactly: title, legend, y-axis
/// chrome + labels, marks, x-axis chrome + labels, all placed from Scene data.
pub fn rasterize(scene: &Scene, opts: &RasterOptions) -> Rendered {
    let mut buf = Buffer::new(scene.size.columns, scene.size.rows);
    // plot.x is `gutter + 1`; the gutter column sits one to its left.
    let gutter = scene.plot.x.saturating_sub(1);
    let top = scene.plot.y;
    let plot_w = scene.plot.w;
    let plot_h = scene.plot.h;
    let axis = Some(scene.chrome.axis);

    if let Some(t) = &scene.title {
        buf.text(t.col, t.row, &t.text, Some(scene.chrome.title));
    }
    for entry in &scene.legend {
        buf.text(entry.col, entry.row, "──", Some(entry.color));
        buf.text(entry.col + 3, entry.row, &entry.name, axis);
    }

    // Y axis: the full vertical rule first, then tick marks + labels on top.
    for r in 0..plot_h {
        buf.set(gutter, top + r, '│', axis);
    }
    for tick in &scene.y_axis.ticks {
        buf.set(gutter, tick.row, '┤', axis);
        let len = tick.label.chars().count();
        buf.text(gutter.saturating_sub(len), tick.row, &tick.label, axis);
    }

    for mark in &scene.marks {
        match mark {
            SceneMark::Bars { bars } => {
                rasterize_bars(&mut buf, bars, opts, gutter, top, plot_w, plot_h);
            }
            SceneMark::Path { .. } | SceneMark::Points { .. } | SceneMark::Fill { .. } => {
                todo!("xy marks rasterize in Task 5")
            }
        }
    }

    // X axis: baseline, category/quantitative tick glyphs, then labels.
    let axis_row = top + plot_h;
    buf.set(gutter, axis_row, '└', axis);
    for c in 0..plot_w {
        buf.set(gutter + 1 + c, axis_row, '─', axis);
    }
    for &c in &scene.x_axis.tick_cols {
        if c < plot_w {
            buf.set(gutter + 1 + c, axis_row, '┴', axis);
        }
    }
    for label in &scene.x_axis.labels {
        buf.text(label.col, label.row, &label.text, axis);
    }

    Rendered {
        text: buf.to_ansi(opts.color),
        meta: scene.meta(),
    }
}

fn rasterize_bars(
    buf: &mut Buffer,
    bars: &[crate::scene::Bar],
    opts: &RasterOptions,
    gutter: usize,
    top: usize,
    plot_w: usize,
    plot_h: usize,
) {
    match opts.bar_style {
        BarStyle::Dots => {
            let mut canvas = PixelCanvas::new(plot_w, plot_h, opts.marker);
            let ph = (plot_h * 4) as i64;
            for bar in bars {
                let x0 = (bar.x0 * plot_w as f64).round() as usize;
                let bar_w = (bar.w * plot_w as f64).round() as usize;
                let level = (bar.h * ph as f64).round() as i64;
                for px in (x0 * 2) as i64..((x0 + bar_w) * 2) as i64 {
                    for py in (ph - level)..ph {
                        canvas.set(px, py, bar.color);
                    }
                }
            }
            for cy in 0..plot_h {
                for cx in 0..plot_w {
                    if let Some((ch, color)) = canvas.cell(cx, cy) {
                        buf.set(gutter + 1 + cx, top + cy, ch, Some(color));
                    }
                }
            }
        }
        BarStyle::Blocks => {
            for bar in bars {
                let x0 = (bar.x0 * plot_w as f64).round() as usize;
                let bar_w = (bar.w * plot_w as f64).round() as usize;
                let level = (bar.h * (plot_h * 8) as f64).round() as i64;
                for r in 0..plot_h {
                    let fill = level - ((plot_h - 1 - r) * 8) as i64;
                    if fill <= 0 {
                        continue;
                    }
                    let ch = if fill >= 8 {
                        '█'
                    } else {
                        EIGHTHS[fill as usize]
                    };
                    for c in 0..bar_w {
                        buf.set(gutter + 1 + x0 + c, top + r, ch, Some(bar.color));
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

impl serde::Serialize for Rgb {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
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
    char::from_u32(0x2800 + u32::from(v)).expect("U+2800..=U+28FF are valid chars")
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
