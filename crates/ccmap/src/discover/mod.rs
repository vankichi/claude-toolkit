//! Discovery of Claude Code agents, skills, and commands from markdown
//! sources on disk. This module currently scans user (`~/.claude`) and
//! project (`<project>/.claude`) sources only; plugin-provided items are
//! wired in a later task.

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

/// Discovers agents, skills, and commands from user and project sources.
///
/// This is a temporary implementation: plugin-provided items and MCP
/// servers are wired into `discover_all` by later tasks.
#[must_use]
pub fn discover_all(ctx: &Context) -> Discovered {
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
}
