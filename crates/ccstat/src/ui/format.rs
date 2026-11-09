//! Terminal-free formatting and color assignment shared by the renderers.

use crate::model::Category;
use crate::usage::Row;
use ccmap::model::Provenance;
use chrono::NaiveDate;
use ratatui::style::Color;

/// Human-readable recency: "never", "today", "1d ago", "5d ago".
#[must_use]
pub fn format_recency(last_used: Option<NaiveDate>, today: NaiveDate) -> String {
    match last_used {
        None => "never".to_string(),
        Some(d) => {
            let days = (today - d).num_days();
            match days {
                n if n <= 0 => "today".to_string(),
                1 => "1d ago".to_string(),
                n => format!("{n}d ago"),
            }
        }
    }
}

/// Row color for a provenance (mirrors ccmap's provenance palette); `None`
/// (built-in / undiscovered) is dim gray.
#[must_use]
pub(super) fn provenance_color(provenance: Option<Provenance>) -> Color {
    match provenance {
        Some(Provenance::Local) => Color::White,
        Some(Provenance::Project) => Color::Cyan,
        Some(Provenance::Official) => Color::Yellow,
        Some(Provenance::Community) => Color::Magenta,
        None => Color::DarkGray,
    }
}

/// Human-readable source label for the detail pane.
#[must_use]
pub(super) fn source_label(provenance: Option<Provenance>) -> &'static str {
    match provenance {
        Some(Provenance::Local) => "Local (~/.claude)",
        Some(Provenance::Project) => "Project",
        Some(Provenance::Official) => "Official plugin",
        Some(Provenance::Community) => "Community plugin",
        None => "built-in / unknown",
    }
}

/// Detail-pane text for the selected row. Pure and terminal-free.
#[must_use]
pub fn detail_lines(row: &Row, category: Category, today: NaiveDate) -> Vec<String> {
    let mut lines = vec![
        format!("{}: {}", category.title(), row.name),
        String::new(),
        format!(
            "count {} · last {} · first {}",
            row.count,
            format_recency(row.last_used, today),
            row.first_used
                .map_or_else(|| "—".to_string(), |d| d.format("%Y-%m-%d").to_string()),
        ),
    ];

    if category == Category::Model && row.count > 0 {
        lines.push(String::new());
        lines.push(format!(
            "tokens: in {} · out {} · cache-write {} · cache-read {}",
            row.input, row.output, row.cache_creation, row.cache_read
        ));
        lines.push(format!("est. cost: ${:.2}", row.cost_usd));
    }

    if !row.by_project.is_empty() {
        lines.push(String::new());
        lines.push("by project:".to_string());
        for (proj, n) in row.by_project.iter().take(8) {
            lines.push(format!("  {proj}  {n}"));
        }
    }

    lines
}

/// Distinct, readable line/label colors. Assigned per series (or per model, by
/// a stable name ordering) so overlaid lines and rows stay visually separable.
pub(super) const PALETTE: [Color; 8] = [
    Color::Magenta,
    Color::Cyan,
    Color::Green,
    Color::Yellow,
    Color::Red,
    Color::Blue,
    Color::LightGreen,
    Color::LightMagenta,
];

/// Stable distinct color for a model `name`: its slot in the alphabetically
/// ordered set of the current model names, mapped onto [`PALETTE`]. Stable
/// across refreshes (independent of usage rank) so a model keeps its color, and
/// distinct up to the palette size — unlike family colors, this separates
/// variants (opus-4-8 vs opus-4-7) and unknown families (fable) too.
pub(super) fn model_color(rows: &[Row], name: &str) -> Color {
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    color_for_name(&names, name)
}

