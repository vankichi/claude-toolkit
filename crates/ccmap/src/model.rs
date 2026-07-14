//! Core domain model for `ccmap`: the kinds of Claude Code extensions it
//! discovers, where each one comes from, and how a user would invoke it.

use std::path::PathBuf;

/// The category of a discovered Claude Code extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Agent,
    Skill,
    Command,
    Plugin,
    Mcp,
}

/// Where an [`Item`] was loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    User,
    Project,
    Plugin { plugin: String, marketplace: String },
}

/// The install/enable lifecycle state of a plugin, ordered for ranking
/// (`Available` < `Installed` < `Enabled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PluginState {
    Available,
    Installed,
    Enabled,
}

/// A single discovered Claude Code extension (agent, skill, command, plugin,
/// or MCP server).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub description: String,
    pub source: Source,
    pub path: Option<PathBuf>,
    pub extra: Vec<(String, String)>,
    pub plugin_state: Option<PluginState>,
}

impl Item {
    /// Renders how a user would invoke this item: a slash command for
    /// skills/commands, the bare name for agents/MCP servers, and a
    /// `plugin@marketplace` identifier for plugins.
    #[must_use]
    pub fn invocation_string(&self) -> String {
        match self.kind {
            Kind::Agent | Kind::Mcp => self.name.clone(),
            Kind::Skill | Kind::Command => match &self.source {
                Source::Plugin { plugin, .. } => format!("/{plugin}:{}", self.name),
                Source::User | Source::Project => format!("/{}", self.name),
            },
            Kind::Plugin => match &self.source {
                Source::Plugin {
                    plugin,
                    marketplace,
                } => format!("{plugin}@{marketplace}"),
                Source::User | Source::Project => self.name.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_string_covers_each_kind() {
        let user_skill = Item {
            kind: Kind::Skill,
            name: "api-design-review".into(),
            description: String::new(),
            source: Source::User,
            path: None,
            extra: vec![],
            plugin_state: None,
        };
        assert_eq!(user_skill.invocation_string(), "/api-design-review");

        let plugin_skill = Item {
            source: Source::Plugin {
                plugin: "superpowers".into(),
                marketplace: "claude-plugins-official".into(),
            },
            ..user_skill.clone()
        };
        assert_eq!(
            plugin_skill.invocation_string(),
            "/superpowers:api-design-review"
        );

        let agent = Item {
            kind: Kind::Agent,
            name: "code-refactor-advisor".into(),
            ..user_skill.clone()
        };
        assert_eq!(agent.invocation_string(), "code-refactor-advisor");

        let plugin = Item {
            kind: Kind::Plugin,
            name: "superpowers".into(),
            source: Source::Plugin {
                plugin: "superpowers".into(),
                marketplace: "claude-plugins-official".into(),
            },
            ..user_skill.clone()
        };
        assert_eq!(
            plugin.invocation_string(),
            "superpowers@claude-plugins-official"
        );

        let mcp = Item {
            kind: Kind::Mcp,
            name: "serena".into(),
            ..user_skill.clone()
        };
        assert_eq!(mcp.invocation_string(), "serena");
    }
}
