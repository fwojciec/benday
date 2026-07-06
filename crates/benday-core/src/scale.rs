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

// Bin selection is the pure-math foundation for histograms; the compile path
// wires it in a later task, so it is unreachable within the crate until then.
/// A resolved bin layout: `n` bins of width `step` from `lo`.
/// Every edge is `lo + k*step`; edges are nice numbers unless the
/// caller forced `step`.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct Bins {
    pub lo: f64,
    pub step: f64,
    pub n: usize,
}

#[allow(dead_code)]
impl Bins {
    pub fn hi(&self) -> f64 {
        self.lo + self.step * self.n as f64
    }

    /// Half-open `[edge, next)`; the final bin is closed so `x == hi` lands in
    /// it rather than falling off the top.
    pub fn index(&self, x: f64) -> usize {
        let k = ((x - self.lo) / self.step).floor();
        if k <= 0.0 {
            0
        } else if k as usize >= self.n {
            self.n - 1
        } else {
            k as usize
        }
    }
}

/// Snap `[min, max]` outward to whole multiples of `step` and count the bins.
#[allow(dead_code)]
fn expand(min: f64, max: f64, step: f64) -> Bins {
    let lo = (min / step).floor() * step;
    let hi = (max / step).ceil() * step;
    Bins {
        lo,
        step,
        n: ((hi - lo) / step).round() as usize,
    }
}

/// Automatic bins: size a step off the same 1/2/5 ladder the y axis walks so
/// the data splits into roughly `target` bins, then snap the domain to nice
/// edges. Rounding the step UP keeps the count at or under `target` before the
/// snap, so the final count stays within a bin or two of `target`.
#[allow(dead_code)]
pub fn bins_auto(min: f64, mut max: f64, target: usize) -> Bins {
    debug_assert!(target >= 1);
    debug_assert!(min <= max);
    if !(max - min).is_normal() {
        max = min + 1.0;
    }
    expand(min, max, nice_num((max - min) / target as f64, false))
}

/// Like `bins_auto` with `target = n`, then coarsen up the ladder until the
/// count fits under `n`. A domain straddling zero can never fall below two
/// bins — one on each side — so it stops there instead of looping forever.
#[allow(dead_code)]
pub fn bins_maxbins(min: f64, mut max: f64, n: usize) -> Bins {
    debug_assert!(n >= 1);
    debug_assert!(min <= max);
    if !(max - min).is_normal() {
        max = min + 1.0;
    }
    let mut step = nice_num((max - min) / n as f64, false);
    let mut bins = expand(min, max, step);
    while bins.n > n {
        if min < 0.0 && max > 0.0 && bins.n <= 2 {
            break;
        }
        step = next_nice(step);
        bins = expand(min, max, step);
    }
    bins
}

/// Bins of the caller's exact width, the domain floored/ceiled to `step`
/// multiples so every edge is a multiple of `step`.
#[allow(dead_code)]
pub fn bins_step(min: f64, mut max: f64, step: f64) -> Bins {
    debug_assert!(step > 0.0);
    debug_assert!(min <= max);
    if !(max - min).is_normal() {
        max = min + 1.0;
    }
    expand(min, max, step)
}

