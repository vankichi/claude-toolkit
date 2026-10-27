//! Facts from git and GitHub: each worktree's distance from the base
//! (`origin/HEAD`) and uncommitted changes, and the PRs opened and merged
//! today. Nothing here fetches; git only reads local refs.
//!
//! These run on background threads (see [`Job`]) because `git status` on a
//! large tree or a slow `gh` would otherwise freeze the dashboard.

use crate::board::Row;
use crate::cmd;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// One `git worktree list --porcelain` entry, with counts against the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub repo: PathBuf,
    pub path: PathBuf,
    /// Short branch name; `None` when detached.
    pub branch: Option<String>,
    /// Commits on HEAD not on the base, and the reverse. `None` without a
    /// base (`origin/HEAD` unset) or when git failed.
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    /// Entries in `git status --porcelain`.
    pub dirty: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    branch: Option<String>,
    skip: bool,
}

fn parse_worktree_list(out: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for block in out.split("\n\n") {
        let mut entry: Option<Entry> = None;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                entry = Some(Entry {
                    path: PathBuf::from(p),
                    branch: None,
                    skip: false,
                });
            } else if let Some(e) = entry.as_mut() {
                if let Some(b) = line.strip_prefix("branch ") {
                    e.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
                } else if line == "bare" || line.starts_with("prunable") {
                    e.skip = true;
                }
            }
        }
        entries.extend(entry);
    }
    entries
}

/// `rev-list --left-right --count base...HEAD` → (behind, ahead).
fn parse_counts(out: &str) -> Option<(u32, u32)> {
    let mut it = out.split_whitespace();
    let behind = it.next()?.parse().ok()?;
    let ahead = it.next()?.parse().ok()?;
    Some((behind, ahead))
}

