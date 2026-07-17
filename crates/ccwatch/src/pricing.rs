//! Per-model pricing and metadata.
//!
//! Prices are USD per **million** tokens. Adjust as Anthropic updates pricing.

use crate::jsonl::Usage;
use ratatui::style::Color;

/// Anthropic model family. Determines pricing and the default UI accent color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    Opus,
    Sonnet,
    Haiku,
}

/// Resolved metadata about the model behind a JSONL session, derived from the
/// model name string. Carries everything downstream code needs (pricing,
/// context window, accent color) without re-parsing the raw name repeatedly.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ModelInfo {
    pub family: Family,
    pub long_context: bool,
}

impl ModelInfo {
    /// Classify a raw model identifier (e.g. `claude-opus-4-7[1m]`) into a
    /// `Family` and detect the 1M-context variant marker. Falls back to
    /// `Sonnet` (the default tier) when the family is unrecognized.
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

    /// Public per-model rate card (USD per million tokens) covering input,
    /// output, cache writes (5min and 1hr ephemeral) and cache reads.
    /// Cache write rates: 5min ephemeral = 1.25× input, 1hr ephemeral = 2.00× input.
    /// Cache read = 0.10× input. (Anthropic public pricing as of 2026-07; the
    /// current Opus tier is $5/$25 in/out, not the legacy $15/$75.)
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

    /// Maximum context window in tokens for this model. Defaults to 1M for
    /// every family because Claude Code now defaults to the 1M-context tier
    /// and JSONL doesn't surface the variant on assistant events. Users on
    /// the 200k tier can opt down with `CCWATCH_CONTEXT_WINDOW=200000`.
    #[must_use]
    pub fn context_window(self) -> u64 {
        // `long_context` is still consulted by `pricing()` (1M-tier billing
        // differs above 200k input), but the gauge denominator is uniform.
        let _ = self.long_context;
        1_000_000
    }

    /// Family-coded accent color used in the header `model:` field so each
    /// model tier is visually distinguishable at a glance.
    #[must_use]
    pub fn color(self) -> Color {
        match self.family {
            Family::Opus => Color::Magenta,
            Family::Sonnet => Color::Blue,
            Family::Haiku => Color::Cyan,
        }
    }
}

impl Default for ModelInfo {
    /// Defaults to Sonnet at 200k. Used as the initial value before any
    /// assistant event has been ingested (model name unknown yet).
    fn default() -> Self {
        Self {
            family: Family::Sonnet,
            long_context: false,
        }
    }
}

/// Per-million-token rate card for a single model. All fields are USD/M.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_write_5m_per_mtok: f64,
    pub cache_write_1h_per_mtok: f64,
    pub cache_read_per_mtok: f64,
}

impl Pricing {
    /// Compute the dollar cost of a single assistant turn given its `Usage`.
    /// Splits `cache_creation` tokens into 5min vs 1hr buckets when the
    /// breakdown is present (Anthropic charges 1hr writes at 2× input vs
    /// 1.25× for 5min). Falls back to treating all cache writes as 5min when
    /// the breakdown is absent in the JSONL.
    #[must_use]
    #[allow(clippy::similar_names)] // 5m / 1h pair is the natural cache TTL distinction
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

    fn one_million(field: fn(&mut Usage)) -> Usage {
        let mut u = Usage::default();
        field(&mut u);
        u
    }

    #[test]
    fn detects_family_from_model_name() {
        assert_eq!(ModelInfo::parse("claude-opus-4-7").family, Family::Opus);
        assert_eq!(ModelInfo::parse("claude-sonnet-4-6").family, Family::Sonnet);
        assert_eq!(ModelInfo::parse("haiku").family, Family::Haiku);
        // Unknown falls back to Sonnet (the default tier).
        assert_eq!(ModelInfo::parse("foobar").family, Family::Sonnet);
    }

    #[test]
    fn detects_long_context_variants() {
        assert!(ModelInfo::parse("claude-opus-4-7[1m]").long_context);
        assert!(ModelInfo::parse("claude-sonnet-4-6-1m").long_context);
        assert!(ModelInfo::parse("CLAUDE-OPUS-4-7[1M]").long_context);
        assert!(!ModelInfo::parse("claude-sonnet-4-6").long_context);
    }

