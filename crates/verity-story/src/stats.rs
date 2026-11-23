//! The statistics Frame Story needs (docs/07-frame-story.md, step 4).

/// P(X ≥ k) for X ~ Binomial(n, p), computed in log space so large `n` neither
/// overflows nor underflows to a wrong answer.
pub fn binomial_upper_tail(k: u64, n: u64, p: f64) -> f64 {
    if k == 0 {
        return 1.0;
    }
    if k > n || p <= 0.0 {
        return 0.0;
    }
    if p >= 1.0 {
        return 1.0;
    }
    let (lp, lq) = (p.ln(), (1.0 - p).ln());
    let ln_choose = |i: u64| ln_factorial(n) - ln_factorial(i) - ln_factorial(n - i);
    let terms: Vec<f64> = (k..=n)
        .map(|i| ln_choose(i) + i as f64 * lp + (n - i) as f64 * lq)
        .collect();
    let max = terms.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let sum: f64 = terms.iter().map(|t| (t - max).exp()).sum();
    (max + sum.ln()).exp().min(1.0)
}

/// ln(n!) by summation below 256 and Stirling's series above, accurate to
/// well under 1e-10 relative error over the range Frame Story uses.
fn ln_factorial(n: u64) -> f64 {
    if n < 256 {
        return (2..=n).map(|i| (i as f64).ln()).sum();
    }
    let x = n as f64;
    x * x.ln() - x + 0.5 * (2.0 * std::f64::consts::PI * x).ln() + 1.0 / (12.0 * x)
        - 1.0 / (360.0 * x.powi(3))
}

/// Holm–Bonferroni adjusted p-values, in the input order.
pub fn holm(p: &[f64]) -> Vec<f64> {
    let m = p.len();
    let mut order: Vec<usize> = (0..m).collect();
    order.sort_by(|&a, &b| p[a].total_cmp(&p[b]));
    let mut adjusted = vec![0.0; m];
    let mut running: f64 = 0.0;
    for (rank, &i) in order.iter().enumerate() {
        let v = ((m - rank) as f64 * p[i]).min(1.0);
        running = running.max(v);
        adjusted[i] = running;
    }
    adjusted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * b.abs().max(1e-300)
    }

    #[test]
    fn binomial_matches_hand_computed_values() {
        // n = 10, p = 0.5: P(X ≥ 8) = (45 + 10 + 1) / 1024.
        assert!(close(binomial_upper_tail(8, 10, 0.5), 56.0 / 1024.0, 1e-12));
        // n = 5, p = 0.1: P(X ≥ 1) = 1 − 0.9^5.
        assert!(close(
            binomial_upper_tail(1, 5, 0.1),
            1.0 - 0.9f64.powi(5),
            1e-12
        ));
        // n = 37, p = 0.05, k = 14: tiny but not zero.
        let p = binomial_upper_tail(14, 37, 0.05);
        assert!(p > 0.0 && p < 1e-8, "{p}");
        // Edges.
        assert_eq!(binomial_upper_tail(0, 10, 0.3), 1.0);
        assert_eq!(binomial_upper_tail(11, 10, 0.3), 0.0);
        assert_eq!(binomial_upper_tail(3, 10, 0.0), 0.0);
        assert_eq!(binomial_upper_tail(3, 10, 1.0), 1.0);
        // Large n stays finite and sensible: mean 500, P(X ≥ 500) ≈ 0.5.
        let half = binomial_upper_tail(500, 1000, 0.5);
        assert!(half > 0.5 && half < 0.52, "{half}");
    }

    #[test]
    fn stirling_agrees_with_summation_at_the_switch() {
        let summed: f64 = (2..=300u64).map(|i| (i as f64).ln()).sum();
        assert!(close(ln_factorial(300), summed, 1e-12));
    }

    #[test]
    fn holm_matches_hand_computed_values() {
        // m = 4: sorted 0.01, 0.02, 0.03, 0.04 → 0.04, 0.06, 0.06 (monotone), 0.06.
        let adj = holm(&[0.03, 0.01, 0.04, 0.02]);
        let want = [0.06, 0.04, 0.06, 0.06];
        for (a, w) in adj.iter().zip(want) {
            assert!(close(*a, w, 1e-12), "{adj:?}");
        }
        assert_eq!(holm(&[0.9, 0.8]), [1.0, 1.0]);
        assert!(holm(&[]).is_empty());
    }
}
