//! Character-grid buffer and ANSI serialization.
//!
//! Output is plain lines of text with inline 24-bit SGR sequences — no cursor
//! addressing, no alternate screen — so it survives pipes, transcripts, and
//! agent tool-call capture.

use crate::raster::Rgb;

#[derive(Debug, Clone, Copy)]
struct Cell {
    ch: char,
    fg: Option<Rgb>,
}

pub struct Buffer {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
}

impl Buffer {
    pub fn new(width: usize, height: usize) -> Self {
        Buffer {
            width,
            height,
            cells: vec![Cell { ch: ' ', fg: None }; width * height],
        }
    }

    pub fn set(&mut self, x: usize, y: usize, ch: char, fg: Option<Rgb>) {
        if x < self.width && y < self.height {
            self.cells[y * self.width + x] = Cell { ch, fg };
        }
    }

    /// Write a string starting at (x, y); clips at the right edge.
    pub fn text(&mut self, x: usize, y: usize, s: &str, fg: Option<Rgb>) {
        for (i, ch) in s.chars().enumerate() {
            self.set(x + i, y, ch, fg);
        }
    }

    pub fn to_ansi(&self, color: bool) -> String {
        let mut out = String::new();
        for y in 0..self.height {
            let row = &self.cells[y * self.width..(y + 1) * self.width];
            let end = row
                .iter()
                .rposition(|c| c.ch != ' ')
                .map(|i| i + 1)
                .unwrap_or(0);
            let mut current: Option<Rgb> = None;
            let mut colored = false;
            for cell in &row[..end] {
                if color && cell.ch != ' ' && cell.fg != current {
                    match cell.fg {
                        Some(Rgb(r, g, b)) => {
                            out.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
                            colored = true;
                        }
                        None => out.push_str("\x1b[39m"),
                    }
                    current = cell.fg;
                }
                out.push(cell.ch);
            }
            if colored {
                out.push_str("\x1b[0m");
            }
            out.push('\n');
        }
        out
    }
}
