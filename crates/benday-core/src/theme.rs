//! Color themes, named after print processes. Gradients and palettes are
//! generated in OKLCH so ramps are perceptually uniform (no brightness wobble
//! mid-gradient like naive HSV interpolation).

use crate::raster::Rgb;

#[derive(Debug, Clone, Copy)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    /// Hue in degrees.
    pub h: f64,
}

impl Oklch {
    pub fn rgb(self) -> Rgb {
        let hr = self.h.to_radians();
        let (a, b) = (self.c * hr.cos(), self.c * hr.sin());
        let l_ = self.l + 0.3963377774 * a + 0.2158037573 * b;
        let m_ = self.l - 0.1055613458 * a - 0.0638541728 * b;
        let s_ = self.l - 0.0894841775 * a - 1.2914855480 * b;
        let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
        let r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
        let g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
        let bb = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;
        Rgb(gamma(r), gamma(g), gamma(bb))
    }
}

fn gamma(c: f64) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round() as u8
}

fn mix(a: Oklch, b: Oklch, t: f64) -> Oklch {
    let mut dh = b.h - a.h;
    if dh > 180.0 {
        dh -= 360.0;
    }
    if dh < -180.0 {
        dh += 360.0;
    }
    Oklch {
        l: a.l + (b.l - a.l) * t,
        c: a.c + (b.c - a.c) * t,
        h: a.h + dh * t,
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub axis: Rgb,
    pub title: Rgb,
    /// Single-series accent color.
    pub accent: Rgb,
    grad_ends: (Oklch, Oklch),
    pub palette: Vec<Rgb>,
}

impl Theme {
    /// Sample the theme gradient at t in [0, 1].
    pub fn grad(&self, t: f64) -> Rgb {
        mix(self.grad_ends.0, self.grad_ends.1, t.clamp(0.0, 1.0)).rgb()
    }

    pub fn series(&self, i: usize) -> Rgb {
        self.palette[i % self.palette.len()]
    }
}

pub const THEME_NAMES: &[&str] = &["benday", "lichtenstein", "rotogravure"];

pub fn by_name(name: &str) -> Option<Theme> {
    match name {
        // House style: cool teal-to-amber thermal ramp, restrained axes.
        "benday" => Some(Theme {
            name: "benday",
            axis: Rgb(106, 112, 122),
            title: Rgb(222, 226, 232),
            accent: Oklch {
                l: 0.80,
                c: 0.14,
                h: 210.0,
            }
            .rgb(),
            grad_ends: (
                Oklch {
                    l: 0.70,
                    c: 0.12,
                    h: 235.0,
                },
                Oklch {
                    l: 0.86,
                    c: 0.15,
                    h: 75.0,
                },
            ),
            palette: palette_from_hues(&[235.0, 155.0, 70.0, 330.0, 200.0, 20.0, 110.0, 285.0]),
        }),
        // Comic primaries: red, yellow, blue over warm gray.
        "lichtenstein" => Some(Theme {
            name: "lichtenstein",
            axis: Rgb(132, 120, 106),
            title: Rgb(235, 228, 216),
            accent: Oklch {
                l: 0.62,
                c: 0.22,
                h: 28.0,
            }
            .rgb(),
            grad_ends: (
                Oklch {
                    l: 0.58,
                    c: 0.22,
                    h: 28.0,
                },
                Oklch {
                    l: 0.88,
                    c: 0.16,
                    h: 95.0,
                },
            ),
            palette: vec![
                Oklch {
                    l: 0.62,
                    c: 0.22,
                    h: 28.0,
                }
                .rgb(),
                Oklch {
                    l: 0.88,
                    c: 0.16,
                    h: 95.0,
                }
                .rgb(),
                Oklch {
                    l: 0.55,
                    c: 0.19,
                    h: 262.0,
                }
                .rgb(),
                Oklch {
                    l: 0.42,
                    c: 0.08,
                    h: 262.0,
                }
                .rgb(),
            ],
        }),
        // Monochrome ink: lightness ramp, near-zero chroma.
        "rotogravure" => Some(Theme {
            name: "rotogravure",
            axis: Rgb(110, 110, 116),
            title: Rgb(228, 228, 232),
            accent: Oklch {
                l: 0.85,
                c: 0.02,
                h: 250.0,
            }
            .rgb(),
            grad_ends: (
                Oklch {
                    l: 0.42,
                    c: 0.02,
                    h: 250.0,
                },
                Oklch {
                    l: 0.95,
                    c: 0.02,
                    h: 250.0,
                },
            ),
            palette: vec![
                Oklch {
                    l: 0.90,
                    c: 0.02,
                    h: 250.0,
                }
                .rgb(),
                Oklch {
                    l: 0.70,
                    c: 0.02,
                    h: 250.0,
                }
                .rgb(),
                Oklch {
                    l: 0.52,
                    c: 0.02,
                    h: 250.0,
                }
                .rgb(),
                Oklch {
                    l: 0.36,
                    c: 0.02,
                    h: 250.0,
                }
                .rgb(),
            ],
        }),
        _ => None,
    }
}

fn palette_from_hues(hues: &[f64]) -> Vec<Rgb> {
    hues.iter()
        .map(|&h| {
            Oklch {
                l: 0.78,
                c: 0.13,
                h,
            }
            .rgb()
        })
        .collect()
}
