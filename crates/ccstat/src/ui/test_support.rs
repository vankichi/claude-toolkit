//! Fixtures shared by the `ui` submodules' tests.

use crate::model::Category;
use crate::usage::{LineData, UsageDb};
use cctk::jsonl::Extracted;
use chrono::{NaiveDate, TimeZone};

pub(super) fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

pub(super) fn db_with(items: &[(Category, &str, &str)]) -> UsageDb {
    let mut db = UsageDb::default();
    let ts = chrono::Utc.with_ymd_and_hms(2026, 7, 16, 12, 0, 0).unwrap();
    for (cat, name, project) in items {
        let ex = match cat {
            // Model isn't exercised by these tests; it maps to Skill as a placeholder.
            Category::Skill | Category::Model => Extracted::Skill {
                name: (*name).into(),
            },
            Category::Agent => Extracted::Agent {
                name: (*name).into(),
            },
            Category::Command => Extracted::Command {
                name: (*name).into(),
            },
            Category::Mcp => Extracted::Mcp {
                server: (*name).into(),
            },
        };
        let line = LineData {
            timestamp: Some(ts),
            items: vec![ex],
        };
        db.absorb(&line, project, day(2026, 7, 16));
    }
    db
}
