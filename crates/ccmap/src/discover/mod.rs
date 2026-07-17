//! Discovery of Claude Code agents, skills, commands, plugins, and MCP
//! servers. `discover_all` sweeps: user (`~/.claude`) and project
//! (`<project>/.claude`) markdown sources; enabled-plugin agents, skills,
//! and commands; plugin catalog state (`Kind::Plugin`); and MCP servers
//! configured in `~/.claude.json` and `.mcp.json`.

pub mod mcp;
pub mod plugins;

use crate::frontmatter;
use crate::model::{Item, Kind, Source};
use std::fs;
use std::path::{Path, PathBuf};

/// The two filesystem roots discovery scans: the user's global `~/.claude`
/// directory and the current project's `.claude` directory.
#[derive(Debug, Clone)]
pub struct Context {
    pub claude_dir: PathBuf,
    pub project_dir: PathBuf,
}

/// The full set of items found by a discovery pass.
#[derive(Debug)]
pub struct Discovered {
    pub items: Vec<Item>,
}

/// How a source directory is laid out on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// Each item is a single `<name>.md` file directly under `root`.
    FlatMd,
    /// Each item is a subdirectory of `root` containing a `SKILL.md` file.
    SkillDir,
}

/// Scans `root` for markdown-defined items of `kind`, tagging each with
/// `source`. Returns an empty `Vec` if `root` does not exist or is not a
/// directory. `root` (and any directory entry under it) may be a symlink;
/// both are followed transparently via `fs::read_dir`/`fs::metadata`.
pub(crate) fn scan_md(kind: Kind, source: &Source, root: &Path, layout: Layout) -> Vec<Item> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let item = match layout {
            Layout::FlatMd => {
                if !fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                build_item(kind, source, &path, stem)
            }
            Layout::SkillDir => {
                if !fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
                    continue;
                }
                let skill_md = path.join("SKILL.md");
                if !fs::metadata(&skill_md).is_ok_and(|m| m.is_file()) {
                    continue;
                }
                let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                build_item(kind, source, &skill_md, dir_name)
            }
        };
        if let Some(item) = item {
            items.push(item);
        }
    }
    items
}

/// Reads `file` and builds an [`Item`] from its frontmatter, falling back to
/// `default_name` and a placeholder description when frontmatter is absent
/// or missing those keys. Returns `None` if `file` cannot be read.
fn build_item(kind: Kind, source: &Source, file: &Path, default_name: &str) -> Option<Item> {
    let content = fs::read_to_string(file).ok()?;
    let fm = frontmatter::parse(&content);
    let name = fm.get("name").unwrap_or(default_name).to_string();
    let description = fm
        .get("description")
        .unwrap_or("(no description)")
        .to_string();
    let extra = ["tools", "allowed-tools", "model"]
        .into_iter()
        .filter_map(|key| {
            fm.get(key)
                .map(|value| (key.to_string(), value.to_string()))
        })
        .collect();

    Some(Item {
        kind,
        name,
        description,
        source: source.clone(),
        path: Some(file.to_path_buf()),
        extra,
        plugin_state: None,
    })
}

/// Discovers agents, skills, and commands (user + project + enabled plugins)
/// plus the plugin catalog. Does NOT read `~/.claude.json` or discover MCP
/// servers — that is `discover_all`'s additional step. Split out so callers
/// that only need extensions (e.g. usage-provenance lookups) avoid the
/// `~/.claude.json` read.
#[must_use]
pub fn discover_extensions(ctx: &Context) -> Vec<Item> {
    let specs = [
        (Kind::Agent, "agents", Layout::FlatMd),
        (Kind::Skill, "skills", Layout::SkillDir),
        (Kind::Command, "commands", Layout::FlatMd),
    ];

    let mut items = Vec::new();
    for (kind, sub_dir, layout) in specs {
        items.extend(scan_md(
            kind,
            &Source::User,
            &ctx.claude_dir.join(sub_dir),
            layout,
        ));
        items.extend(scan_md(
            kind,
            &Source::Project,
            &ctx.project_dir.join(".claude").join(sub_dir),
            layout,
        ));
    }

    let plugin_discovery = plugins::discover(&ctx.claude_dir);
    for plugin in &plugin_discovery.enabled {
        let source = Source::Plugin {
            plugin: plugin.plugin.clone(),
            marketplace: plugin.marketplace.clone(),
        };
        items.extend(scan_md(
            Kind::Agent,
            &source,
            &plugin.agents_dir,
            Layout::FlatMd,
        ));
        items.extend(scan_md(
            Kind::Skill,
            &source,
            &plugin.skills_dir,
            Layout::SkillDir,
        ));
        items.extend(scan_md(
            Kind::Command,
            &source,
            &plugin.commands_dir,
            Layout::FlatMd,
        ));
    }
    items.extend(plugin_discovery.items);
    items
}

