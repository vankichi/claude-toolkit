//! Dashboard settings from `<state>/config.toml`.
//!
//! Only a flat subset of TOML is read (no new dependency for a few keys):
//! `key = 123` and `key = "text"` lines, blank lines, and `#` comments.
//!
//! ```toml
//! limit = 4               # parallel sessions before the header turns red
//! round_minutes = 30      # check-in grid from local midnight; 0 hides it
//! container_target = "dev" # fallback tmux target for a container session
//! ```
//!
//! The command that runs tmux inside the container (`container_tmux`) is
//! the one setting the board executes, so it is read only from the host's
//! `$CCFOCUS_CONTAINER_TMUX`, never from this file: a dev container may
//! mount the state directory writable, and could otherwise choose a program
//! for the host to run.

use std::io::Read;
use std::path::Path;

pub const FILE_NAME: &str = "config.toml";

/// Host environment variable holding `container_tmux` (e.g. `dev tmux`).
pub const CONTAINER_TMUX_ENV: &str = "CCFOCUS_CONTAINER_TMUX";

/// Larger config files are refused rather than read.
const MAX_BYTES: u64 = 64 * 1024;

/// Longest `container_tmux` value.
const MAX_COMMAND_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Parallel sessions allowed before the header warns (warning only).
    pub limit: usize,
    /// Minutes between check-ins, aligned to local midnight; 0 hides them.
    pub round_minutes: u32,
    /// `tmux switch-client -t` target for a container session that didn't
    /// record its host pane (see `SessionState::host_tmux_pane`).
    pub container_target: String,
    /// argv prefix that runs `tmux` inside the container (e.g. `dev tmux`),
    /// used to select the session's window there. Empty: don't. From
    /// [`CONTAINER_TMUX_ENV`] only.
    pub container_tmux: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            limit: 4,
            round_minutes: 30,
            container_target: "dev".to_string(),
            container_tmux: Vec::new(),
        }
    }
}

/// The value of [`CONTAINER_TMUX_ENV`]: `Ok(None)` when unset, `Err` when
/// it is not UTF-8. Read once and kept, so a reload (and a test) doesn't
/// depend on the process environment.
pub(crate) type Env = Result<Option<String>, ()>;

