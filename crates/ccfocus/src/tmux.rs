//! The dashboard's key actions as tmux commands. Planning is pure (an
//! argument list, a [`Jump`], or the reason it can't be done); [`run`] and
//! [`run_jump`] execute them. Arguments go to tmux (or, for a container
//! session's jump, to the configured `container_tmux` command) directly,
//! never through a shell. tmux itself format-expands some of them (the `rename-window` name and the
//! `display-popup -d` directory), where `#(…)` would run a shell command, so
//! values from the state files are passed through [`literal`] there.

use crate::board::{Row, Shown};
use crate::config::Config;
use cctk::focus::HostKind;
use std::process::Command;
use std::time::Duration;

/// Longest window name `p` sets.
const MAX_WINDOW_NAME: usize = 40;

const TMUX_TIMEOUT: Duration = Duration::from_secs(2);

/// Per command in the container's tmux, which starts with the launcher
/// (e.g. `docker exec`) before tmux itself runs.
const CONTAINER_TIMEOUT: Duration = Duration::from_secs(3);

/// How long [`open`] waits for tmux to reject a popup (a bad target fails
/// in milliseconds).
const POPUP_SETTLE: Duration = Duration::from_millis(300);

/// The tmux client this dashboard runs in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Client {
    /// `$TMUX` is set.
    pub inside: bool,
    /// `$TMUX_PANE`, validated.
    pub pane: Option<String>,
}

impl Client {
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            inside: std::env::var_os("TMUX").is_some_and(|v| !v.is_empty()),
            pane: std::env::var("TMUX_PANE")
                .ok()
                .filter(|p| crate::probe::is_pane_id(p)),
        }
    }
}

pub type Plan = Result<Vec<String>, String>;

fn need_tmux(client: &Client) -> Result<(), String> {
    if client.inside {
        Ok(())
    } else {
        Err("run the board inside tmux to use this key".into())
    }
}

/// What [`jump`] runs, in order (see [`run_jump`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jump {
    /// Full argv lists (program first) that select the session's pane in the
    /// container's own tmux server. The program and its flags come from
    /// `config.container_tmux` only, never from a state file.
    pub container: Vec<Vec<String>>,
    /// Full argv that prints the container session's current
    /// `CCFOCUS_HOST_PANE`: its tmux updates that on every attach, while the
    /// session file keeps what Claude was started with.
    pub host_pane_query: Option<Vec<String>>,
    /// `tmux switch-client` arguments to try in turn until one works.
    pub switch: Vec<Vec<String>>,
}

/// `%N`, or `None`.
fn pane_id(p: Option<&str>) -> Option<&str> {
    p.filter(|p| crate::probe::is_pane_id(p))
}

/// Switch to the session's pane.
///
/// A container session's pane lives in the container's own tmux server,
/// shown on the host through the window that runs the container. Its row
/// gets two steps: select the pane in the container's tmux (when
/// `config.container_tmux` is set and the pane is known), then switch the
/// host client to the host pane now attached to the container (asked from
/// the container's tmux), else the one recorded in the session file, else
/// `config.container_target`. Pane ids come from the container, so each is
/// checked to be `%N` before it becomes a target.
pub fn jump(row: &Row, client: &Client, config: &Config) -> Result<Jump, String> {
    need_tmux(client)?;
    if row.shown == Shown::Stopped {
        return Err("that session has stopped".into());
    }
    let pane = pane_id(row.tmux_pane.as_deref());
    if !row.local {
        if row.host != HostKind::Container {
            return Err("that session runs on the other side (host / container)".into());
        }
        let inside = |tail: &[&str]| {
            let mut argv = config.container_tmux.clone();
            argv.extend(args(tail));
            argv
        };
        let (container, host_pane_query) = match pane {
            Some(p) if !config.container_tmux.is_empty() => (
                vec![
                    inside(&["select-window", "-t", p]),
                    inside(&["select-pane", "-t", p]),
                ],
                Some(inside(&[
                    "show-environment",
                    "-t",
                    p,
                    crate::probe::HOST_PANE_ENV,
                ])),
            ),
            _ => (Vec::new(), None),
        };
        let mut switch = Vec::new();
        if let Some(host) = pane_id(row.host_tmux_pane.as_deref()) {
            switch.push(args(&["switch-client", "-t", host]));
        }
        switch.push(args(&["switch-client", "-t", &config.container_target]));
        return Ok(Jump {
            container,
            host_pane_query,
            switch,
        });
    }
    match pane {
        Some(p) => Ok(Jump {
            container: Vec::new(),
            host_pane_query: None,
            switch: vec![args(&["switch-client", "-t", p])],
        }),
        None => Err("that session is not in tmux".into()),
    }
}

