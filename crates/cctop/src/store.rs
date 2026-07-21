//! Live data store backing the three overview panels.
//!
//! Holds the self-aggregated [`NowStats`] plus a `ccstat` and a `ccmap`
//! `AppState` (reused verbatim for both the compact overview panels and the
//! drill-down full views). `load` does the initial scan/discover; `rescan`
//! refreshes the stats + map subsystems while preserving the Now aggregate and
//! any in-view selection/filter state.

use crate::now::NowStats;
use ccmap::discover::{self, Context};
use ccmap::model::{Item, Kind};
use ccstat::live::ActiveSet;
use ccstat::provenance::ProvenanceMap;
use ccstat::scan::{self, ScanConfig};
use chrono::{Duration, NaiveDate, Utc};

/// How recently a skill/agent must have run to count as "running now" in the
/// config map.
const ACTIVE_WINDOW_SECS: i64 = 120;
/// Tail size read per active session when detecting running items.
const ACTIVE_TAIL_BYTES: u64 = 16 * 1024;

/// The set of `(category, name)` pairs running now, from the active session
/// tails.
fn running_now(scan_cfg: &ScanConfig) -> ActiveSet {
    scan::compute_active(
        scan_cfg,
        Utc::now(),
        Duration::seconds(ACTIVE_WINDOW_SECS),
        ACTIVE_TAIL_BYTES,
    )
}

/// Per-kind counts for the Config-map panel.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MapCounts {
    pub agents: usize,
    pub skills: usize,
    pub commands: usize,
    pub plugins: usize,
    pub mcp: usize,
}

impl MapCounts {
    #[must_use]
    pub fn from_items(items: &[Item]) -> Self {
        let mut c = MapCounts::default();
        for it in items {
            match it.kind {
                Kind::Agent => c.agents += 1,
                Kind::Skill => c.skills += 1,
                Kind::Command => c.commands += 1,
                Kind::Plugin => c.plugins += 1,
                Kind::Mcp => c.mcp += 1,
            }
        }
        c
    }
}

/// The live dashboard state shared by the overview and drill-down views.
pub struct Dashboard {
    pub now: NowStats,
    pub stats: ccstat::ui::AppState,
    pub map: ccmap::ui::AppState,
    pub map_counts: MapCounts,
    /// Skills/agents/etc. running right now (config-map live highlight).
    pub active: ActiveSet,
}

impl Dashboard {
    /// Initial load: scan every session into the stats store and discover the
    /// config map. The Now aggregate starts empty and is filled by the tail.
    #[must_use]
    pub fn load(scan_cfg: &ScanConfig, ctx: &Context, today: NaiveDate) -> Self {
        let db = scan::scan(scan_cfg, today);
        let provenance = ProvenanceMap::build(&discover::discover_extensions(ctx));
        let stats = ccstat::ui::AppState::new(db, provenance, today);

        let items = discover::discover_all(ctx).items;
        let map_counts = MapCounts::from_items(&items);
        let map = ccmap::ui::AppState::new(items);

        Dashboard {
            now: NowStats::new(),
            stats,
            map,
            map_counts,
            active: running_now(scan_cfg),
        }
    }

    /// Re-scan the corpus and re-discover the config map, preserving the Now
    /// aggregate and the stats/map view state (selection, filter, tab).
    pub fn rescan(&mut self, scan_cfg: &ScanConfig, ctx: &Context, today: NaiveDate) {
        let db = scan::scan(scan_cfg, today);
        let provenance = ProvenanceMap::build(&discover::discover_extensions(ctx));
        self.stats.reload_at(today, db, provenance);

        let items = discover::discover_all(ctx).items;
        self.map_counts = MapCounts::from_items(&items);
        self.map.reload(items);

        self.active = running_now(scan_cfg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccmap::model::Source;

    fn item(kind: Kind) -> Item {
        Item {
            kind,
            name: "x".into(),
            description: String::new(),
            source: Source::User,
            path: None,
            extra: Vec::new(),
            plugin_state: None,
        }
    }

    #[test]
    fn map_counts_tally_by_kind() {
        let items = vec![
            item(Kind::Agent),
            item(Kind::Agent),
            item(Kind::Skill),
            item(Kind::Command),
            item(Kind::Plugin),
            item(Kind::Mcp),
            item(Kind::Mcp),
            item(Kind::Mcp),
        ];
        let c = MapCounts::from_items(&items);
        assert_eq!(c.agents, 2);
        assert_eq!(c.skills, 1);
        assert_eq!(c.commands, 1);
        assert_eq!(c.plugins, 1);
        assert_eq!(c.mcp, 3);
    }

    #[test]
    fn map_counts_empty_is_all_zero() {
        assert_eq!(MapCounts::from_items(&[]), MapCounts::default());
    }

    #[test]
    fn load_on_empty_dirs_is_empty() {
        let projects = tempfile::tempdir().unwrap();
        let claude = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let scan_cfg = ScanConfig {
            projects_dir: projects.path().to_path_buf(),
        };
        let ctx = Context {
            claude_dir: claude.path().to_path_buf(),
            project_dir: project.path().to_path_buf(),
        };
        let today = NaiveDate::from_ymd_opt(2026, 7, 18).unwrap();
        let dash = Dashboard::load(&scan_cfg, &ctx, today);
        assert_eq!(dash.map_counts, MapCounts::default());
        assert_eq!(dash.now.total_tokens(), 0);
    }
}
