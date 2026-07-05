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

    /// Like `nice_from`, but for a y axis drawn on `rows` terminal rows: the
    /// step is coarsened up the 1/2/5 ladder until the tick intervals divide
    /// the row intervals exactly, so every tick lands on a uniformly spaced
    /// integer row at least 2 rows from its neighbor.
    ///
    /// Termination: k strictly shrinks as the step coarsens, and k = 1 always
    /// passes — but a domain straddling zero pins k at 2 forever (it nices to
    /// lo = -step, hi = step at every rung), so k <= 2 without alignment falls
    /// back to a single interval: the domain niced at the FINEST step (minimal
    /// inflation), its two endpoints the only ticks.
    #[allow(dead_code)] // wired into the render path in a later task
    pub fn row_aligned(
        mut min: f64,
        mut max: f64,
        target_ticks: usize,
        rows: usize,
        include_zero: bool,
    ) -> Self {
        if include_zero {
            min = min.min(0.0);
            max = max.max(0.0);
        }
        if !(max - min).is_normal() {
            max = min + 1.0;
        }
        let intervals = rows.max(3) - 1;
        let step0 = nice_num((max - min) / (target_ticks.max(2) - 1) as f64, true);
        let mut step = step0;
        loop {
            let lo = (min / step).floor() * step;
            let hi = (max / step).ceil() * step;
            let k = ((hi - lo) / step).round() as usize;
            if k >= 1 && intervals.is_multiple_of(k) && intervals / k >= 2 {
                return Linear {
                    min: lo,
                    max: hi,
                    step,
                };
            }
            if k <= 2 {
                let lo = (min / step0).floor() * step0;
                let hi = (max / step0).ceil() * step0;
                return Linear {
                    min: lo,
                    max: hi,
                    step: hi - lo,
                };
            }
            step = next_nice(step);
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

/// The next step up the 1/2/5 ladder (1 -> 2 -> 5 -> 10 -> 20 ...).
#[allow(dead_code)] // used by `row_aligned`, wired into render in a later task
fn next_nice(step: f64) -> f64 {
    let exp = step.log10().floor();
    let pow = 10f64.powf(exp);
    let f = (step / pow).round();
    if f < 2.0 {
        2.0 * pow
    } else if f < 5.0 {
        5.0 * pow
    } else {
        10.0 * pow
    }
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

    #[test]
    fn row_aligned_keeps_fine_step_when_even() {
        // 12 intervals, k=6 divides: step 1 survives, all 7 labels.
        let s = Linear::row_aligned(0.0, 6.0, 6, 13, true);
        assert_eq!((s.min, s.max, s.step), (0.0, 6.0, 1.0));
    }

    #[test]
    fn row_aligned_coarsens_to_divide_rows() {
        // 9 intervals: k=6 fails (9 % 6 != 0), step 2 gives k=3, spacing 3.
        let s = Linear::row_aligned(0.0, 6.0, 6, 10, true);
        assert_eq!((s.min, s.max, s.step), (0.0, 6.0, 2.0));
    }

    #[test]
    fn row_aligned_climbs_ladder_past_domain_inflation() {
        // h7: 6 intervals. step 50 -> k=4 (no), step 100 -> k=2, spacing 3.
        let s = Linear::row_aligned(0.0, 160.0, 6, 7, true);
        assert_eq!((s.min, s.max, s.step), (0.0, 200.0, 100.0));
    }

    #[test]
    fn row_aligned_min_spacing_forces_fallback() {
        // h6: 5 intervals (prime). step 2 -> k=5, spacing 1 (rejected);
        // step 5 -> k=2 (5 % 2 != 0, k <= 2): single-interval fallback.
        let s = Linear::row_aligned(0.0, 10.0, 6, 6, true);
        assert_eq!((s.min, s.max, s.step), (0.0, 10.0, 10.0));
        assert_eq!(s.ticks(), vec![0.0, 10.0]);
    }

    #[test]
    fn row_aligned_zero_straddle_terminates() {
        // THE HANG CASE: a domain straddling zero nices to lo=-step, hi=step
        // at every coarser rung — k pins at 2, and 5 intervals never divide
        // by 2. The k<=2 fallback must fire: single interval over the domain
        // niced at the FINEST step (minimal inflation), two ticks.
        let s = Linear::row_aligned(-20.0, 30.0, 6, 6, false);
        assert_eq!((s.min, s.max, s.step), (-20.0, 30.0, 50.0));
        assert_eq!(s.ticks(), vec![-20.0, 30.0]);
    }

    #[test]
    fn row_aligned_negative_domain_no_zero() {
        // step 10 -> k=5 (12 % 5 != 0); step 20 -> k=3, spacing 4.
        let s = Linear::row_aligned(-20.0, 30.0, 6, 13, false);
        assert_eq!((s.min, s.max, s.step), (-20.0, 40.0, 20.0));
    }

    #[test]
    fn row_aligned_fractional_step() {
        // step 0.5 -> domain 0..2, k=4, 12 % 4 = 0, spacing 3.
        let s = Linear::row_aligned(0.0, 1.7, 6, 13, true);
        assert_eq!((s.min, s.max, s.step), (0.0, 2.0, 0.5));
    }
}
