//! benday-core: terminal charts from a Vega-Lite-style JSON spec.
//!
//! Pure spec-in, string-out — no I/O, no TTY detection, no environment
//! sniffing — so it can sit under a CLI, an MCP server, or another Rust
//! program unchanged.

mod ansi;
mod data;
pub mod error;
mod raster;
mod render;
mod scale;
pub mod spec;
pub mod theme;

pub use error::Error;
pub use raster::{Marker, Rgb};
pub use render::{render, BarStyle, RenderOptions, Rendered};