/// The palette color for `name` by its slot in the alphabetically sorted,
/// de-duplicated `names`. Distinct up to the palette size, stable for a fixed
/// name set. Public so `cctop` colors its per-model lines the same way.
#[must_use]
pub fn color_for_name(names: &[&str], name: &str) -> Color {
    let mut sorted: Vec<&str> = names.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let idx = sorted.iter().position(|&n| n == name).unwrap_or(0);
    PALETTE[idx % PALETTE.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::day;
    use crate::usage::TREND_DAYS;

    #[test]
    fn provenance_color_maps_each_case() {
        use ccmap::model::Provenance;
        assert_eq!(
            super::provenance_color(Some(Provenance::Local)),
            Color::White
        );
        assert_eq!(
            super::provenance_color(Some(Provenance::Project)),
            Color::Cyan
        );
        assert_eq!(
            super::provenance_color(Some(Provenance::Official)),
            Color::Yellow
        );
        assert_eq!(
            super::provenance_color(Some(Provenance::Community)),
            Color::Magenta
        );
        assert_eq!(super::provenance_color(None), Color::DarkGray);
    }

    #[test]
    fn recency_formatting() {
        let today = day(2026, 7, 16);
        assert_eq!(format_recency(None, today), "never");
        assert_eq!(format_recency(Some(day(2026, 7, 16)), today), "today");
        assert_eq!(format_recency(Some(day(2026, 7, 15)), today), "1d ago");
        assert_eq!(format_recency(Some(day(2026, 7, 6)), today), "10d ago");
    }

    #[test]
    fn detail_lines_include_tokens_only_for_model() {
        let today = day(2026, 7, 16);
        let model_row = Row {
            name: "opus".into(),
            count: 3,
            last_used: Some(today),
            first_used: Some(day(2026, 7, 1)),
            trend: vec![0; TREND_DAYS],
            trend_tokens: vec![0; TREND_DAYS],
            trend_cost: vec![0.0; TREND_DAYS],
            input: 100,
            output: 200,
            cache_creation: 0,
            cache_read: 50,
            cost_usd: 1.234,
            by_project: vec![],
        };
        let ml = detail_lines(&model_row, Category::Model, today);
        assert!(ml.iter().any(|l| l.contains("tokens:")));
        assert!(ml.iter().any(|l| l.contains("est. cost: $1.23")));

        let skill_row = Row {
            by_project: vec![("alpha".into(), 3)],
            ..model_row.clone()
        };
        let sl = detail_lines(&skill_row, Category::Skill, today);
        assert!(!sl.iter().any(|l| l.contains("tokens:")));
        assert!(sl.iter().any(|l| l.contains("by project:")));
    }

    #[test]
    fn detail_lines_suppresses_tokens_for_zero_count_model_row() {
        let today = day(2026, 7, 16);
        let row = Row {
            name: "opus".into(),
            count: 0,
            last_used: Some(today),
            first_used: Some(day(2026, 7, 1)),
            trend: vec![0; TREND_DAYS],
            trend_tokens: vec![0; TREND_DAYS],
            trend_cost: vec![0.0; TREND_DAYS],
            input: 0,
            output: 0,
            cache_creation: 0,
            cache_read: 0,
            cost_usd: 0.0,
            by_project: vec![],
        };
        let lines = detail_lines(&row, Category::Model, today);
        assert!(!lines.iter().any(|l| l.starts_with("tokens:")));
        assert!(!lines.iter().any(|l| l.starts_with("est. cost:")));
    }

    #[test]
    fn detail_lines_omits_by_project_section_when_empty() {
        let today = day(2026, 7, 16);
        let row = Row {
            name: "opus".into(),
            count: 3,
            last_used: Some(today),
            first_used: Some(day(2026, 7, 1)),
            trend: vec![0; TREND_DAYS],
            trend_tokens: vec![0; TREND_DAYS],
            trend_cost: vec![0.0; TREND_DAYS],
            input: 100,
            output: 200,
            cache_creation: 0,
            cache_read: 50,
            cost_usd: 1.23,
            by_project: vec![],
        };
        let lines = detail_lines(&row, Category::Model, today);
        assert!(!lines.iter().any(|l| l == "by project:"));
    }

    #[test]
    fn model_colors_are_distinct_per_variant_and_family() {
        // The exact models the family-color scheme collided on: opus variants
        // and fable-vs-sonnet. All must get distinct colors.
        let names = [
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-sonnet-5",
            "claude-fable-5",
            "claude-haiku-4-5",
        ];
        let colors: Vec<Color> = names
            .iter()
            .map(|n| super::color_for_name(&names, n))
            .collect();
        for i in 0..colors.len() {
            for j in (i + 1)..colors.len() {
                assert_ne!(
                    colors[i], colors[j],
                    "{} and {} share a color",
                    names[i], names[j]
                );
            }
        }
    }

    #[test]
    fn model_color_is_stable_regardless_of_input_order() {
        let a = ["claude-opus-4-8", "claude-sonnet-5", "claude-fable-5"];
        let b = ["claude-fable-5", "claude-opus-4-8", "claude-sonnet-5"];
        assert_eq!(
            super::color_for_name(&a, "claude-opus-4-8"),
            super::color_for_name(&b, "claude-opus-4-8"),
        );
    }
}