/// The pane in `show-environment` output (`NAME=%N`); `None` for anything
/// else, including `-NAME` (unset by the last attach).
fn parse_host_pane(out: &str) -> Option<String> {
    let value = out.lines().find_map(|l| {
        l.strip_prefix(crate::probe::HOST_PANE_ENV)?
            .strip_prefix('=')
    })?;
    pane_id(Some(value.trim())).map(str::to_string)
}

/// Run a [`Jump`]: the container steps first (so the session's window is
/// already showing on arrival; the first failure stops them and skips the
/// host-pane query), then each switch until one works, starting with the
/// queried host pane. `Ok(Some(_))` when the client arrived but not as
/// planned (a container step failed, or a fallback target was used), with
/// the reason to show.
pub fn run_jump(jump: &Jump) -> Result<Option<String>, String> {
    run_jump_with(jump, |argv| run_argv(argv, CONTAINER_TIMEOUT), run)
}

fn run_jump_with(
    jump: &Jump,
    mut run_container: impl FnMut(&[String]) -> Result<String, String>,
    mut run_switch: impl FnMut(&[String]) -> Result<(), String>,
) -> Result<Option<String>, String> {
    let inner = jump
        .container
        .iter()
        .find_map(|argv| run_container(argv).err())
        .map(|e| format!("in the container: {e}"));
    let mut switch = jump.switch.clone();
    let queried = match (&inner, &jump.host_pane_query) {
        (None, Some(query)) => run_container(query)
            .ok()
            .as_deref()
            .and_then(parse_host_pane),
        _ => None,
    };
    if let Some(now) = queried {
        let target = args(&["switch-client", "-t", &now]);
        switch.retain(|s| *s != target);
        switch.insert(0, target);
    }
    let mut first_failure: Option<String> = None;
    let mut last_failure = "nothing to switch to".to_string();
    let mut arrived = None;
    for target in &switch {
        match run_switch(target) {
            Ok(()) => {
                arrived = Some(target);
                break;
            }
            Err(e) => {
                first_failure.get_or_insert_with(|| e.clone());
                last_failure = e;
            }
        }
    }
    // Arrived on a fallback: say why the first target didn't work.
    let fell_back = match (arrived, first_failure) {
        (Some(target), Some(e)) => Some(format!(
            "fell back to {}: {e}",
            crate::text::clean(target.last().map_or("", String::as_str))
        )),
        _ => None,
    };
    let notes: Vec<String> = [inner, fell_back].into_iter().flatten().collect();
    match arrived {
        Some(_) => Ok((!notes.is_empty()).then(|| notes.join("; "))),
        None if notes.is_empty() => Err(last_failure),
        None => Err(format!("{last_failure} ({})", notes.join("; "))),
    }
}

/// Run `argv` (program first) without a shell; `Ok` carries its stdout.
/// `Err` names the program and carries its first stderr line.
fn run_argv(argv: &[String], timeout: Duration) -> Result<String, String> {
    let Some((program, rest)) = argv.split_first() else {
        return Err("empty command".into());
    };
    let name = crate::text::clean(program);
    let mut cmd = Command::new(program);
    cmd.args(rest);
    match crate::cmd::run(cmd, timeout) {
        Some(o) if o.success => Ok(o.stdout),
        Some(o) => Err(format!("{name}: {}", crate::cmd::first_line(&o.stderr))),
        None => Err(format!("couldn't run {name}")),
    }
}

/// Open `hunk diff <merge-base> --watch` in a popup over the dashboard, in
/// the session's worktree; it closes when hunk exits cleanly. A popup leaves the
/// window's layout alone (a split's space would not return to the board).
/// `base` is the merge-base the caller computed. Run it with [`open`].
pub fn diff(row: &Row, client: &Client, base: Option<&str>) -> Plan {
    need_tmux(client)?;
    if !row.local {
        return Err("can't open a worktree on the other side (host / container)".into());
    }
    let Some(worktree) = row.worktree_path.as_ref().and_then(|p| p.to_str()) else {
        return Err("that session is not in a git worktree".into());
    };
    let Some(base) = base else {
        return Err("no merge-base with origin/HEAD".into());
    };
    // Without the board's pane tmux would guess the client, and with several
    // attached the popup could open on another one.
    let Some(pane) = &client.pane else {
        return Err("can't tell which tmux pane the board runs in".into());
    };
    // `-EE`: close on a clean exit only, so a failed hunk leaves its error
    // on screen (`open` has returned by then). Two or more command
    // arguments run as argv, without a shell.
    Ok(args(&[
        "display-popup",
        "-EE",
        "-w",
        "90%",
        "-h",
        "90%",
        "-t",
        pane,
        "-d",
        &literal(worktree),
        "hunk",
        "diff",
        base,
        "--watch",
    ]))
}

