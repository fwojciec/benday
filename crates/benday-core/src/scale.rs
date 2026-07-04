//! Continuous scales with Heckbert-style "nice" tick selection.

#[derive(Debug, Clone, Copy)]
pub struct Linear {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl Linear {
    /// Build a scale whose domain is expanded to nice tick boundaries.
    pub fn nice_from(mut min: f64, mut max: f64, target_ticks: usize, include_zero: bool) -> Self {
        if include_zero {
            min = min.min(0.0);
            max = max.max(0.0);
        }
        if !(max - min).is_normal() {
            max = min + 1.0;
        }
        let step = nice_num((max - min) / (target_ticks.max(2) - 1) as f64, true);
        Linear {
            min: (min / step).floor() * step,
            max: (max / step).ceil() * step,
            step,
        }
    }

    /// Identity-ish scale over category indices 0..n-1 (no nice expansion).
    pub fn indices(n: usize) -> Self {
        Linear {
            min: 0.0,
            max: (n.saturating_sub(1)).max(1) as f64,
            step: 1.0,
        }
    }

    pub fn ticks(&self) -> Vec<f64> {
        let n = ((self.max - self.min) / self.step).round() as usize;
        (0..=n).map(|i| self.min + i as f64 * self.step).collect()
    }

    /// Normalize into [0, 1].
    pub fn norm(&self, v: f64) -> f64 {
        (v - self.min) / (self.max - self.min)
    }
}

/// Round `x` to a "nice" number (1/2/5 times a power of ten).
fn nice_num(x: f64, round: bool) -> f64 {
    let exp = x.log10().floor();
    let pow = 10f64.powf(exp);
    let f = x / pow;
    let nf = if round {
        if f < 1.5 {
            1.0
        } else if f < 3.0 {
            2.0
        } else if f < 7.0 {
            5.0
        } else {
            10.0
        }
    } else if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    nf * pow
}

/// Format a tick value compactly: step-derived decimals, k/M/G above 10 000.
pub fn fmt_tick(v: f64, step: f64) -> String {
    let av = v.abs();
    if av >= 1e9 {
        return trim_zeros(format!("{:.1}", v / 1e9)) + "G";
    }
    if av >= 1e6 {
        return trim_zeros(format!("{:.1}", v / 1e6)) + "M";
    }
    if av >= 1e4 {
        return trim_zeros(format!("{:.1}", v / 1e3)) + "k";
    }
    let decimals = if step >= 1.0 {
        0
    } else {
        (-step.log10().floor()) as usize
    };
    format!("{v:.decimals$}")
}

fn trim_zeros(s: String) -> String {
    if s.ends_with(".0") {
        s[..s.len() - 2].to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_ticks_cover_domain() {
        let s = Linear::nice_from(0.0, 9.4, 5, true);
        assert_eq!(s.min, 0.0);
        assert_eq!(s.max, 10.0);
        assert_eq!(s.ticks(), vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
    }

    #[test]
    fn fractional_steps_get_decimals() {
        let s = Linear::nice_from(0.0, 1.7, 5, true);
        assert!(s.step < 1.0);
        assert_eq!(fmt_tick(0.5, s.step), "0.5");
    }

    #[test]
    fn large_values_humanize() {
        assert_eq!(fmt_tick(2_500_000.0, 500_000.0), "2.5M");
        assert_eq!(fmt_tick(40_000.0, 10_000.0), "40k");
    }
}
