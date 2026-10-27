//! Dashboard settings from `<state>/config.toml`.
//!
//! Only a flat subset of TOML is read (no new dependency for three keys):
//! `key = 123` and `key = "text"` lines, blank lines, and `#` comments.
//!
//! ```toml
//! limit = 4               # parallel sessions before the header turns red
//! round_minutes = 30      # check-in grid from local midnight; 0 hides it
//! container_target = "dev" # tmux target for jumping to a container session
//! ```

use std::io::Read;
use std::path::Path;

pub const FILE_NAME: &str = "config.toml";

/// Larger config files are refused rather than read.
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Parallel sessions allowed before the header warns (warning only).
    pub limit: usize,
    /// Minutes between check-ins, aligned to local midnight; 0 hides them.
    pub round_minutes: u32,
    /// `tmux switch-client -t` target for sessions running in the container
    /// (whose panes live in the host tmux's window for the container).
    pub container_target: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            limit: 4,
            round_minutes: 30,
            container_target: "dev".to_string(),
        }
    }
}

/// The config in `state_dir`, plus a one-line warning when the file exists
/// but could not be used (the defaults apply then). A missing file is not a
/// warning.
#[must_use]
pub fn load(state_dir: &Path) -> (Config, Option<String>) {
    let path = state_dir.join(FILE_NAME);
    match std::fs::symlink_metadata(&path) {
        Err(_) => return (Config::default(), None),
        Ok(m) if !m.file_type().is_file() => {
            return (
                Config::default(),
                Some(format!("{FILE_NAME}: not a regular file")),
            );
        }
        Ok(m) if m.len() > MAX_BYTES => {
            return (
                Config::default(),
                Some(format!("{FILE_NAME}: larger than {MAX_BYTES} bytes")),
            );
        }
        Ok(_) => {}
    }
    let mut text = String::new();
    let read = std::fs::File::open(&path)
        .and_then(|f| f.take(MAX_BYTES).read_to_string(&mut text).map(|_| ()));
    if let Err(e) = read {
        return (Config::default(), Some(format!("{FILE_NAME}: {e}")));
    }
    match parse(&text) {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(format!("{FILE_NAME}: {e}"))),
    }
}

/// Parse the flat TOML subset described in the module docs.
pub fn parse(text: &str) -> Result<Config, String> {
    let mut config = Config::default();
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {n}: expected `key = value`"))?;
        let value = Value::parse(value.trim()).map_err(|e| format!("line {n}: {e}"))?;
        match (key.trim(), value) {
            ("limit", Value::Int(v)) if (1..=99).contains(&v) => {
                config.limit = usize::try_from(v).unwrap_or(4);
            }
            ("round_minutes", Value::Int(v)) if v <= 24 * 60 => {
                config.round_minutes = u32::try_from(v).unwrap_or(30);
            }
            ("container_target", Value::Text(v)) if is_tmux_target(&v) => {
                config.container_target = v;
            }
            ("limit" | "round_minutes" | "container_target", _) => {
                return Err(format!("line {n}: invalid value for `{}`", key.trim()));
            }
            (other, _) => return Err(format!("line {n}: unknown key `{other}`")),
        }
    }
    Ok(config)
}

enum Value {
    Int(u64),
    Text(String),
}

impl Value {
    fn parse(s: &str) -> Result<Self, String> {
        if let Some(rest) = s.strip_prefix('"') {
            let end = rest.find('"').ok_or("unterminated string")?;
            let (body, tail) = (&rest[..end], rest[end + 1..].trim());
            if body.contains('\\') {
                return Err("escapes are not supported".into());
            }
            if !(tail.is_empty() || tail.starts_with('#')) {
                return Err("unexpected text after the string".into());
            }
            return Ok(Self::Text(body.to_string()));
        }
        let number = s.split('#').next().unwrap_or_default().trim();
        number
            .parse::<u64>()
            .map(Self::Int)
            .map_err(|_| format!("expected a number or a quoted string, got `{number}`"))
    }
}

/// A tmux target made of the characters tmux target names use, so a typo
/// can't smuggle flags (`-…`) or control characters into the command.
fn is_tmux_target(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.:%=@$".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(parse("").unwrap(), Config::default());
        assert_eq!(parse("# only a comment\n\n").unwrap(), Config::default());
    }

    #[test]
    fn reads_every_key_with_comments() {
        let c = parse(
            "limit = 3   # judge 2 + delegate 1\nround_minutes = 60\ncontainer_target = \"main:dev\" # host window\n",
        )
        .unwrap();
        assert_eq!(c.limit, 3);
        assert_eq!(c.round_minutes, 60);
        assert_eq!(c.container_target, "main:dev");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse("limit = 0").is_err());
        assert!(parse("limit = \"4\"").is_err());
        assert!(parse("round_minutes = 2000").is_err());
        assert!(parse("container_target = \"-t evil\"").is_err());
        assert!(parse("container_target = \"a\\tb\"").is_err());
        assert!(parse("container_target = \"dev").is_err());
        assert!(parse("colour = 1").is_err());
        assert!(parse("[section]").is_err());
        assert!(parse("limit 4").is_err());
    }

    #[test]
    fn load_warns_on_a_broken_file_and_keeps_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(tmp.path()), (Config::default(), None));
        std::fs::write(tmp.path().join(FILE_NAME), "limit = nope\n").unwrap();
        let (c, warning) = load(tmp.path());
        assert_eq!(c, Config::default());
        assert!(warning.unwrap().contains("line 1"));
        std::fs::write(tmp.path().join(FILE_NAME), "limit = 2\n").unwrap();
        assert_eq!(load(tmp.path()).0.limit, 2);
    }
}
