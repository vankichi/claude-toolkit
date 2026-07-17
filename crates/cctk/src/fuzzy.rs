//! Tiny in-house fuzzy matcher — subsequence match with light scoring.
//!
//! Deliberately dependency-free (no `fuzzy-matcher` / `nucleo`): the query must
//! appear as a case-insensitive subsequence of the candidate, and the score
//! rewards consecutive runs and word-boundary starts so prefix/word matches
//! rank above scattered ones. Higher score = better; `None` means no match.

/// Score `candidate` against `query`. Returns `None` when `query` is not a
/// case-insensitive subsequence of `candidate`. An empty query matches
/// everything with score 0.
#[must_use]
pub fn score(query: &str, candidate: &str) -> Option<i64> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let cand: Vec<char> = candidate.chars().collect();

    let mut qi = 0;
    let mut total: i64 = 0;
    let mut prev_match: Option<usize> = None;

    for (i, &ch) in cand.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if ch.eq_ignore_ascii_case(&q[qi]) {
            let mut pts: i64 = 1;
            // Consecutive-match bonus.
            if prev_match == Some(i.wrapping_sub(1)) && i > 0 {
                pts += 5;
            }
            // Word-boundary bonus (string start or right after a separator).
            let boundary = i == 0 || matches!(cand[i - 1], '_' | '-' | '/' | ' ' | ':' | '.' | '@');
            if boundary {
                pts += 3;
            }
            total += pts;
            prev_match = Some(i);
            qi += 1;
        }
    }

    (qi == q.len()).then_some(total)
}

/// Whether `query` fuzzy-matches `candidate` at all.
#[must_use]
pub fn matches(query: &str, candidate: &str) -> bool {
    score(query, candidate).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("   ", "anything"), Some(0));
        assert!(matches("", ""));
    }

    #[test]
    fn exact_and_subsequence_match() {
        assert!(score("abc", "abc").is_some());
        assert!(score("abc", "xaxbxc").is_some());
        assert!(matches("brain", "superpowers:brainstorming"));
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(score("acb", "abc"), None);
        assert_eq!(score("cb", "abc"), None);
        assert_eq!(score("xyz", "abc"), None);
    }

    #[test]
    fn case_insensitive() {
        assert!(matches("AB", "abcd"));
        assert!(matches("ab", "ABCD"));
    }

    #[test]
    fn prefix_ranks_above_scattered() {
        // "br" as a leading consecutive run beats a scattered occurrence.
        let prefix = score("br", "brainstorm").unwrap();
        let scattered = score("br", "superbar").unwrap();
        assert!(
            prefix > scattered,
            "prefix {prefix} vs scattered {scattered}"
        );
    }

    #[test]
    fn word_boundary_bonus_after_separator() {
        // Isolate the boundary bonus: a single-char match right after a
        // separator scores higher than the same char mid-word (no consecutive
        // run in either case).
        let after_sep = score("i", "work-intake").unwrap();
        let mid_word = score("i", "raining").unwrap();
        assert!(
            after_sep > mid_word,
            "after-separator {after_sep} vs mid-word {mid_word}"
        );
    }

    #[test]
    fn consecutive_beats_gapped() {
        let consecutive = score("mcp", "mcp__notion").unwrap();
        let gapped = score("mcp", "m_c_p_x").unwrap();
        assert!(consecutive > gapped);
    }
}
