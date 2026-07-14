//! Plugin discovery: cross-references marketplace catalogs, install
//! records, and enabled-plugin settings to produce one [`Item`] per known
//! plugin plus the list of currently enabled plugins. Scanning an enabled
//! plugin's own agents/skills/commands markdown is wired in a later task.

use crate::model::{Item, Kind, PluginState, Source};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// An enabled plugin, ready to have its agents/skills/commands markdown
/// scanned by a later discovery pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnabledPlugin {
    pub plugin: String,
    pub marketplace: String,
    pub agents_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub commands_dir: PathBuf,
}

/// The result of a plugin discovery pass: one [`Item`] per known plugin
/// (regardless of lifecycle state) plus every plugin found to be enabled.
#[derive(Debug)]
pub struct PluginDiscovery {
    pub items: Vec<Item>,
    pub enabled: Vec<EnabledPlugin>,
}

/// A plugin's accumulated state while cross-referencing marketplace,
/// install, and enabled sources, before being turned into an [`Item`].
struct Entry {
    name: String,
    description: String,
    marketplace: String,
    state: PluginState,
    install_path: Option<PathBuf>,
}

/// Reads and parses `path` as JSON, returning `None` if it is missing or
/// fails to parse. Callers must tolerate `None` rather than panicking:
/// these files are user-editable and may have drifted schema (unknown
/// `version`, missing keys, and so on).
fn read_json(path: &Path) -> Option<Value> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Returns `(marketplace_name, install_location)` for every marketplace
/// known via `plugins/known_marketplaces.json` or `settings.json`'s
/// `extraKnownMarketplaces`, unioned by name. `known_marketplaces.json`'s
/// `installLocation` wins when present; otherwise the location is resolved
/// by convention as `claude_dir/plugins/marketplaces/<mkt>`.
fn marketplaces(claude_dir: &Path, plugins_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut result: BTreeMap<String, PathBuf> = BTreeMap::new();

    let known = read_json(&plugins_dir.join("known_marketplaces.json"));
    if let Some(map) = known.as_ref().and_then(Value::as_object) {
        for (name, value) in map {
            let install_location = value
                .get("installLocation")
                .and_then(Value::as_str)
                .map_or_else(
                    || plugins_dir.join("marketplaces").join(name),
                    PathBuf::from,
                );
            result.insert(name.clone(), install_location);
        }
    }

    let settings = read_json(&claude_dir.join("settings.json"));
    if let Some(map) = settings
        .as_ref()
        .and_then(|v| v.get("extraKnownMarketplaces"))
        .and_then(Value::as_object)
    {
        for name in map.keys() {
            result
                .entry(name.clone())
                .or_insert_with(|| plugins_dir.join("marketplaces").join(name));
        }
    }

    result.into_iter().collect()
}

/// Seeds `entries` with every plugin declared by
/// `<install_location>/.claude-plugin/marketplace.json`, each starting at
/// [`PluginState::Available`]. A missing or malformed catalog contributes
/// nothing.
fn seed_marketplace(
    entries: &mut BTreeMap<String, Entry>,
    marketplace: &str,
    install_location: &Path,
) {
    let Some(catalog) = read_json(
        &install_location
            .join(".claude-plugin")
            .join("marketplace.json"),
    ) else {
        return;
    };
    let Some(plugins) = catalog.get("plugins").and_then(Value::as_array) else {
        return;
    };

    for plugin in plugins {
        let Some(name) = plugin.get("name").and_then(Value::as_str) else {
            continue;
        };
        let description = plugin
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        entries.insert(
            format!("{name}@{marketplace}"),
            Entry {
                name: name.to_string(),
                description,
                marketplace: marketplace.to_string(),
                state: PluginState::Available,
                install_path: None,
            },
        );
    }
}

/// Applies `plugins/installed_plugins.json` records on top of `entries`,
/// upgrading matching (or newly discovered) plugins to
/// [`PluginState::Installed`] and capturing their install path. Prefers the
/// `scope == "user"` record when a plugin has multiple install records,
/// falling back to the first.
fn apply_installed(entries: &mut BTreeMap<String, Entry>, plugins_dir: &Path) {
    let Some(installed) = read_json(&plugins_dir.join("installed_plugins.json")) else {
        return;
    };
    let Some(map) = installed.get("plugins").and_then(Value::as_object) else {
        return;
    };

    for (key, records) in map {
        let Some(records) = records.as_array() else {
            continue;
        };
        let record = records
            .iter()
            .find(|record| record.get("scope").and_then(Value::as_str) == Some("user"))
            .or_else(|| records.first());
        let Some(install_path) = record
            .and_then(|record| record.get("installPath"))
            .and_then(Value::as_str)
        else {
            continue;
        };

        let entry = entries.entry(key.clone()).or_insert_with(|| {
            let (name, marketplace) = key.split_once('@').unwrap_or((key.as_str(), ""));
            Entry {
                name: name.to_string(),
                description: String::new(),
                marketplace: marketplace.to_string(),
                state: PluginState::Available,
                install_path: None,
            }
        });
        entry.state = entry.state.max(PluginState::Installed);
        entry.install_path = Some(PathBuf::from(install_path));
    }
}