/// Name the session's tmux window after its project.
pub fn rename(row: &Row, client: &Client) -> Plan {
    need_tmux(client)?;
    if !row.local {
        return Err("that session runs on the other side (host / container)".into());
    }
    let Some(pane) = &row.tmux_pane else {
        return Err("that session is not in tmux".into());
    };
    if row.project.is_empty() {
        return Err("no project name (set one with claude -n)".into());
    }
    let name = literal(&crate::text::fit(&row.project, MAX_WINDOW_NAME));
    // `--`: a project like `-a todo` must not be read as flags.
    Ok(args(&["rename-window", "-t", pane, "--", &name]))
}

/// `s` as a tmux format that expands to itself: every `#` doubled, so
/// `#(cmd)`, `#{…}`, and `#H` stay text.
#[must_use]
pub fn literal(s: &str) -> String {
    s.replace('#', "##")
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_string()).collect()
}

/// Run `tmux <args>`. `Err` carries tmux's first stderr line.
pub fn run(args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new("tmux");
    cmd.args(args);
    match crate::cmd::run(cmd, TMUX_TIMEOUT) {
        Some(o) if o.success => Ok(()),
        Some(o) => Err(format!("tmux: {}", crate::cmd::first_line(&o.stderr))),
        None => Err("couldn't run tmux".into()),
    }
}

/// Run `tmux <args>` for a command that stays until it is closed
/// (`display-popup` returns only then). An error tmux reports at once is
/// returned; a command still running after [`POPUP_SETTLE`] is open.
pub fn open(args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new("tmux");
    cmd.args(args);
    match crate::cmd::start(cmd, POPUP_SETTLE) {
        Ok(None) => Ok(()),
        Ok(Some(o)) if o.success => Ok(()),
        Ok(Some(o)) => Err(format!("tmux: {}", crate::cmd::first_line(&o.stderr))),
        Err(()) => Err("couldn't run tmux".into()),
    }
}