pub(crate) fn env() -> Env {
    match std::env::var(CONTAINER_TMUX_ENV) {
        Ok(v) => Ok(Some(v)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(()),
    }
}

/// The config in `state_dir` with `container_tmux` from the host's
/// [`CONTAINER_TMUX_ENV`] (its value given as `env`, see [`env`]), plus a
/// one-line warning (cleaned for display: it can quote the file) when
/// either could not be used (the defaults apply then). A missing file or
/// unset variable is not a warning. A symlink is followed, so a config
/// managed elsewhere (e.g. a dotfiles link) works.
pub(crate) fn load_with(state_dir: &Path, env: &Env) -> (Config, Option<String>) {
    let (mut config, file_warning) = load_raw(state_dir);
    let env_warning = match env {
        Ok(None) => None,
        Ok(Some(v)) => match parse_command(v) {
            Some(argv) => {
                config.container_tmux = argv;
                None
            }
            None => Some(format!("${CONTAINER_TMUX_ENV}: invalid command")),
        },
        Err(()) => Some(format!("${CONTAINER_TMUX_ENV}: not UTF-8")),
    };
    let warning = [file_warning, env_warning]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    (
        config,
        (!warning.is_empty()).then(|| crate::text::clean(&warning.join("; "))),
    )
}

fn load_raw(state_dir: &Path) -> (Config, Option<String>) {
    let path = state_dir.join(FILE_NAME);
    match std::fs::metadata(&path) {
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
            ("container_tmux", _) => {
                return Err(format!(
                    "line {n}: `container_tmux` is read only from ${CONTAINER_TMUX_ENV} on the host"
                ));
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

/// What `container_tmux` is split on.
const COMMAND_SEPARATORS: [char; 2] = [' ', '\t'];

/// A command line split on spaces and tabs into argv (never through a
/// shell); empty means "not set". Tokens may start with `-` (they are the
/// command's own flags). Any other whitespace (which would look like a
/// separator but isn't one), control characters, and values over
/// [`MAX_COMMAND_BYTES`] are refused (`None`).
fn parse_command(s: &str) -> Option<Vec<String>> {
    let ok = s.len() <= MAX_COMMAND_BYTES
        && s.chars()
            .all(|c| COMMAND_SEPARATORS.contains(&c) || !(c.is_control() || c.is_whitespace()));
    ok.then(|| {
        s.split(COMMAND_SEPARATORS)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    })
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
    fn container_tmux_is_an_argv_prefix() {
        assert_eq!(parse_command("  dev   tmux ").unwrap(), ["dev", "tmux"]);
        assert_eq!(parse_command("dev\ttmux").unwrap(), ["dev", "tmux"]);
        // Flags are the command's own, and nothing goes through a shell.
        assert_eq!(
            parse_command("docker exec -u me box tmux").unwrap(),
            ["docker", "exec", "-u", "me", "box", "tmux"]
        );
        assert_eq!(parse_command("a;b $(c)").unwrap(), ["a;b", "$(c)"]);
        assert!(parse_command("").unwrap().is_empty());
    }

    #[test]
    fn container_tmux_refuses_control_characters_and_long_values() {
        assert_eq!(parse_command("dev\u{1b}[2J tmux"), None);
        assert_eq!(parse_command("dev\u{0}tmux"), None);
        assert_eq!(parse_command("dev\u{7f}tmux"), None);
        assert_eq!(parse_command("dev\ntmux"), None);
        // Whitespace that is not a separator would split nowhere.
        assert_eq!(parse_command("dev\u{a0}tmux"), None);
        assert_eq!(parse_command("dev\u{2028}tmux"), None);
        assert!(parse_command(&"a".repeat(256)).is_some());
        assert_eq!(parse_command(&"a".repeat(257)), None);
    }

    #[test]
    fn container_tmux_is_never_read_from_the_file() {
        // A container that can write the state directory must not pick a
        // program for the host to run.
        let e = parse("container_tmux = \"sh -c evil\"").unwrap_err();
        assert!(e.contains(CONTAINER_TMUX_ENV), "{e}");
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(FILE_NAME),
            "limit = 2\ncontainer_tmux = \"sh -c evil\"\n",
        )
        .unwrap();
        let (c, warning) = load_with(tmp.path(), &Ok(Some("dev tmux".into())));
        assert_eq!(c.container_tmux, ["dev", "tmux"]);
        assert_eq!(
            c.limit,
            Config::default().limit,
            "the broken file is dropped"
        );
        assert!(warning.unwrap().contains("line 2"));
    }

    #[test]
    fn load_takes_container_tmux_from_the_host_env() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load_with(tmp.path(), &Ok(None)), (Config::default(), None));
        let (c, warning) = load_with(tmp.path(), &Ok(Some("dev tmux".into())));
        assert_eq!((c.container_tmux, warning), (args(&["dev", "tmux"]), None));
        let (c, warning) = load_with(tmp.path(), &Ok(Some("dev\u{1b}tmux".into())));
        assert!(c.container_tmux.is_empty());
        let warning = warning.unwrap();
        assert!(warning.contains(CONTAINER_TMUX_ENV), "{warning}");
        assert!(!warning.contains('\u{1b}'));
        let (c, warning) = load_with(tmp.path(), &Err(()));
        assert!(c.container_tmux.is_empty());
        assert!(warning.unwrap().contains("not UTF-8"));
        // Both broken: both reasons, on one line.
        std::fs::write(tmp.path().join(FILE_NAME), "limit = nope\n").unwrap();
        let warning = load_with(tmp.path(), &Err(())).1.unwrap();
        assert!(
            warning.contains("line 1") && warning.contains("not UTF-8"),
            "{warning}"
        );
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
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
        let load = |p: &Path| load_with(p, &Ok(None));
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(tmp.path()), (Config::default(), None));
        std::fs::write(tmp.path().join(FILE_NAME), "limit = nope\n").unwrap();
        let (c, warning) = load(tmp.path());
        assert_eq!(c, Config::default());
        assert!(warning.unwrap().contains("line 1"));
        std::fs::write(tmp.path().join(FILE_NAME), "limit = 2\n").unwrap();
        assert_eq!(load(tmp.path()).0.limit, 2);
        std::fs::write(tmp.path().join(FILE_NAME), "co\u{1b}[2Jlour = 1\n").unwrap();
        assert!(!load(tmp.path()).1.unwrap().contains('\u{1b}'));
    }

    #[cfg(unix)]
    #[test]
    fn follows_a_symlinked_config() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("managed.toml");
        std::fs::write(&elsewhere, "limit = 3\n").unwrap();
        std::os::unix::fs::symlink(&elsewhere, tmp.path().join(FILE_NAME)).unwrap();
        assert_eq!(
            load_with(tmp.path(), &Ok(None)),
            (
                Config {
                    limit: 3,
                    ..Config::default()
                },
                None
            )
        );
    }
}