/// Merges `enabledPlugins` from `settings.json` and `settings.local.json`
/// (local settings win per key when both declare the same key) and
/// upgrades every plugin enabled (`true`) that is already present in
/// `entries` to [`PluginState::Enabled`]. A plugin enabled but unknown to
/// any marketplace/install source is not added — there's nothing to show.
fn apply_enabled(entries: &mut BTreeMap<String, Entry>, claude_dir: &Path) {
    let mut enabled_plugins: BTreeMap<String, bool> = BTreeMap::new();

    for file in ["settings.json", "settings.local.json"] {
        let Some(settings) = read_json(&claude_dir.join(file)) else {
            continue;
        };
        let Some(map) = settings.get("enabledPlugins").and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in map {
            if let Some(is_enabled) = value.as_bool() {
                enabled_plugins.insert(key.clone(), is_enabled);
            }
        }
    }

    for (key, is_enabled) in enabled_plugins {
        if !is_enabled {
            continue;
        }
        if let Some(entry) = entries.get_mut(&key) {
            entry.state = entry.state.max(PluginState::Enabled);
        }
    }
}

/// Resolves the `agents`/`skills`/`commands` directories for an enabled
/// plugin, honoring custom paths declared in
/// `<install_path>/.claude-plugin/plugin.json` and falling back to
/// `install_path/{agents,skills,commands}`.
fn enabled_plugin(plugin: &str, marketplace: &str, install_path: &Path) -> EnabledPlugin {
    let manifest = read_json(&install_path.join(".claude-plugin").join("plugin.json"));

    let resolve = |key: &str, default_dir: &str| {
        manifest
            .as_ref()
            .and_then(|m| m.get(key))
            .and_then(Value::as_str)
            .map_or_else(
                || install_path.join(default_dir),
                |custom| install_path.join(custom),
            )
    };

    EnabledPlugin {
        plugin: plugin.to_string(),
        marketplace: marketplace.to_string(),
        agents_dir: resolve("agents", "agents"),
        skills_dir: resolve("skills", "skills"),
        commands_dir: resolve("commands", "commands"),
    }
}

