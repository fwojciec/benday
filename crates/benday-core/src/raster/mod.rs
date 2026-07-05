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
use crate::scene::{BarDirection, Scene, SceneMark};

/// Bar-fill glyph ramp for vertical block bars, indexed 0..8 by eighths of a
/// cell filled from the BOTTOM up.
const EIGHTHS: [char; 8] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];

/// Bar-fill glyph ramp for horizontal block bars, indexed 0..8 by eighths of a
/// cell filled from the LEFT (U+258F down to U+2589); index 8 is `█`, handled
/// separately like `EIGHTHS`.
const LEFT_EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];

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

    // Bars draw straight to the buffer. XY marks (line/point/area) all share a
    // single pixel canvas so overlapping sub-pixels in the same cell merge into
    // one glyph — matching the pre-refactor single-canvas draw — then blit once.
    match scene.marks.first() {
        Some(SceneMark::Bars { .. }) => {
            for mark in &scene.marks {
                if let SceneMark::Bars { bars, direction } = mark {
                    rasterize_bars(
                        &mut buf, bars, *direction, opts, gutter, top, plot_w, plot_h,
                    );
                }
            }
        }
        _ => rasterize_xy(&mut buf, &scene.marks, opts, gutter, top, plot_w, plot_h),
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

#[allow(clippy::too_many_arguments)]
fn rasterize_bars(
    buf: &mut Buffer,
    bars: &[crate::scene::Bar],
    direction: BarDirection,
    opts: &RasterOptions,
    gutter: usize,
    top: usize,
    plot_w: usize,
    plot_h: usize,
) {
    match opts.bar_style {
        BarStyle::Dots => {
            let mut canvas = PixelCanvas::new(plot_w, plot_h, opts.marker);
            match direction {
                BarDirection::Vertical => {
                    // Verbatim from the pre-generalization single-direction path:
                    // its exact rounding order (round(h*ph) then fill ph-level..ph)
                    // must be preserved subpixel-for-subpixel.
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
                }
                BarDirection::Horizontal => {
                    // Length anchored at the rounded value extent; bar rows are
                    // exact cell multiples by construction, so the y span is exact.
                    for bar in bars {
                        let px_lo = (bar.x0 * plot_w as f64).round() as i64 * 2;
                        let px_hi = ((bar.x0 + bar.w) * plot_w as f64).round() as i64 * 2;
                        let py_lo = (bar.y0 * plot_h as f64).round() as i64 * 4;
                        let py_hi = ((bar.y0 + bar.h) * plot_h as f64).round() as i64 * 4;
                        for px in px_lo..px_hi {
                            for py in py_lo..py_hi {
                                canvas.set(px, py, bar.color);
                            }
                        }
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
        BarStyle::Blocks => match direction {
            BarDirection::Vertical => {
                // Verbatim bottom-up eighths fill from the pre-generalization path.
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
            BarDirection::Horizontal => {
                // Left-anchored eighths fill: full columns get `█`, the fractional
                // end column gets the left-eighth glyph for the remainder. Bar rows
                // are exact cell multiples, so the y span rounds cleanly.
                for bar in bars {
                    let x0 = (bar.x0 * plot_w as f64).round() as usize;
                    let r0 = (bar.y0 * plot_h as f64).round() as usize;
                    let r1 = ((bar.y0 + bar.h) * plot_h as f64).round() as usize;
                    let level = (bar.w * (plot_w * 8) as f64).round() as i64;
                    for c in 0..plot_w {
                        let fill = level - (c * 8) as i64;
                        if fill <= 0 {
                            continue;
                        }
                        let ch = if fill >= 8 {
                            '█'
                        } else {
                            LEFT_EIGHTHS[fill as usize]
                        };
                        for r in r0..r1 {
                            buf.set(gutter + 1 + x0 + c, top + r, ch, Some(bar.color));
                        }
                    }
                }
            }
        },
    }
}

/// Rasterize line/point/area marks into one shared pixel canvas, then blit it
/// into the plot area. A mark point is `[frac_x, frac_y]`; frac_y was already
/// flipped by the compiler (0 = top), so both axes map the same way:
/// `px = round(frac_x * (pixel_w - 1))`, `py = round(frac_y * (pixel_h - 1))`.
/// The grid is 2×4 pixels per cell for braille AND octant markers alike.
fn rasterize_xy(
    buf: &mut Buffer,
    marks: &[SceneMark],
    opts: &RasterOptions,
    gutter: usize,
    top: usize,
    plot_w: usize,
    plot_h: usize,
) {
    let mut canvas = PixelCanvas::new(plot_w, plot_h, opts.marker);
    let (pw, ph) = (canvas.pixel_width() as i64, canvas.pixel_height() as i64);
    let px = |fx: f64| (fx * (pw - 1) as f64).round() as i64;
    let py = |fy: f64| (fy * (ph - 1) as f64).round() as i64;

    for mark in marks {
        let (series, points, fill, points_mark) = match mark {
            SceneMark::Points { series, points } => (series, points, false, true),
            SceneMark::Path { series, points } => (series, points, false, false),
            SceneMark::Fill { series, points } => (series, points, true, false),
            SceneMark::Bars { .. } => continue,
        };
        let color = series.color;

        if points_mark {
            // 2×2 dot square centred on each pixel.
            for p in points {
                let (cx, cy) = (px(p[0]), py(p[1]));
                for dx in 0..2 {
                    for dy in 0..2 {
                        canvas.set(cx + dx, cy + dy, color);
                    }
                }
            }
            continue;
        }

        // Area fill first: per column, interpolate the top edge and fill down
        // to the pixel-grid bottom, so the line lands on top of the fill.
        if fill {
            for w in points.windows(2) {
                let (x0, y0, x1, y1) = (px(w[0][0]), py(w[0][1]), px(w[1][0]), py(w[1][1]));
                for x in x0..=x1 {
                    let t = if x1 == x0 {
                        0.0
                    } else {
                        (x - x0) as f64 / (x1 - x0) as f64
                    };
                    let ytop = (y0 as f64 + t * (y1 - y0) as f64).round() as i64;
                    for yy in ytop..ph {
                        canvas.set(x, yy, color);
                    }
                }
            }
        }
        // A lone point has no window to draw; stamp two horizontal dots.
        if points.len() == 1 {
            let (cx, cy) = (px(points[0][0]), py(points[0][1]));
            canvas.set(cx, cy, color);
            canvas.set(cx + 1, cy, color);
        }
        for w in points.windows(2) {
            canvas.line(px(w[0][0]), py(w[0][1]), px(w[1][0]), py(w[1][1]), color);
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

    /// Rasterize a single bar into a fresh buffer and return the plot rows as
    /// strings. Column 0 is the (empty) gutter — `rasterize_bars` draws marks at
    /// `gutter + 1 + cx`, so with `gutter = 0` bars start at column 1 and every
    /// row carries one leading gutter space.
    fn bar_rows(
        direction: BarDirection,
        bar_style: BarStyle,
        plot_w: usize,
        plot_h: usize,
        bar: crate::scene::Bar,
    ) -> Vec<String> {
        let mut buf = Buffer::new(plot_w + 1, plot_h);
        let opts = RasterOptions {
            marker: Marker::Octant,
            bar_style,
            color: false,
        };
        rasterize_bars(&mut buf, &[bar], direction, &opts, 0, 0, plot_w, plot_h);
        let out = buf.to_ansi(false);
        let mut rows: Vec<String> = out.split('\n').map(str::to_string).collect();
        rows.pop(); // trailing newline after the last row
        rows
    }

    #[test]
    fn rasterize_bars_glyph_rows() {
        use crate::scene::Bar;
        use BarDirection::{Horizontal, Vertical};

        struct Case {
            name: &'static str,
            direction: BarDirection,
            style: BarStyle,
            plot_w: usize,
            plot_h: usize,
            bar: Bar,
            expected: &'static [&'static str],
        }

        let color = Rgb(1, 2, 3);
        let cases = [
            // Vertical blocks: h = 0.75 over 2 rows → 12 eighths. Bottom row full
            // (`█`), top row 4 eighths from the bottom (`▄`).
            Case {
                name: "vertical blocks",
                direction: Vertical,
                style: BarStyle::Blocks,
                plot_w: 1,
                plot_h: 2,
                bar: Bar {
                    x0: 0.0,
                    y0: 0.25,
                    w: 1.0,
                    h: 0.75,
                    color,
                },
                expected: &[" ▄", " █"],
            },
            // Horizontal blocks: w = 0.75 over 2 cols → 12 eighths. Left col full
            // (`█`), end col 4 eighths from the left (`▌`).
            Case {
                name: "horizontal blocks",
                direction: Horizontal,
                style: BarStyle::Blocks,
                plot_w: 2,
                plot_h: 1,
                bar: Bar {
                    x0: 0.0,
                    y0: 0.0,
                    w: 0.75,
                    h: 1.0,
                    color,
                },
                expected: &[" █▌"],
            },
            // Vertical dots: h = 0.5 over 2 cells fills the bottom cell fully (`█`),
            // top cell empty.
            Case {
                name: "vertical dots",
                direction: Vertical,
                style: BarStyle::Dots,
                plot_w: 1,
                plot_h: 2,
                bar: Bar {
                    x0: 0.0,
                    y0: 0.5,
                    w: 1.0,
                    h: 0.5,
                    color,
                },
                expected: &["", " █"],
            },
            // Horizontal dots: w = 1.0 over 2 cells fills both cells fully (`██`).
            Case {
                name: "horizontal dots",
                direction: Horizontal,
                style: BarStyle::Dots,
                plot_w: 2,
                plot_h: 1,
                bar: Bar {
                    x0: 0.0,
                    y0: 0.0,
                    w: 1.0,
                    h: 1.0,
                    color,
                },
                expected: &[" ██"],
            },
        ];

        for case in cases {
            let rows = bar_rows(
                case.direction,
                case.style,
                case.plot_w,
                case.plot_h,
                case.bar,
            );
            assert_eq!(rows, case.expected, "case: {}", case.name);
        }
    }
}