/// Every worktree of `repo` (bare and prunable entries skipped).
#[must_use]
pub fn scan_repo(repo: &Path) -> Vec<Worktree> {
    let Some(list) = cmd::git(repo, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    let base = cmd::git(repo, &["rev-parse", "--verify", "--quiet", "origin/HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| is_object_id(s));
    parse_worktree_list(&list)
        .into_iter()
        .filter(|e| !e.skip)
        .map(|e| {
            let counts = base.as_deref().and_then(|b| {
                cmd::git(
                    &e.path,
                    &[
                        "rev-list",
                        "--left-right",
                        "--count",
                        &format!("{b}...HEAD"),
                    ],
                )
                .and_then(|o| parse_counts(&o))
            });
            let dirty = cmd::git(&e.path, &["status", "--porcelain"])
                .map(|o| u32::try_from(o.lines().count()).unwrap_or(u32::MAX));
            Worktree {
                repo: repo.to_path_buf(),
                path: e.path,
                branch: e.branch,
                behind: counts.map(|c| c.0),
                ahead: counts.map(|c| c.1),
                dirty,
            }
        })
        .collect()
}

/// `git merge-base HEAD origin/HEAD` in `worktree`, validated as an object id.
#[must_use]
pub fn merge_base(worktree: &Path) -> Option<String> {
    cmd::git(worktree, &["merge-base", "HEAD", "origin/HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| is_object_id(s))
}

fn is_object_id(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A worktree ahead of the base with no PR: work to finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub repo: String,
    pub worktree: String,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: Option<u32>,
    /// Numbers of the sessions working in it.
    pub sessions: Vec<u8>,
}

/// Worktrees ahead of the base whose branch has no PR. With `heads` unknown
/// for a repo (gh not run or failed), its ahead worktrees are all listed.
#[must_use]
pub fn candidates<S: std::hash::BuildHasher, T: std::hash::BuildHasher>(
    worktrees: &[Worktree],
    heads: Option<&HashMap<PathBuf, HashSet<String, T>, S>>,
    rows: &[Row],
) -> Vec<Candidate> {
    worktrees
        .iter()
        .filter(|w| w.ahead.is_some_and(|a| a > 0))
        .filter(|w| {
            let known = heads.and_then(|h| h.get(&w.repo));
            match (known, &w.branch) {
                (Some(h), Some(b)) => !h.contains(b),
                _ => true,
            }
        })
        .map(|w| {
            let sessions = rows
                .iter()
                .filter(|r| r.local && r.worktree_path.as_deref() == Some(w.path.as_path()))
                .filter_map(|r| r.number)
                .collect();
            Candidate {
                repo: crate::text::base_name(&w.repo.to_string_lossy()),
                worktree: crate::text::base_name(&w.path.to_string_lossy()),
                ahead: w.ahead.unwrap_or(0),
                behind: w.behind.unwrap_or(0),
                dirty: w.dirty,
                sessions,
            }
        })
        .collect()
}

/// Repos of this side's sessions (container paths can't be read from the
/// host, and the reverse).
#[must_use]
pub fn local_repos(rows: &[Row]) -> Vec<PathBuf> {
    rows.iter()
        .filter(|r| r.local)
        .filter_map(|r| r.repo_path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// PRs by `@me` today, from `gh`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tasks {
    /// Distinct PRs created or merged today.
    pub done: usize,
    /// Head branches of `@me`'s open or merged PRs per repo, for
    /// [`candidates`]. A repo is missing when its query failed.
    pub heads: HashMap<PathBuf, HashSet<String>>,
}

/// Each gh call.
const GH_TIMEOUT: Duration = Duration::from_secs(20);

fn gh(dir: Option<&Path>, args: &[&str]) -> Option<serde_json::Value> {
    let mut cmd = Command::new("gh");
    cmd.args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1");
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    let out = cmd::run(cmd, GH_TIMEOUT).filter(|o| o.success)?;
    serde_json::from_str(&out.stdout).ok()
}

/// `repo#number` of each PR in a `gh search prs --json number,repository` reply.
fn pr_keys(v: &serde_json::Value) -> Option<HashSet<String>> {
    v.as_array()?
        .iter()
        .map(|pr| {
            let number = pr.get("number")?.as_u64()?;
            let repo = pr.get("repository")?.get("nameWithOwner")?.as_str()?;
            Some(format!("{repo}#{number}"))
        })
        .collect()
}

/// Heads of open or merged PRs. A closed, unmerged PR leaves its branch
/// unfinished, so it doesn't count.
fn head_names(v: &serde_json::Value) -> Option<HashSet<String>> {
    let mut heads = HashSet::new();
    for pr in v.as_array()? {
        let head = pr.get("headRefName")?.as_str()?;
        if matches!(pr.get("state")?.as_str()?, "OPEN" | "MERGED") {
            heads.insert(head.to_string());
        }
    }
    Some(heads)
}

/// Query gh. `since` is the start of today as RFC 3339 with the local offset
/// (GitHub reads bare dates as UTC). `None` when the search itself fails
/// (no gh, not logged in, offline).
#[must_use]
pub fn fetch_tasks(since: &str, repos: &[PathBuf]) -> Option<Tasks> {
    let range = format!(">={since}");
    let search = |flag: &str| {
        gh(
            None,
            &[
                "search",
                "prs",
                "--author",
                "@me",
                flag,
                &range,
                "--json",
                "number,repository",
                "--limit",
                "200",
            ],
        )
        .as_ref()
        .and_then(pr_keys)
    };
    let mut done = search("--created")?;
    done.extend(search("--merged-at")?);
    let heads = repos
        .iter()
        .filter_map(|repo| {
            let v = gh(
                Some(repo),
                &[
                    "pr",
                    "list",
                    "--author",
                    "@me",
                    "--state",
                    "all",
                    "--json",
                    "headRefName,state",
                    "--limit",
                    "200",
                ],
            )?;
            Some((repo.clone(), head_names(&v)?))
        })
        .collect();
    Some(Tasks {
        done: done.len(),
        heads,
    })
}

/// A background computation run at most every `every`, one at a time. The
/// thread ends when its work does (every command has a timeout); a dropped
/// [`Job`] lets a running thread finish and discards its result.
#[derive(Debug)]
pub struct Job<T> {
    every: Duration,
    started: Option<Instant>,
    running: Option<Receiver<T>>,
}

impl<T: Send + 'static> Job<T> {
    #[must_use]
    pub fn new(every: Duration) -> Self {
        Self {
            every,
            started: None,
            running: None,
        }
    }

    /// Start `work` unless one is running or the last start was less than
    /// `every` ago (`force` skips the wait, not the one-at-a-time rule).
    /// `true` when it started.
    pub fn kick(
        &mut self,
        now: Instant,
        force: bool,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> bool {
        let due = force
            || self
                .started
                .is_none_or(|t| now.duration_since(t) >= self.every);
        if self.running.is_some() || !due {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        self.started = Some(now);
        self.running = Some(rx);
        true
    }

    /// The finished result, if the running work is done.
    pub fn poll(&mut self) -> Option<T> {
        let rx = self.running.as_ref()?;
        match rx.try_recv() {
            Ok(v) => {
                self.running = None;
                Some(v)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.running = None;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Shown;
    use cctk::focus::HostKind;
    use chrono::DateTime;

    const LIST: &str = "worktree /work/repo-a
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /work/repo-a/.claude/worktrees/feat-x
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feat/x

worktree /work/repo-a/.claude/worktrees/detached
HEAD 3333333333333333333333333333333333333333
detached

worktree /work/repo-a/.claude/worktrees/gone
HEAD 4444444444444444444444444444444444444444
branch refs/heads/gone
prunable gitdir file points to non-existent location
";

    #[test]
    fn parses_porcelain_worktree_list() {
        let e = parse_worktree_list(LIST);
        assert_eq!(e.len(), 4);
        assert_eq!(e[0].branch.as_deref(), Some("main"));
        assert_eq!(e[1].branch.as_deref(), Some("feat/x"));
        assert_eq!(e[2].branch, None);
        assert!(e[3].skip && !e[1].skip);
        assert!(parse_worktree_list("worktree /bare\nbare\n")[0].skip);
    }

    #[test]
    fn parses_left_right_counts() {
        assert_eq!(parse_counts("5\t1\n"), Some((5, 1)));
        assert_eq!(parse_counts("x"), None);
    }

    fn wt(path: &str, branch: Option<&str>, ahead: Option<u32>) -> Worktree {
        Worktree {
            repo: "/work/repo-a".into(),
            path: path.into(),
            branch: branch.map(str::to_string),
            ahead,
            behind: Some(2),
            dirty: Some(0),
        }
    }

    fn row(number: u8, worktree: &str, local: bool) -> Row {
        let t = DateTime::parse_from_rfc3339("2026-01-05T12:00:00+09:00").unwrap();
        Row {
            number: Some(number),
            shown: Shown::Working,
            session_id: format!("s{number}"),
            host: HostKind::Host,
            local,
            repo: "repo-a".into(),
            project: String::new(),
            worktree: String::new(),
            branch: None,
            tmux: None,
            tmux_pane: None,
            repo_path: Some("/work/repo-a".into()),
            worktree_path: Some(worktree.into()),
            started_at: t,
            updated_at: t,
        }
    }

    #[test]
    fn candidates_are_ahead_without_a_pr() {
        let trees = [
            wt("/w/a", Some("feat/a"), Some(1)),
            wt("/w/b", Some("feat/b"), Some(3)),
            wt("/w/c", Some("feat/c"), Some(0)),
            wt("/w/d", None, Some(2)),
            wt("/w/e", Some("feat/e"), None),
        ];
        let heads: HashMap<PathBuf, HashSet<String>> =
            [("/work/repo-a".into(), ["feat/b".to_string()].into())].into();
        let rows = [row(2, "/w/a", true), row(3, "/w/a", false)];
        let got = candidates(&trees, Some(&heads), &rows);
        let names: Vec<_> = got.iter().map(|c| c.worktree.as_str()).collect();
        assert_eq!(names, ["a", "d"]);
        assert_eq!(got[0].sessions, [2]);
        assert_eq!((got[0].ahead, got[0].behind), (1, 2));

        // Without gh, every ahead worktree is listed.
        assert_eq!(
            candidates::<std::hash::RandomState, std::hash::RandomState>(&trees, None, &rows).len(),
            3
        );
    }

    #[test]
    fn local_repos_are_deduplicated_and_this_side_only() {
        let rows = [
            row(1, "/w/a", true),
            row(2, "/w/b", true),
            row(3, "/w/c", false),
        ];
        assert_eq!(local_repos(&rows), [PathBuf::from("/work/repo-a")]);
    }

    #[test]
    fn reads_gh_json() {
        let v: serde_json::Value = serde_json::from_str(
            r#"[{"number":1,"repository":{"nameWithOwner":"o/r"}},{"number":2,"repository":{"nameWithOwner":"o/r"}}]"#,
        )
        .unwrap();
        assert_eq!(pr_keys(&v).unwrap().len(), 2);
        assert!(pr_keys(&serde_json::json!([{"number":1}])).is_none());
        let v = serde_json::json!([
            {"headRefName":"feat/x","state":"OPEN"},
            {"headRefName":"feat/y","state":"MERGED"},
            {"headRefName":"feat/z","state":"CLOSED"}
        ]);
        assert_eq!(
            head_names(&v).unwrap(),
            ["feat/x".to_string(), "feat/y".to_string()].into()
        );
        assert!(head_names(&serde_json::json!([{"headRefName":"a"}])).is_none());
    }

    #[test]
    fn object_ids() {
        assert!(is_object_id(&"a".repeat(40)));
        assert!(is_object_id(&"0".repeat(64)));
        assert!(!is_object_id("abc"));
        assert!(!is_object_id(&"g".repeat(40)));
    }

    #[test]
    fn job_runs_one_at_a_time_and_waits_between_runs() {
        let mut job: Job<u32> = Job::new(Duration::from_mins(10));
        let t0 = Instant::now();
        assert!(job.kick(t0, false, || 1));
        assert!(!job.kick(t0, true, || 2), "still running (or unpolled)");
        let mut got = None;
        for _ in 0..200 {
            got = job.poll();
            if got.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got, Some(1));
        job.kick(t0 + Duration::from_secs(1), false, || 3);
        assert!(job.running.is_none(), "not due yet");
        job.kick(t0 + Duration::from_secs(1), true, || 4);
        assert!(job.running.is_some());
    }
}