/// `n + 1` integer cell edges tiling a `plot_w`-column plot, each edge
/// `round(k/n * plot_w)`. Rounding the shared edges — not each bar's origin
/// and width independently — is what makes adjacent bars tile with no gap or
/// overrun.
#[allow(dead_code)]
pub fn cell_edges(n: usize, plot_w: usize) -> Vec<usize> {
    debug_assert!(n >= 1);
    (0..=n)
        .map(|k| (k as f64 / n as f64 * plot_w as f64).round() as usize)
        .collect()
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

    #[test]
    fn bins_auto_sweep_stays_nice() {
        // Across magnitudes, signs, and target counts: every layout is a
        // 1/2/5 ladder step, its edges are step multiples covering the data,
        // and the count lands in [2, target+2] (snapping may add a bin or two).
        let spans = [
            (0.0, 0.001),
            (0.0, 1.0),
            (0.0, 100.0),
            (0.0, 1e9),
            (-50.0, 50.0),
            (-1000.0, -10.0),
            (3.0, 97.0),
            (-0.001, 0.002),
            (12_345.0, 67_890.0),
        ];
        for &(min, max) in &spans {
            for target in 5..=20 {
                let b = bins_auto(min, max, target);
                let mantissa = b.step / 10f64.powf(b.step.log10().floor());
                assert!(
                    [1.0, 2.0, 5.0, 10.0]
                        .iter()
                        .any(|m| (mantissa - m).abs() < 1e-6),
                    "step {} is not 1/2/5x10^k (min={min}, max={max}, target={target})",
                    b.step
                );
                let edge = b.lo / b.step;
                assert!(
                    (edge - edge.round()).abs() < 1e-6,
                    "lo {} is not a multiple of step {}",
                    b.lo,
                    b.step
                );
                assert!(b.lo <= min + 1e-6 * b.step, "lo {} > min {min}", b.lo);
                assert!(b.hi() >= max - 1e-6 * b.step, "hi {} < max {max}", b.hi());
                assert!(
                    b.n >= 2 && b.n <= target + 2,
                    "count {} out of [2, {}] (min={min}, max={max})",
                    b.n,
                    target + 2
                );
            }
        }
    }

    #[test]
    fn bins_degenerate_span_contains_the_value() {
        // min == max: the Linear::nice_from guard treats it as [min, min+1], so
        // selection still returns at least one bin holding the value.
        let b = bins_auto(7.3, 7.3, 8);
        assert!(b.n >= 1);
        assert!(b.lo <= 7.3 && 7.3 <= b.hi());
        assert!(b.index(7.3) < b.n);
    }

    #[test]
    fn bins_maxbins_coarsens_below_ceiling() {
        // target=9 nices to step 10, but floor/ceil push the count to 10 bins;
        // the ladder must climb one rung to step 20 (5 bins) to fit the ceiling.
        let b = bins_maxbins(5.0, 95.0, 9);
        assert!(b.n <= 9);
        assert_eq!((b.lo, b.step, b.n), (0.0, 20.0, 5));
    }

    #[test]
    fn bins_maxbins_never_exceeds_ceiling() {
        let spans = [
            (0.0, 0.001),
            (0.0, 100.0),
            (0.0, 1e9),
            (-50.0, 50.0),
            (-1000.0, -10.0),
            (3.0, 97.0),
        ];
        for &(min, max) in &spans {
            for n in 2..=20 {
                let b = bins_maxbins(min, max, n);
                assert!(
                    b.n <= n,
                    "count {} exceeds maxbins {n} (min={min}, max={max})",
                    b.n
                );
            }
        }
    }

    #[test]
    fn bins_maxbins_one_terminates() {
        // maxbins=1 across zero can't fall below two bins (one each side): it
        // must terminate at that floor rather than spin up the ladder forever.
        let b = bins_maxbins(-50.0, 50.0, 1);
        assert!(b.n <= 2);
        // a single-signed domain does collapse to one bin.
        assert_eq!(bins_maxbins(3.0, 97.0, 1).n, 1);
    }

    #[test]
    fn bins_step_uses_width_verbatim() {
        // step 10 over 3..97 -> lo=0, hi=100, n=10, edges exactly 0,10,...,100.
        let b = bins_step(3.0, 97.0, 10.0);
        assert_eq!((b.lo, b.step, b.n), (0.0, 10.0, 10));
        assert_eq!(b.hi(), 100.0);
        let edges: Vec<f64> = (0..=b.n).map(|k| b.lo + k as f64 * b.step).collect();
        assert_eq!(
            edges,
            vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0]
        );
    }

    #[test]
    fn bins_index_is_half_open_final_closed() {
        let b = bins_step(0.0, 100.0, 10.0);
        assert_eq!((b.n, b.hi()), (10, 100.0));
        assert_eq!(b.index(0.0), 0);
        // an interior edge belongs to the bin on its right
        assert_eq!(b.index(10.0), 1);
        assert_eq!(b.index(50.0), 5);
        // just under an edge stays in the left bin
        assert_eq!(b.index(9.999), 0);
        // the final bin is closed: hi is bin n-1, not bin n
        assert_eq!(b.index(100.0), 9);
        assert_eq!(b.index(99.999), 9);
    }

    #[test]
    fn cell_edges_tile_the_plot() {
        // The Codex counterexample: rounding the edges (not each bar's origin
        // and width) tiles [0,3,5,8,10] with no gap or overrun.
        assert_eq!(cell_edges(4, 10), vec![0, 3, 5, 8, 10]);
        for &(n, w) in &[(1usize, 1usize), (3, 7), (5, 5), (7, 100), (1, 0), (4, 3)] {
            let e = cell_edges(n, w);
            assert_eq!(e.len(), n + 1);
            assert_eq!(e[0], 0);
            assert_eq!(*e.last().unwrap(), w);
            assert!(e.windows(2).all(|p| p[0] <= p[1]), "not monotone: {e:?}");
        }
    }
}