/// Discovers agents, skills, commands, plugins, and MCP servers from user,
/// project, and enabled-plugin sources.
#[must_use]
pub fn discover_all(ctx: &Context) -> Discovered {
    let mut items = discover_extensions(ctx);

    // `--claude-dir` overrides `~/.claude`, and `~/.claude.json` lives as its
    // sibling on disk (not inside it), so the equivalent override path is
    // derived rather than joined: `claude_dir`'s parent plus `.claude.json`.
    let claude_json = ctx
        .claude_dir
        .parent()
        .map_or_else(|| PathBuf::from(".claude.json"), |p| p.join(".claude.json"));
    items.extend(mcp::discover(&claude_json, &ctx.project_dir));

    Discovered { items }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn scans_user_agents_skills_commands_including_symlinked_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(real.join("agents")).unwrap();
        std::fs::write(
            real.join("agents/a.md"),
            "---\nname: alpha\ndescription: desc a\n---\n",
        )
        .unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let items = scan_md(
            Kind::Agent,
            &Source::User,
            &link.join("agents"),
            Layout::FlatMd,
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, Kind::Agent);
        assert_eq!(items[0].source, Source::User);
        assert_eq!(items[0].name, "alpha");
        assert_eq!(items[0].description, "desc a");
        assert!(
            scan_md(
                Kind::Command,
                &Source::User,
                &link.join("commands"),
                Layout::FlatMd
            )
            .is_empty()
        );
    }

    #[test]
    fn skilldir_uses_dir_name_when_frontmatter_omits_name() {
        let tmp = tempfile::tempdir().unwrap();
        let skills = tmp.path().join("skills");
        std::fs::create_dir_all(skills.join("s")).unwrap();
        std::fs::write(
            skills.join("s/SKILL.md"),
            "---\ndescription: a skill\n---\n",
        )
        .unwrap();

        let items = scan_md(Kind::Skill, &Source::User, &skills, Layout::SkillDir);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, Kind::Skill);
        assert_eq!(items[0].source, Source::User);
        assert_eq!(items[0].name, "s");
        assert_eq!(items[0].description, "a skill");
        assert_eq!(items[0].path, Some(skills.join("s/SKILL.md")));
    }

    #[test]
    fn flatmd_falls_back_to_stem_and_placeholder_description() {
        let tmp = tempfile::tempdir().unwrap();
        let commands = tmp.path().join("commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(commands.join("c.md"), "just a body, no frontmatter\n").unwrap();

        let items = scan_md(Kind::Command, &Source::Project, &commands, Layout::FlatMd);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, Kind::Command);
        assert_eq!(items[0].source, Source::Project);
        assert_eq!(items[0].name, "c");
        assert_eq!(items[0].description, "(no description)");
        assert!(items[0].extra.is_empty());
    }

    fn write_json(path: &Path, value: &serde_json::Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    /// Builds a `claude_dir` fixture with one marketplace declaring two
    /// plugins that are both installed, but only `enabled-plugin` is turned
    /// on via `enabledPlugins`. `enabled-plugin` gets a
    /// `skills/demo/SKILL.md`; `hidden-plugin` (installed but not enabled)
    /// gets `skills/hidden/SKILL.md`, which must not surface in discovery
    /// results. Returns the `claude_dir` path.
    fn build_plugin_fixture(root: &Path) -> PathBuf {
        let claude_dir = root.join("claude");
        let plugins_dir = claude_dir.join("plugins");
        let marketplace_root = root.join("external/mkt");

        write_json(
            &marketplace_root.join(".claude-plugin/marketplace.json"),
            &serde_json::json!({
                "plugins": [
                    { "name": "enabled-plugin", "description": "Enabled plugin" },
                    { "name": "hidden-plugin", "description": "Hidden plugin" }
                ]
            }),
        );
        write_json(
            &plugins_dir.join("known_marketplaces.json"),
            &serde_json::json!({
                "mkt": { "installLocation": marketplace_root.to_string_lossy() }
            }),
        );

        let enabled_install = root.join("installed/enabled-plugin");
        let hidden_install = root.join("installed/hidden-plugin");
        write_json(
            &plugins_dir.join("installed_plugins.json"),
            &serde_json::json!({
                "version": 1,
                "plugins": {
                    "enabled-plugin@mkt": [
                        {
                            "scope": "user",
                            "installPath": enabled_install.to_string_lossy(),
                            "version": "0.1"
                        }
                    ],
                    "hidden-plugin@mkt": [
                        {
                            "scope": "user",
                            "installPath": hidden_install.to_string_lossy(),
                            "version": "0.1"
                        }
                    ]
                }
            }),
        );
        write_json(
            &claude_dir.join("settings.json"),
            &serde_json::json!({
                "enabledPlugins": { "enabled-plugin@mkt": true }
            }),
        );

        fs::create_dir_all(enabled_install.join("skills/demo")).unwrap();
        fs::write(
            enabled_install.join("skills/demo/SKILL.md"),
            "---\nname: demo\ndescription: demo skill\n---\n",
        )
        .unwrap();

        fs::create_dir_all(hidden_install.join("skills/hidden")).unwrap();
        fs::write(
            hidden_install.join("skills/hidden/SKILL.md"),
            "---\nname: hidden\ndescription: hidden skill\n---\n",
        )
        .unwrap();

        claude_dir
    }

    #[test]
    fn discover_all_includes_enabled_plugin_skills_but_not_installed_only_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = build_plugin_fixture(tmp.path());
        let project_dir = tmp.path().join("project");
        fs::create_dir_all(&project_dir).unwrap();

        let ctx = Context {
            claude_dir,
            project_dir,
        };
        let discovered = discover_all(&ctx);

        let skill_names: Vec<&str> = discovered
            .items
            .iter()
            .filter(|item| item.kind == Kind::Skill)
            .map(|item| item.name.as_str())
            .collect();
        assert!(skill_names.contains(&"demo"));
        assert!(!skill_names.contains(&"hidden"));

        let demo = discovered
            .items
            .iter()
            .find(|item| item.kind == Kind::Skill && item.name == "demo")
            .unwrap();
        assert_eq!(
            demo.source,
            Source::Plugin {
                plugin: "enabled-plugin".into(),
                marketplace: "mkt".into(),
            }
        );
    }

    #[test]
    fn discover_all_wires_all_five_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = build_plugin_fixture(tmp.path());

        fs::create_dir_all(claude_dir.join("agents")).unwrap();
        fs::write(
            claude_dir.join("agents/a.md"),
            "---\nname: agent-a\ndescription: user agent\n---\n",
        )
        .unwrap();

        let project_dir = tmp.path().join("project");
        fs::create_dir_all(project_dir.join(".claude/commands")).unwrap();
        fs::write(
            project_dir.join(".claude/commands/c.md"),
            "---\nname: cmd-c\ndescription: project command\n---\n",
        )
        .unwrap();

        write_json(
            &tmp.path().join(".claude.json"),
            &serde_json::json!({
                "mcpServers": { "srv1": { "command": "secret-value" } }
            }),
        );
        write_json(
            &project_dir.join(".mcp.json"),
            &serde_json::json!({
                "mcpServers": { "srv2": {} }
            }),
        );

        let ctx = Context {
            claude_dir,
            project_dir,
        };
        let discovered = discover_all(&ctx);

        let count = |kind: Kind| discovered.items.iter().filter(|i| i.kind == kind).count();
        assert_eq!(count(Kind::Agent), 1);
        assert_eq!(count(Kind::Skill), 1);
        assert_eq!(count(Kind::Command), 1);
        assert_eq!(count(Kind::Plugin), 2);
        assert_eq!(count(Kind::Mcp), 2);
    }

    #[test]
    fn discover_extensions_excludes_mcp_and_matches_discover_all_minus_mcp() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = build_plugin_fixture(tmp.path());
        fs::create_dir_all(claude_dir.join("agents")).unwrap();
        fs::write(
            claude_dir.join("agents/a.md"),
            "---\nname: agent-a\ndescription: user agent\n---\n",
        )
        .unwrap();
        let project_dir = tmp.path().join("project");
        fs::create_dir_all(&project_dir).unwrap();
        // An .mcp.json that discover_all WOULD pick up, to prove extensions ignores MCP.
        write_json(
            &project_dir.join(".mcp.json"),
            &serde_json::json!({ "mcpServers": { "srv2": {} } }),
        );

        let ctx = Context {
            claude_dir,
            project_dir,
        };
        let ext = discover_extensions(&ctx);
        // No MCP items from discover_extensions.
        assert!(ext.iter().all(|i| i.kind != Kind::Mcp));
        // But it does surface agents/skills/plugins.
        assert!(
            ext.iter()
                .any(|i| i.kind == Kind::Agent && i.name == "agent-a")
        );
        assert!(
            ext.iter()
                .any(|i| i.kind == Kind::Skill && i.name == "demo")
        );
        assert!(ext.iter().any(|i| i.kind == Kind::Plugin));

        // discover_all = discover_extensions + MCP (same non-MCP set).
        let all = discover_all(&ctx).items;
        let all_non_mcp = all.iter().filter(|i| i.kind != Kind::Mcp).count();
        assert_eq!(all_non_mcp, ext.len());
        assert!(all.iter().any(|i| i.kind == Kind::Mcp)); // MCP only via discover_all
    }

    #[test]
    fn extra_is_collected_in_fixed_key_order() {
        let tmp = tempfile::tempdir().unwrap();
        let agents = tmp.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("a.md"),
            "---\nname: a\nmodel: opus\nallowed-tools: Read\ntools: Bash\n---\n",
        )
        .unwrap();

        let items = scan_md(Kind::Agent, &Source::User, &agents, Layout::FlatMd);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].extra,
            vec![
                ("tools".to_string(), "Bash".to_string()),
                ("allowed-tools".to_string(), "Read".to_string()),
                ("model".to_string(), "opus".to_string()),
            ]
        );
    }
}
