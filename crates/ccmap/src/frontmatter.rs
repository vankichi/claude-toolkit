//! Flat YAML-frontmatter parser for Claude Code agent/skill/command markdown
//! files: pulls the `---` delimited block at the top of a file into a list
//! of `key: value` pairs without pulling in a full YAML parser.

/// The flat `key: value` pairs parsed out of a markdown file's leading
/// `---` frontmatter block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Frontmatter {
    pub fields: Vec<(String, String)>,
}

impl Frontmatter {
    /// Looks up a key's value via a linear scan; frontmatter blocks are
    /// small, so this is simpler than a map for the caller's needs.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Strips a single pair of surrounding double-quotes from `value`, if
/// present; otherwise returns `value` unchanged.
fn strip_quotes(value: &str) -> &str {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// Parses the flat frontmatter block at the top of `md`, if any.
///
/// The input must start with a `---` line; the block ends at the next lone
/// `---` line. If there is no leading `---` or the block is never
/// terminated, the returned [`Frontmatter`] has empty `fields`. Each
/// non-blank line inside the block is split on the first `:` into a
/// key/value pair; values are trimmed and have surrounding double-quotes
/// stripped.
#[must_use]
pub fn parse(md: &str) -> Frontmatter {
    let mut lines = md.lines();
    match lines.next() {
        Some("---") => {}
        _ => return Frontmatter::default(),
    }

    let mut fields = Vec::new();
    for line in lines {
        if line == "---" {
            return Frontmatter { fields };
        }
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, ':');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            fields.push((
                key.trim().to_string(),
                strip_quotes(value.trim()).to_string(),
            ));
        }
    }

    Frontmatter::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_keys_and_colon_values() {
        let md = "---\nname: foo\ndescription: \"hi: there\"\nallowed-tools: Bash(gh issue view:*), Read\n---\nbody";
        let fm = parse(md);
        assert_eq!(fm.get("name"), Some("foo"));
        assert_eq!(fm.get("description"), Some("hi: there"));
        assert_eq!(fm.get("allowed-tools"), Some("Bash(gh issue view:*), Read"));
    }

    #[test]
    fn no_frontmatter_is_empty() {
        assert!(parse("# just markdown").fields.is_empty());
    }

    #[test]
    fn unterminated_block_is_empty() {
        let md = "---\nname: foo\ndescription: bar\n";
        assert!(parse(md).fields.is_empty());
    }

    #[test]
    fn blank_lines_are_skipped() {
        let md = "---\nname: foo\n\n\ndescription: bar\n---\nbody";
        let fm = parse(md);
        assert_eq!(fm.fields.len(), 2);
        assert_eq!(fm.get("name"), Some("foo"));
        assert_eq!(fm.get("description"), Some("bar"));
    }

    #[test]
    fn get_returns_none_for_missing_key() {
        let md = "---\nname: foo\n---\nbody";
        let fm = parse(md);
        assert_eq!(fm.get("missing"), None);
    }
}