/// `#S:#I` of the dashboard's own window, for the header.
#[must_use]
pub fn own_window(client: &Client) -> Option<String> {
    let pane = client.pane.as_ref()?;
    let mut cmd = Command::new("tmux");
    cmd.args(["display-message", "-p", "-t", pane, "#S:#I"]);
    let out = crate::cmd::run(cmd, TMUX_TIMEOUT).filter(|o| o.success)?;
    Some(crate::text::clean(&out.stdout)).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    fn row() -> Row {
        let t = DateTime::parse_from_rfc3339("2026-01-05T12:00:00+09:00").unwrap();
        Row {
            number: Some(1),
            shown: Shown::Waiting,
            session_id: "s".into(),
            host: HostKind::Host,
            local: true,
            repo: "repo-a".into(),
            project: "demo".into(),
            worktree: "feat-x".into(),
            branch: Some("feat/x".into()),
            tmux: Some("work:2.1".into()),
            tmux_pane: Some("%3".into()),
            host_tmux_pane: None,
            repo_path: Some("/work/repo-a".into()),
            worktree_path: Some("/work/repo-a/wt".into()),
            started_at: t,
            updated_at: t,
        }
    }

    fn inside() -> Client {
        Client {
            inside: true,
            pane: Some("%9".into()),
        }
    }

    fn switch(target: &str) -> Vec<String> {
        args(&["switch-client", "-t", target])
    }

    fn container_row() -> Row {
        let mut r = row();
        r.local = false;
        r.host = HostKind::Container;
        r.host_tmux_pane = Some("%12".into());
        r
    }

    fn with_container_tmux() -> Config {
        Config {
            container_tmux: args(&["dev", "tmux"]),
            ..Config::default()
        }
    }

    #[test]
    fn jump_targets_the_pane_or_the_container_window() {
        let c = Config::default();
        assert_eq!(
            jump(&row(), &inside(), &c).unwrap(),
            Jump {
                container: vec![],
                host_pane_query: None,
                switch: vec![switch("%3")],
            }
        );
        let mut r = row();
        r.local = false;
        r.host = HostKind::Container;
        assert_eq!(
            jump(&r, &inside(), &c).unwrap(),
            Jump {
                container: vec![],
                host_pane_query: None,
                switch: vec![switch("dev")],
            }
        );
        r.host = HostKind::Host;
        assert!(jump(&r, &inside(), &c).is_err());
        let mut r = row();
        r.tmux_pane = None;
        assert!(jump(&r, &inside(), &c).unwrap_err().contains("not in tmux"));
        let mut r = row();
        r.shown = Shown::Stopped;
        assert!(jump(&r, &inside(), &c).is_err());
        assert!(jump(&row(), &Client::default(), &c).is_err());
    }

    #[test]
    fn a_container_jump_selects_the_pane_inside_then_switches_to_the_host_pane() {
        assert_eq!(
            jump(&container_row(), &inside(), &with_container_tmux()).unwrap(),
            Jump {
                container: vec![
                    args(&["dev", "tmux", "select-window", "-t", "%3"]),
                    args(&["dev", "tmux", "select-pane", "-t", "%3"]),
                ],
                host_pane_query: Some(args(&[
                    "dev",
                    "tmux",
                    "show-environment",
                    "-t",
                    "%3",
                    "CCFOCUS_HOST_PANE"
                ])),
                switch: vec![switch("%12"), switch("dev")],
            }
        );
    }

    #[test]
    fn a_container_jump_falls_back_to_container_target() {
        let c = with_container_tmux();
        for bad in [
            None,
            Some("12"),
            Some("%"),
            Some("#(touch x)"),
            Some("-t %1"),
        ] {
            let mut r = container_row();
            r.host_tmux_pane = bad.map(str::to_string);
            assert_eq!(
                jump(&r, &inside(), &c).unwrap().switch,
                [switch("dev")],
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_container_jump_without_container_tmux_only_switches() {
        assert_eq!(
            jump(&container_row(), &inside(), &Config::default()).unwrap(),
            Jump {
                container: vec![],
                host_pane_query: None,
                switch: vec![switch("%12"), switch("dev")],
            }
        );
    }

    #[test]
    fn a_pane_id_that_is_not_percent_n_is_never_a_target() {
        let c = with_container_tmux();
        for bad in ["#(touch x)", "-t %1", "%3; kill-server", "%3 "] {
            let mut r = container_row();
            r.tmux_pane = Some(bad.into());
            let j = jump(&r, &inside(), &c).unwrap();
            assert!(j.container.is_empty(), "{bad:?}");
            assert_eq!(j.host_pane_query, None, "{bad:?}");
            assert_eq!(j.switch, [switch("%12"), switch("dev")]);
            let mut r = row();
            r.tmux_pane = Some(bad.into());
            assert!(jump(&r, &inside(), &c).is_err(), "{bad:?}");
        }
    }

    /// `run_jump_with` over fakes that log each call, fail the argv lists
    /// whose last word is in `fail`, and answer the host-pane query with
    /// `env`.
    fn fake_run(
        j: &Jump,
        fail: &[&str],
        env: &str,
    ) -> (Result<Option<String>, String>, Vec<String>) {
        let log = std::cell::RefCell::new(Vec::new());
        let step = |kind: &str, argv: &[String]| {
            log.borrow_mut().push(format!("{kind} {}", argv.join(" ")));
            let last = argv.last().map_or("", String::as_str);
            if fail.contains(&last) {
                Err(format!("no {last}"))
            } else if argv.iter().any(|a| a == "show-environment") {
                Ok(env.to_string())
            } else {
                Ok(String::new())
            }
        };
        let got = run_jump_with(j, |a| step("in", a), |a| step("out", a).map(drop));
        (got, log.into_inner())
    }

    fn planned() -> Jump {
        jump(&container_row(), &inside(), &with_container_tmux()).unwrap()
    }

    const QUERY: &str = "in dev tmux show-environment -t %3 CCFOCUS_HOST_PANE";

    #[test]
    fn run_jump_selects_inside_then_switches_to_the_attached_host_pane() {
        let (got, log) = fake_run(&planned(), &[], "CCFOCUS_HOST_PANE=%20\n");
        assert_eq!(got, Ok(None));
        assert_eq!(
            log,
            [
                "in dev tmux select-window -t %3",
                "in dev tmux select-pane -t %3",
                QUERY,
                "out switch-client -t %20",
            ]
        );
    }

    #[test]
    fn run_jump_uses_the_recorded_pane_when_the_query_has_none() {
        for env in [
            "",
            "-CCFOCUS_HOST_PANE\n",
            "CCFOCUS_HOST_PANE=\n",
            "CCFOCUS_HOST_PANE=#(touch x)\n",
            "CCFOCUS_HOST_PANE=-t %1\n",
            "OTHER=%20\n",
        ] {
            let (got, log) = fake_run(&planned(), &[], env);
            assert_eq!(got, Ok(None), "{env:?}");
            assert_eq!(log[3..], ["out switch-client -t %12"], "{env:?}");
        }
        let (_, log) = fake_run(&planned(), &["CCFOCUS_HOST_PANE"], "");
        assert_eq!(log[3..], ["out switch-client -t %12"]);
    }

    #[test]
    fn run_jump_tries_the_queried_pane_once() {
        let (got, log) = fake_run(&planned(), &["%12"], "CCFOCUS_HOST_PANE=%12\n");
        assert_eq!(
            log[3..],
            ["out switch-client -t %12", "out switch-client -t dev"]
        );
        assert_eq!(got, Ok(Some("fell back to dev: no %12".into())));
        let (got, log) = fake_run(&planned(), &["%20"], "CCFOCUS_HOST_PANE=%20\n");
        assert_eq!(
            log[3..],
            ["out switch-client -t %20", "out switch-client -t %12"]
        );
        assert_eq!(got, Ok(Some("fell back to %12: no %20".into())));
    }

    #[test]
    fn run_jump_still_switches_after_a_failed_container_step() {
        let mut j = planned();
        j.container[0].push("broken".into());
        let (got, log) = fake_run(&j, &["broken"], "CCFOCUS_HOST_PANE=%20\n");
        assert_eq!(
            log,
            [
                "in dev tmux select-window -t %3 broken",
                "out switch-client -t %12",
            ],
            "the second container step and the query are skipped"
        );
        let warning = got.unwrap().unwrap();
        assert!(warning.starts_with("in the container: "), "{warning}");
    }

    #[test]
    fn run_jump_falls_back_and_says_so() {
        let (got, log) = fake_run(&planned(), &["%12"], "");
        assert_eq!(
            log[3..],
            ["out switch-client -t %12", "out switch-client -t dev"]
        );
        assert_eq!(got, Ok(Some("fell back to dev: no %12".into())));

        let (got, _) = fake_run(&planned(), &["%12", "dev", "%3"], "");
        let e = got.unwrap_err();
        assert!(e.starts_with("no dev ("), "{e}");
        assert!(e.contains("in the container: no %3"), "{e}");
        assert!(!e.contains("fell back"), "{e}");
    }

    #[test]
    fn run_argv_names_a_missing_program() {
        let e = run_argv(&args(&["ccfocus-test-no-such-program"]), CONTAINER_TIMEOUT).unwrap_err();
        assert_eq!(e, "couldn't run ccfocus-test-no-such-program");
    }

    #[test]
    fn diff_opens_a_popup_over_the_dashboard() {
        let base = "a".repeat(40);
        assert_eq!(
            diff(&row(), &inside(), Some(&base)).unwrap(),
            [
                "display-popup",
                "-EE",
                "-w",
                "90%",
                "-h",
                "90%",
                "-t",
                "%9",
                "-d",
                "/work/repo-a/wt",
                "hunk",
                "diff",
                &base,
                "--watch"
            ]
        );
        assert!(diff(&row(), &inside(), None).is_err());
        let no_pane = Client {
            inside: true,
            pane: None,
        };
        assert!(
            diff(&row(), &no_pane, Some(&base))
                .unwrap_err()
                .contains("which tmux pane")
        );
        let mut r = row();
        r.worktree_path = None;
        assert!(diff(&r, &inside(), Some(&base)).is_err());
    }

    #[test]
    fn values_tmux_expands_are_escaped() {
        let mut r = row();
        r.project = "a #(touch x) #H".into();
        assert_eq!(rename(&r, &inside()).unwrap()[4], "a ##(touch x) ##H");
        r.worktree_path = Some("/w/#(x)".into());
        let plan = diff(&r, &inside(), Some(&"a".repeat(40))).unwrap();
        let d = plan.iter().position(|a| a == "-d").unwrap();
        assert_eq!(plan[d + 1], "/w/##(x)");
    }

    #[test]
    fn rename_uses_the_fitted_project_name() {
        assert_eq!(
            rename(&row(), &inside()).unwrap(),
            ["rename-window", "-t", "%3", "--", "demo"]
        );
        let mut r = row();
        r.project = "x".repeat(60);
        assert_eq!(crate::text::width(&rename(&r, &inside()).unwrap()[4]), 40);
        r.project = "-a todo".into();
        assert_eq!(rename(&r, &inside()).unwrap()[3..], ["--", "-a todo"]);
        r.project.clear();
        assert!(rename(&r, &inside()).is_err());
    }
}