    #[test]
    fn context_window_defaults_to_1m_for_all_variants() {
        let normal = ModelInfo {
            family: Family::Opus,
            long_context: false,
        };
        let long = ModelInfo {
            family: Family::Opus,
            long_context: true,
        };
        assert_eq!(normal.context_window(), 1_000_000);
        assert_eq!(long.context_window(), 1_000_000);
    }

    #[test]
    fn opus_costs_more_than_sonnet_for_same_usage() {
        let usage = one_million(|u| u.input_tokens = 1_000_000);
        let opus = ModelInfo::parse("opus").pricing().cost_usd(&usage);
        let sonnet = ModelInfo::parse("sonnet").pricing().cost_usd(&usage);
        assert!(opus > sonnet, "opus {opus} should exceed sonnet {sonnet}");
    }

    #[test]
    fn output_costs_more_than_input() {
        let p = ModelInfo::parse("sonnet").pricing();
        let only_input = one_million(|u| u.input_tokens = 1_000_000);
        let only_output = one_million(|u| u.output_tokens = 1_000_000);
        assert!(p.cost_usd(&only_output) > p.cost_usd(&only_input));
    }

    #[test]
    fn cache_read_is_cheapest() {
        let p = ModelInfo::parse("sonnet").pricing();
        let only_input = one_million(|u| u.input_tokens = 1_000_000);
        let only_read = one_million(|u| u.cache_read_input_tokens = 1_000_000);
        assert!(p.cost_usd(&only_read) < p.cost_usd(&only_input));
    }

    #[test]
    fn haiku_is_cheaper_than_sonnet() {
        let usage = one_million(|u| u.output_tokens = 1_000_000);
        let haiku = ModelInfo::parse("haiku").pricing().cost_usd(&usage);
        let sonnet = ModelInfo::parse("sonnet").pricing().cost_usd(&usage);
        assert!(haiku < sonnet);
    }

    #[test]
    fn zero_usage_costs_nothing() {
        let p = ModelInfo::parse("opus").pricing();
        assert!(p.cost_usd(&Usage::default()).abs() < f64::EPSILON);
    }

    #[test]
    fn one_hour_cache_write_costs_more_than_five_minute() {
        use crate::jsonl::CacheCreation;
        let p = ModelInfo::parse("opus").pricing();
        let five_min = Usage {
            cache_creation_input_tokens: 1_000_000,
            cache_creation: Some(CacheCreation {
                ephemeral_5m_input_tokens: 1_000_000,
                ephemeral_1h_input_tokens: 0,
            }),
            ..Default::default()
        };
        let one_hour = Usage {
            cache_creation_input_tokens: 1_000_000,
            cache_creation: Some(CacheCreation {
                ephemeral_5m_input_tokens: 0,
                ephemeral_1h_input_tokens: 1_000_000,
            }),
            ..Default::default()
        };
        // Opus: 5m write = $6.25, 1h write = $10.00 per million.
        assert!((p.cost_usd(&five_min) - 6.25).abs() < 0.01);
        assert!((p.cost_usd(&one_hour) - 10.0).abs() < 0.01);
    }

    #[test]
    fn missing_cache_creation_breakdown_falls_back_to_five_minute() {
        let p = ModelInfo::parse("opus").pricing();
        let usage = Usage {
            cache_creation_input_tokens: 1_000_000,
            cache_creation: None, // legacy / missing
            ..Default::default()
        };
        // Should treat all as 5min: $6.25/M
        assert!((p.cost_usd(&usage) - 6.25).abs() < 0.01);
    }

    #[test]
    fn family_color_distinct() {
        assert_eq!(ModelInfo::parse("opus").color(), Color::Magenta);
        assert_eq!(ModelInfo::parse("sonnet").color(), Color::Blue);
        assert_eq!(ModelInfo::parse("haiku").color(), Color::Cyan);
    }
}
