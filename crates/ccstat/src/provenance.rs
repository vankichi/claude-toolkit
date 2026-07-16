//! Joins ccstat usage names to their ccmap-discovered provenance so the UI can
//! color agent/skill/command rows by origin (local / project / official /
//! community). Names with no matching discovered extension (built-in agents,
//! built-in commands, deleted skills) return `None`.

use crate::model::Category;
use ccmap::model::{Item, Kind, Provenance};
use std::collections::HashMap;

/// Lookup from (category, matchable-name) to the discovered extension's provenance.
#[derive(Default)]
pub struct ProvenanceMap {
    map: HashMap<(Category, String), Provenance>,
}

impl ProvenanceMap {
    /// Build the lookup from discovered extension items. Only Agent/Skill/Command
    /// kinds are indexed; Plugin/Mcp items are ignored. The matchable name is
    /// keyed to how ccstat records usage:
    /// - Agent: the agent's `name` (matches recorded `subagent_type`)
    /// - Skill: `invocation_string()` without the leading `/` (matches `input.skill`,
    ///   e.g. `superpowers:brainstorming` or `improve-harness`)
    /// - Command: `invocation_string()` with the leading `/` (matches `/xxx`)
    #[must_use]
    pub fn build(items: &[Item]) -> Self {
        let mut map = HashMap::new();
        for item in items {
            let Some(category) = category_of(item.kind) else {
                continue;
            };
            let key = match category {
                Category::Agent => item.name.clone(),
                Category::Skill => item.invocation_string().trim_start_matches('/').to_string(),
                Category::Command => item.invocation_string(),
                _ => continue,
            };
            // First writer wins (stable across discovery order); duplicates across
            // sources are rare and ccmap itself does not resolve name collisions.
            map.entry((category, key))
                .or_insert_with(|| item.provenance());
        }
        Self { map }
    }

    /// The provenance of a used extension, or `None` if it wasn't discovered
    /// (built-in, plugin-not-enabled, deleted, or from another project).
    #[must_use]
    pub fn lookup(&self, category: Category, name: &str) -> Option<Provenance> {
        self.map.get(&(category, name.to_string())).copied()
    }
}

/// Map a ccmap `Kind` to the ccstat `Category` it colors, if any.
fn category_of(kind: Kind) -> Option<Category> {
    match kind {
        Kind::Agent => Some(Category::Agent),
        Kind::Skill => Some(Category::Skill),
        Kind::Command => Some(Category::Command),
        Kind::Plugin | Kind::Mcp => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccmap::model::Source;

    fn item(kind: Kind, name: &str, source: Source) -> Item {
        Item {
            kind,
            name: name.to_string(),
            description: String::new(),
            source,
            path: None,
            extra: vec![],
            plugin_state: None,
        }
    }

    #[test]
    fn agent_matches_by_name() {
        let items = vec![item(Kind::Agent, "code-refactor-advisor", Source::User)];
        let pm = ProvenanceMap::build(&items);
        assert_eq!(
            pm.lookup(Category::Agent, "code-refactor-advisor"),
            Some(Provenance::Local)
        );
        // A built-in / unknown agent is not present.
        assert_eq!(pm.lookup(Category::Agent, "Explore"), None);
    }

    #[test]
    fn plugin_skill_matches_by_namespaced_invocation() {
        let items = vec![item(
            Kind::Skill,
            "brainstorming",
            Source::Plugin {
                plugin: "superpowers".into(),
                marketplace: "claude-plugins-official".into(),
            },
        )];
        let pm = ProvenanceMap::build(&items);
        assert_eq!(
            pm.lookup(Category::Skill, "superpowers:brainstorming"),
            Some(Provenance::Official)
        );
        // The bare frontmatter name must NOT match (usage records the namespaced form).
        assert_eq!(pm.lookup(Category::Skill, "brainstorming"), None);
    }

    #[test]
    fn user_skill_matches_bare_name() {
        let items = vec![item(Kind::Skill, "improve-harness", Source::User)];
        let pm = ProvenanceMap::build(&items);
        assert_eq!(
            pm.lookup(Category::Skill, "improve-harness"),
            Some(Provenance::Local)
        );
    }

    #[test]
    fn command_matches_with_leading_slash() {
        let items = vec![item(Kind::Command, "review", Source::Project)];
        let pm = ProvenanceMap::build(&items);
        assert_eq!(
            pm.lookup(Category::Command, "/review"),
            Some(Provenance::Project)
        );
        // Built-in command not discovered → None.
        assert_eq!(pm.lookup(Category::Command, "/clear"), None);
    }

    #[test]
    fn plugin_and_mcp_kinds_are_ignored() {
        let items = vec![
            item(
                Kind::Plugin,
                "superpowers",
                Source::Plugin {
                    plugin: "superpowers".into(),
                    marketplace: "claude-plugins-official".into(),
                },
            ),
            item(Kind::Mcp, "serena", Source::User),
        ];
        let pm = ProvenanceMap::build(&items);
        assert_eq!(pm.lookup(Category::Model, "superpowers"), None);
        assert_eq!(pm.lookup(Category::Mcp, "serena"), None);
    }

    #[test]
    fn default_map_is_empty() {
        let pm = ProvenanceMap::default();
        assert_eq!(pm.lookup(Category::Skill, "anything"), None);
    }
}
