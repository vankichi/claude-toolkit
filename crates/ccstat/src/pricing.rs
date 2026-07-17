//! Per-model pricing and accent color. Ported from ccwatch; prices are USD
//! per **million** tokens (Anthropic public pricing as of 2026-07). The
//! current Opus tier (4.5+) is $5/$25 in/out — not the legacy $15/$75.

use crate::jsonl::Usage;
use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Opus,
    Sonnet,
    Haiku,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelInfo {
    pub family: Family,
    pub long_context: bool,
}

impl ModelInfo {
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let m = raw.to_ascii_lowercase();
        let family = if m.contains("opus") {
            Family::Opus
        } else if m.contains("haiku") {
            Family::Haiku
        } else {
            Family::Sonnet
        };
        let long_context = m.contains("[1m]") || m.contains("-1m");
        Self {
            family,
            long_context,
        }
    }

    #[must_use]
    pub fn pricing(self) -> Pricing {
        match self.family {
            Family::Opus => Pricing {
                input_per_mtok: 5.0,
                output_per_mtok: 25.0,
                cache_write_5m_per_mtok: 6.25,
                cache_write_1h_per_mtok: 10.0,
                cache_read_per_mtok: 0.50,
            },
            Family::Haiku => Pricing {
                input_per_mtok: 1.0,
                output_per_mtok: 5.0,
                cache_write_5m_per_mtok: 1.25,
                cache_write_1h_per_mtok: 2.0,
                cache_read_per_mtok: 0.10,
            },
            Family::Sonnet => Pricing {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
                cache_write_5m_per_mtok: 3.75,
                cache_write_1h_per_mtok: 6.0,
                cache_read_per_mtok: 0.30,
            },
        }
    }

    #[must_use]
    pub fn color(self) -> Color {
        match self.family {
            Family::Opus => Color::Magenta,
            Family::Sonnet => Color::Blue,
            Family::Haiku => Color::Cyan,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Pricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_write_5m_per_mtok: f64,
    pub cache_write_1h_per_mtok: f64,
    pub cache_read_per_mtok: f64,
}

impl Pricing {
    #[must_use]
    #[allow(clippy::similar_names)]
    pub fn cost_usd(&self, u: &Usage) -> f64 {
        const M: f64 = 1_000_000.0;
        let (write_5m, write_1h) = u.cache_creation_split();
        (u.input_tokens as f64) * self.input_per_mtok / M
            + (u.output_tokens as f64) * self.output_per_mtok / M
            + (write_5m as f64) * self.cache_write_5m_per_mtok / M
            + (write_1h as f64) * self.cache_write_1h_per_mtok / M
            + (u.cache_read_input_tokens as f64) * self.cache_read_per_mtok / M
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_family_and_defaults_to_sonnet() {
        assert_eq!(ModelInfo::parse("claude-opus-4-8").family, Family::Opus);
        assert_eq!(ModelInfo::parse("haiku").family, Family::Haiku);
        assert_eq!(ModelInfo::parse("foobar").family, Family::Sonnet);
    }

    #[test]
    fn opus_costs_more_than_sonnet_for_same_usage() {
        let u = Usage {
            input_tokens: 1_000_000,
            ..Default::default()
        };
        assert!(
            ModelInfo::parse("opus").pricing().cost_usd(&u)
                > ModelInfo::parse("sonnet").pricing().cost_usd(&u)
        );
    }

    #[test]
    fn one_hour_cache_write_costs_more_than_five_min() {
        let p = ModelInfo::parse("opus").pricing();
        let m5 = Usage {
            cache_creation_5m: 1_000_000,
            ..Default::default()
        };
        let h1 = Usage {
            cache_creation_1h: 1_000_000,
            ..Default::default()
        };
        assert!((p.cost_usd(&m5) - 6.25).abs() < 0.01);
        assert!((p.cost_usd(&h1) - 10.0).abs() < 0.01);
    }

    #[test]
    fn zero_usage_is_free() {
        assert!(
            ModelInfo::parse("opus")
                .pricing()
                .cost_usd(&Usage::default())
                .abs()
                < f64::EPSILON
        );
    }
}
