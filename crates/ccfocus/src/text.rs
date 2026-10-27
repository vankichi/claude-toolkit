//! Display hygiene and width-aware layout for values read from state files.
//!
//! Only `project` is cleaned when written; `cwd`, `repo`, `worktree`,
//! `branch`, `tmux`, and `hostname` come straight from the environment and
//! git, so every value is cleaned again here before it reaches the terminal.

use ratatui::text::Span;

/// Characters never shown: controls (C0, DEL, C1) and invisible formatting
/// that can reorder or hide surrounding text (see [`is_invisible_format`]).
#[must_use]
pub fn is_hidden(c: char) -> bool {
    c.is_control() || is_invisible_format(c)
}

/// Invisible Unicode format characters (category Cf) that change or hide how
/// surrounding text is shown: soft hyphen, Arabic letter mark, Mongolian vowel
/// separator, zero-width and directional marks, bidi embeddings / overrides /
/// isolates, invisible operators, BOM, interlinear annotation marks, and tag
/// characters (which can smuggle hidden ASCII).
#[must_use]
pub fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// `raw` without hidden characters, trimmed.
#[must_use]
pub fn clean(raw: &str) -> String {
    let kept: String = raw.chars().filter(|&c| !is_hidden(c)).collect();
    kept.trim().to_string()
}

/// Terminal columns `s` occupies.
#[must_use]
pub fn width(s: &str) -> usize {
    Span::raw(s).width()
}

fn char_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    width(c.encode_utf8(&mut buf))
}

/// `s` cut to at most `max` columns, ending in `…` when cut.
#[must_use]
pub fn fit(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = char_width(c);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// `s` fitted to `cols` columns and padded with spaces to exactly `cols`.
#[must_use]
pub fn pad(s: &str, cols: usize) -> String {
    let mut out = fit(s, cols);
    let w = width(&out);
    out.extend(std::iter::repeat_n(' ', cols.saturating_sub(w)));
    out
}

/// `s` fitted to `cols` columns and right-aligned in exactly `cols` (`{:>n}`
/// counts chars, not columns, so it misaligns wide text).
#[must_use]
pub fn pad_left(s: &str, cols: usize) -> String {
    let fitted = fit(s, cols);
    let mut out = " ".repeat(cols.saturating_sub(width(&fitted)));
    out.push_str(&fitted);
    out
}

/// Last path component of `path`, cleaned; the cleaned path itself when it
/// has none.
#[must_use]
pub fn base_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    clean(if last.is_empty() { path } else { last })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_drops_controls_and_bidi() {
        assert_eq!(clean(" a\u{1b}[31mb\u{202E}c\u{200B}\n"), "a[31mbc");
        assert_eq!(clean("x\u{E0041}y"), "xy");
        assert_eq!(clean("\u{7}"), "");
    }

    #[test]
    fn fit_counts_wide_characters() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abc", 3), "abc");
        assert_eq!(fit("承認待ち", 5), "承認…");
        assert_eq!(width(&fit("承認待ち", 5)), 5);
        assert_eq!(fit("abc", 0), "");
    }

    #[test]
    fn pad_fills_to_exact_width() {
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(width(&pad("作業中", 8)), 8);
        assert_eq!(pad("abcdef", 3), "ab…");
        assert_eq!(pad_left("未取得", 8), "  未取得");
        assert_eq!(pad_left("12", 4), "  12");
    }

    #[test]
    fn base_name_takes_last_component() {
        assert_eq!(base_name("/work/repo-a/"), "repo-a");
        assert_eq!(base_name("/work/re\u{202E}po"), "repo");
        assert_eq!(base_name("C:\\work\\repo-b"), "repo-b");
        assert_eq!(base_name("/"), "/");
    }
}