/// Discovers plugins known to `claude_dir` across marketplace catalogs,
/// install records, and enabled-plugin settings.
///
/// Algorithm:
/// 1. Union marketplace names from `known_marketplaces.json` and
///    `settings.json`'s `extraKnownMarketplaces`.
/// 2. Seed one entry per `"<plugin>@<marketplace>"` from each
///    marketplace's catalog, at `Available`.
/// 3. Upgrade entries matching `installed_plugins.json` records to
///    `Installed`, capturing the install path (or add a new entry if the
///    installed plugin wasn't in any readable catalog).
/// 4. Upgrade entries enabled via `enabledPlugins` to `Enabled`.
/// 5. Emit one [`Item`] per entry and one [`EnabledPlugin`] per `Enabled`
///    entry that has an install path.
#[must_use]
pub fn discover(claude_dir: &Path) -> PluginDiscovery {
    let plugins_dir = claude_dir.join("plugins");
    let mut entries: BTreeMap<String, Entry> = BTreeMap::new();

    for (marketplace, install_location) in marketplaces(claude_dir, &plugins_dir) {
        seed_marketplace(&mut entries, &marketplace, &install_location);
    }

    apply_installed(&mut entries, &plugins_dir);
    apply_enabled(&mut entries, claude_dir);

    let mut items = Vec::with_capacity(entries.len());
    let mut enabled = Vec::new();
    for entry in entries.into_values() {
        let Entry {
            name,
            description,
            marketplace,
            state,
            install_path,
        } = entry;

        if state == PluginState::Enabled
            && let Some(install_path) = &install_path
        {
            enabled.push(enabled_plugin(&name, &marketplace, install_path));
        }

        items.push(Item {
            kind: Kind::Plugin,
            name: name.clone(),
            description,
            source: Source::Plugin {
                plugin: name,
                marketplace,
            },
            path: install_path,
            extra: Vec::new(),
            plugin_state: Some(state),
        });
    }

    PluginDiscovery { items, enabled }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_json(path: &Path, value: &serde_json::Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    /// Builds a `claude_dir` fixture under `root` with two marketplaces
    /// (one via `known_marketplaces.json`, one via `extraKnownMarketplaces`)
    /// that both declare a plugin named `foo`, an install record for
    /// `foo@mkt-a` picking the `scope=="user"` record over the first, and
    /// that same plugin enabled. Returns the resolved user install path.
    fn build_duplicate_plugin_fixture(root: &Path) -> PathBuf {
        let plugins_dir = root.join("plugins");

        let official_root = root.join("external/official");
        write_json(
            &official_root.join(".claude-plugin/marketplace.json"),
            &serde_json::json!({
                "plugins": [
                    { "name": "foo", "description": "Foo plugin" },
                    { "name": "bar", "description": "Bar plugin" }
                ]
            }),
        );

        write_json(
            &plugins_dir.join("known_marketplaces.json"),
            &serde_json::json!({
                "mkt-a": { "installLocation": official_root.to_string_lossy() }
            }),
        );

        let extra_root = plugins_dir.join("marketplaces/mkt-b");
        write_json(
            &extra_root.join(".claude-plugin/marketplace.json"),
            &serde_json::json!({
                "plugins": [
                    { "name": "foo", "description": "Foo plugin, mkt-b edition" }
                ]
            }),
        );

        write_json(
            &root.join("settings.json"),
            &serde_json::json!({
                "extraKnownMarketplaces": { "mkt-b": { "source": {} } },
                "enabledPlugins": { "foo@mkt-a": true }
            }),
        );

        let user_install_path = root.join("installed/foo-a");
        write_json(
            &user_install_path.join(".claude-plugin/plugin.json"),
            &serde_json::json!({ "skills": "custom-skills" }),
        );

        write_json(
            &plugins_dir.join("installed_plugins.json"),
            &serde_json::json!({
                "version": 1,
                "plugins": {
                    "foo@mkt-a": [
                        {
                            "scope": "project",
                            "installPath": root.join("installed/foo-a-project").to_string_lossy(),
                            "version": "0.1"
                        },
                        {
                            "scope": "user",
                            "installPath": user_install_path.to_string_lossy(),
                            "version": "0.2"
                        }
                    ]
                }
            }),
        );

        user_install_path
    }

    #[test]
    fn discovers_marketplace_installed_and_enabled_states_with_duplicate_plugin_name() {
        let tmp = tempfile::tempdir().unwrap();
        let user_install_path = build_duplicate_plugin_fixture(tmp.path());

        let result = discover(tmp.path());

        assert_eq!(result.items.len(), 3);

        let foo_a = result
            .items
            .iter()
            .find(|item| {
                item.source
                    == Source::Plugin {
                        plugin: "foo".into(),
                        marketplace: "mkt-a".into(),
                    }
            })
            .unwrap();
        assert_eq!(foo_a.plugin_state, Some(PluginState::Enabled));
        assert_eq!(foo_a.kind, Kind::Plugin);
        assert_eq!(foo_a.path, Some(user_install_path.clone()));

        let bar_a = result
            .items
            .iter()
            .find(|item| {
                item.source
                    == Source::Plugin {
                        plugin: "bar".into(),
                        marketplace: "mkt-a".into(),
                    }
            })
            .unwrap();
        assert_eq!(bar_a.plugin_state, Some(PluginState::Available));
        assert_eq!(bar_a.path, None);

        let foo_b = result
            .items
            .iter()
            .find(|item| {
                item.source
                    == Source::Plugin {
                        plugin: "foo".into(),
                        marketplace: "mkt-b".into(),
                    }
            })
            .unwrap();
        assert_eq!(foo_b.plugin_state, Some(PluginState::Available));
        assert_eq!(foo_b.path, None);
        assert_eq!(foo_b.description, "Foo plugin, mkt-b edition");

        assert_eq!(result.enabled.len(), 1);
        let enabled = &result.enabled[0];
        assert_eq!(enabled.plugin, "foo");
        assert_eq!(enabled.marketplace, "mkt-a");
        assert_eq!(enabled.agents_dir, user_install_path.join("agents"));
        assert_eq!(enabled.skills_dir, user_install_path.join("custom-skills"));
        assert_eq!(enabled.commands_dir, user_install_path.join("commands"));
    }

    #[test]
    fn tolerates_missing_and_malformed_json_without_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path();
        let plugins_dir = claude_dir.join("plugins");
        fs::create_dir_all(&plugins_dir).unwrap();
        fs::write(plugins_dir.join("known_marketplaces.json"), "{ not json").unwrap();
        fs::write(
            claude_dir.join("settings.json"),
            serde_json::json!({
                "somethingUnexpected": 42,
                "enabledPlugins": { "foo@mkt-a": "not-a-bool" }
            })
            .to_string(),
        )
        .unwrap();

        let result = discover(claude_dir);

        assert!(result.items.is_empty());
        assert!(result.enabled.is_empty());
    }
}
