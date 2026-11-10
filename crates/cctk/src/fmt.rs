//! Number formatting shared by the tools' text and chart labels.

/// A token count as `999`, `13k` (rounded to the nearest thousand) or
/// `1.2M`.
#[must_use]
pub fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// [`tokens`] of a rate or peak held as `f64`, rounded to the nearest whole
/// token.
#[must_use]
pub fn tokens_f64(v: f64) -> String {
    // The callers' values come from non-negative token counts, so the cast can
    // neither lose a sign nor exceed `u64`.
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    tokens(v.round() as u64)
}

/// A dollar amount with cents, e.g. `$3.40`.
#[must_use]
pub fn usd(v: f64) -> String {
    format!("${v:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_f64_rounds_then_scales() {
        assert_eq!(tokens_f64(0.4), "0");
        assert_eq!(tokens_f64(999.6), "1k");
        assert_eq!(tokens_f64(61_873.6), "62k");
        assert_eq!(tokens_f64(1_234_567.0), "1.2M");
    }

    #[test]
    fn usd_has_cents() {
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(3.4), "$3.40");
        assert_eq!(usd(142.5), "$142.50");
    }

    #[test]
    fn tokens_scale_and_round() {
        assert_eq!(tokens(0), "0");
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1_000), "1k");
        assert_eq!(tokens(12_800), "13k");
        assert_eq!(tokens(61_874), "62k");
        assert_eq!(tokens(1_000_000), "1.0M");
        assert_eq!(tokens(1_234_567), "1.2M");
    }
}
