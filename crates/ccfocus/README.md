# ccfocus

Shows which Claude Code session needs you next.

`ccfocus` has two parts that share one state directory:

- **The hook** (`ccfocus hook <event>`). Claude Code runs it on session events.
  It records each session's status (`idle` / `working` / `waiting` / `done` /
  `failed`), project name, repo, worktree, and tmux pane as one JSON file per
  session. Prompt, message, and tool bodies are never stored.
- **The board** (`ccfocus`, same as `ccfocus tui`). A TUI that reads those
  files and shows the next session to look at, every session, worktrees left to
  finish, and today's numbers. [`cctop`](../cctop)'s Focus panel shows the same
  board.

Register the hook first: the board has nothing to show without it.

## Setup

### Register the hook

Register it for these seven events in your Claude Code settings (for example
`~/.claude/settings.json`). Use the exec form (`command` + `args`) so the
recorded `pid` is Claude's rather than a wrapping shell's, and give the
absolute path to the binary:

```json
{
  "hooks": {
    "SessionStart":     [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "SessionStart"] }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "UserPromptSubmit"] }] }],
    "Notification":     [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "Notification"] }] }],
    "PostToolBatch":    [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "PostToolBatch"] }] }],
    "Stop":             [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "Stop"] }] }],
    "StopFailure":      [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "StopFailure"] }] }],
    "SessionEnd":       [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "SessionEnd"] }] }]
  }
}
```

The hook always exits 0 and prints nothing to stdout, so a failure never
blocks Claude; problems go to stderr as a single line.

### State directory

The state directory is `--state-dir`, else `$CCFOCUS_STATE_DIR`, else
`~/.claude/focus`. Both `ccfocus hook` and `ccfocus tui` take `--state-dir`.

```text
~/.claude/focus/
├── sessions/<session_id>.json   one file per session (SessionEnd removes it)
├── events/<YYYYMMDD>.tsv        one line per recorded event
└── config.toml                  board settings (optional, see Configuration)
```

- The hook appends one line to the day's event log for each event it records,
  including one whose state was superseded by a later overlapping hook.
- `SessionEnd` (which removes the session file) and ignored events (other
  `Notification` types, and `PostToolBatch` unless it ends a wait) add no line.
- Other programs read these files through `cctk::focus`.

## Board

```sh
ccfocus                          # same as `ccfocus tui`
ccfocus tui --state-dir DIR      # read DIR instead of the default
ccfocus tui --tiny               # card view for a narrow side pane
```

### The screen

A synthetic example, 120 columns wide:

```text
focus · main:0                                                              parallel 5 / 4 · next round 14:30 (in 6 min)
┌ next up · waiting 3m ────────────────────────────────────────────────────────────────────┐ ┌ today ──────────────────┐
│[1] repo-a / tmux status line · feat/tmux · dev:2.1                                       │ │tasks done             1 │
└──────────────────────────────────────────────────────────────────────────────────────────┘ │switches              12 │
 # status   repo         project                               worktree       start updated  │yesterday             56 │
 1 waiting  repo-a       tmux status line                      feat-tmux      01/02      3m  │parallel avg/max 2.4 / 5 │
 2 done     repo-b       e2e 安定化                            e2e-388        12/31     12m  │week (switches/day)      │
 3 working  repo-c       token auth                            guard-token    01/03     40s  │▄▇█▃▆█▂                  │
 4 working  repo-a       -                                     main           09:12      2m  └─────────────────────────┘
 - stopped  repo-d       renovate batching                     renovate-2     12/29      6h
   2 sessions done or stopped 2+ days ago









to finish (ahead of base · no PR)
   repo-a       guard-egress   ↑1 ↓5 clean
   repo-c       guard-token    ↑3 ↓0 2 uncommitted [3]
1-9 jump · j/k select · d diff (hunk) · p name window · r reload · q quit
```

From top to bottom:

- **Header**: `parallel N / L` is the number of sessions with an event in the
  last 30 minutes (N) against your limit (L). It turns yellow at the limit and
  red above it; it only warns and never stops anything. `next round` is the
  time of the next check-in, on a grid of `round_minutes` from local midnight.
- **Next up** (`next up`): the session to look at next. The order is waiting
  for approval (`waiting`, longest wait first), then failed (`failed`), then
  finished but not yet visited (`done`), then working (`working`). A finished
  session counts as visited once you jump to it from the board.
- **List**: every session as `#`, status, repo, project, worktree, start time,
  and time since the last update. Rows are in the same order as above, then idle
  (`idle`), then stopped (`stopped`). Rows `1`-`9` can be jumped to with the
  number key; stopped rows have no number.
- **Worktrees to finish** (`to finish`): worktrees that are ahead of
  `origin/HEAD` and whose branch has no open or merged PR of yours. A PR that
  was closed without merging does not hide the branch (the work is not
  finished, so it stays a candidate). Each line shows the commits ahead (`↑`)
  and behind (`↓`), the number of uncommitted entries (`N uncommitted`, or
  `clean`), and the numbers of the sessions using it. At most 4 lines are
  shown; the rest are counted on one line (`… N more`).
- **Today** (`today`, on the right):
  - `tasks done`: your PRs created or merged today.
  - `switches`: prompts sent to a different session than the previous prompt.
  - `yesterday`: yesterday's `switches`.
  - `parallel avg/max`: the mean and maximum number of sessions active in one
    of today's 30-minute windows.
  - `week`: bars of switches per day for the last seven days, ending today.

  Below 110 columns the box becomes a single line above the list
  (`today  done 1 · switches …`).

### Narrow panes (`--tiny`)

`--tiny` draws the board for a narrow side pane (about 20-40 columns, such as
a tmux pane at 10% of the window). The keys are the same. The same example, 24
columns wide:

```text
5 / 4 · 14:30 (6m)
done 1 · sw 12 (y 56)
par 2.4/5 ▄▇█▃▆█▂
┏▸1 waiting━━━━━━━━━━3m┓
┃repo-a · feat-tmux    ┃
┃tmux status line      ┃
┃feat/tmux · dev… 01/02┃
┗━━━━━━━━━━━━━━━━━━━━━━┛
┌2 done─────────────12m┐
│repo-b · e2e-388      │
│e2e 安定化            │
└──────────────────────┘
┌3 working──────────40s┐
│repo-c · guard-token  │
│token auth            │
└──────────────────────┘
↓2 more · +2 old



to finish 2
 repo-a·guard-egr… ↑1 ↓5
 repo-c·guard… ↑3 ±2 [3]
1-9 j/k d p r q
```

- **Header**: `N / L`, the next round with the minutes left, then today's
  numbers in short form (`done` · `sw` switches · `y` yesterday, then `par`
  avg/max and the week's bars).
- **Cards**: one per session with its number, status, time since the last
  event, `repo · worktree`, and project. The cursor's card has a thick border in
  its status colour and one more line for the branch, the tmux window, and the
  start. The next-up card is marked `▸`.
- **When not every card fits**, the cursor's card stays on screen and one line
  counts the cards above (`↑N`) and below (`↓N`) and, after `·`, the sessions
  folded as on the full board (`+N old`).
- **Worktrees to finish** follow at the bottom, the first two only (`+N more`
  for the rest; `· PR?` after the count when PRs are unknown). `↓` is left out
  when not behind; the uncommitted count shows as `±N` and is left out when
  clean (`?` when unknown).
- **In a short pane** the worktrees to finish, then today's numbers, give way so
  the cursor's card stays on screen.

### Session status

- A session on the same side as the board (same host or container, and the same
  hostname) shows `stopped` once its `pid` is gone.
- Otherwise, silence is the only signal: `stopped` appears after 30 minutes
  without an event. This applies to a session on the other side (a container
  session seen from the host, or the reverse), whose `pid` can't be checked,
  and to a same-side session whose liveness can't be determined (no recorded
  `pid`, no `kill` command, Windows, and so on).
- A finished or stopped session untouched for 2 days or more is folded into one
  count line (`N sessions done or stopped 2+ days ago`).

### Keys

| Key | Action |
|---|---|
| `1`-`9` | Switch to that session's tmux pane (`tmux switch-client`). A container session also selects its window and pane inside the container (see [Jumping to a container session](#jumping-to-a-container-session)). |
| `j` / `k` (`↓` / `↑`) | Move the selection |
| `Enter` | Switch to the selected row |
| `d` | Open `hunk diff <merge-base> --watch` in a tmux popup over the board, in the selected row's worktree (`<merge-base>` is `git merge-base HEAD origin/HEAD`). Quitting hunk closes the popup; if hunk fails, the popup stays with its error until `Esc`. The window's layout is left as it was. |
| `p` | Rename the selected row's tmux window to its project name |
| `r` | Re-read now (including `config.toml`) |
| `q` / `Esc` | Close the board |

Outside tmux, the keys that move or rename (`1`-`9`, `Enter`, `d`, `p`) only
print the reason on the bottom line.

### Tools the board calls

- `tmux`: the `1`-`9`, `Enter`, `d`, and `p` keys. For a container session,
  also the command set in the env `CCFOCUS_CONTAINER_TMUX` (run on the host,
  with no shell).
- `hunk`: the `d` key.
- `gh`: `tasks done` and the PR check for worktrees to finish. When `gh` is
  missing or fails, `tasks done` shows `n/a` and worktrees to finish are not
  narrowed by PR.

## Configuration

### `config.toml`

Settings go in `<state dir>/config.toml`:

```toml
limit = 4                   # parallel-session limit (1-99)
round_minutes = 30          # check-in interval; 0 hides it (0-1440)
container_target = "dev"    # fallback tmux target for a container session
```

- Only flat `key = value` lines are read (no tables, no escapes).
- `config.toml` may be a symlink (it is followed), so you can distribute it
  from your dotfiles.
- If the file is broken, the board runs with the defaults and shows the reason
  on the bottom line.

### `CCFOCUS_CONTAINER_TMUX`

The command that runs tmux inside a container is not read from `config.toml`.
Set it in the host's environment variable `CCFOCUS_CONTAINER_TMUX`, in the shell
or tmux environment that starts the board, for example
`export CCFOCUS_CONTAINER_TMUX="dev tmux"`. If it is unset, the board does not
operate the container's tmux.

Why not `config.toml`: this setting decides which program the board runs on the
host. A dev container may mount the state dir (`~/.claude`) writable, so reading
it from `config.toml` would let the container choose a program that runs on the
host. A `container_tmux` line in `config.toml` is rejected as a broken line
(like any other broken line: the whole file counts as defaults, and the bottom
line says to use `$CCFOCUS_CONTAINER_TMUX`).

How the value is read:

- It is split on spaces and tabs into an argv and run without a shell. Other
  whitespace (such as NBSP) does not separate words.
- A value with whitespace other than space and tab, a control character, or more
  than 256 bytes, or a value that is not UTF-8, is ignored, and the reason shows
  on the bottom line (joined into one line with any file warning).

## Jumping to a container session

When the board runs on the host's tmux and a Claude session runs in a dev
container, `1`-`9` or `Enter` on that row does two steps:

1. **Select the pane inside the container.** If `CCFOCUS_CONTAINER_TMUX` is set
   and the session's `tmux_pane` (its pane inside the container, `%N`) is known,
   the board runs `<CCFOCUS_CONTAINER_TMUX> select-window -t <pane>` and
   `<CCFOCUS_CONTAINER_TMUX> select-pane -t <pane>`. This selects the session's
   window and pane in the container's tmux. With
   `CCFOCUS_CONTAINER_TMUX="dev tmux"` and dotfiles' `dev tmux <args>` (which is
   `docker exec -u <user> <container> tmux <args>`), this runs tmux inside the
   container. After that, the board runs `<CCFOCUS_CONTAINER_TMUX>
   show-environment -t <tmux_pane> CCFOCUS_HOST_PANE` to ask the container's
   tmux which host pane shows the container now (see "Why ask" below).
2. **Switch the host's client.** The board runs `tmux switch-client -t` to move
   the host's client to a host pane. It tries these candidates in order: the
   pane just asked in step 1, then `host_tmux_pane` in the session file (the
   value when Claude started), then `container_target`. A candidate is skipped
   if it is missing. The pane asked in step 1 and `host_tmux_pane` must be of the
   form `%N`; `container_target` can be a name such as `dev` or `main:dev` (its
   value in `config.toml`). When a candidate's `switch-client` fails, the board
   moves on to the next one.

When things fail:

- Step 2 runs even when step 1 fails; the error from step 1 shows on the bottom
  line, and the question is skipped when the select step fails.
- Without `CCFOCUS_CONTAINER_TMUX`, step 1 is skipped, including the question
  (the board uses the recorded pane, then `container_target`).
- When a candidate fails and the board moves to a later one, the reason shows on
  the bottom line (`fell back to <target>: <reason>`).

**Why ask.** When you re-enter the container with `dev in`, the container's tmux
rewrites `CCFOCUS_HOST_PANE` in its session environment to the new host pane
(through `update-environment`). A Claude that is already running, and its hook,
keep the value from startup, and a session waiting for input sends no hook
event, so the record alone would jump to the old pane. This needs
`set -ga update-environment CCFOCUS_HOST_PANE` in the container's `tmux.conf`
(dotfiles sets it). Without it, or if the question fails, the board uses the
recorded pane. The answer comes from the container too, so its form is checked
as `%N` before use.

**Recording `host_tmux_pane`.** The side that starts the container passes the
host's pane through the `CCFOCUS_HOST_PANE` environment variable, for example
`docker exec -e CCFOCUS_HOST_PANE="$TMUX_PANE" …`. The hook inside the container
reads it and stores it as `host_tmux_pane`; it records only a value of the form
`%N`, and the hook on the host does not read it.

**What the board trusts.** The session file is written by the container, so the
board does not trust it: a pane id is checked to be of the form `%N` before it
is used as a tmux target, and the command to run comes only from the host's env
`CCFOCUS_CONTAINER_TMUX`. Any value of the form `%N` can point at any pane on
the host, but all that happens is a move; no command is run. A desktop app
session (outside tmux) still cannot be jumped to.

Limits:

- If several host panes are attached to the same container at once, the jump
  goes to the pane that attached last.
- After you detach from the container (leave `dev in`), that host pane is back
  at the host's shell. The switch succeeds, but the container is not shown.
- `CCFOCUS_CONTAINER_TMUX` takes one value, so the jump assumes one container.
  If the sessions of several containers share the same state dir, the jump can
  select a pane with the same number in another container.

## Behavior details

### External commands, timeouts, and process groups

On unix, the board starts each external command it runs while it is up (`git`,
`gh`, `tmux`, `kill`, and the command in `CCFOCUS_CONTAINER_TMUX`) in its own
new process group. The one `hostname` run at startup is the exception.

| Command | Timeout |
|---|---|
| `git` | 3 s |
| `gh` | 20 s |
| `tmux` on the host | 2 s |
| `tmux` in the container | 3 s |
| `kill` | 1 s |

- When a command times out, the board stops every process in that group with
  SIGKILL, whether or not the process closed its pipes.
- It also stops the group when someone still holds the output pipe open 0.5
  seconds after the command has exited.
- Closing the board (`q` / `Esc`, or quitting cctop) also stops the group of
  every command still running with SIGKILL. It first waits for the `tmux`
  command of a jump (`1`-`9`, `Enter`) to finish, so a jump in progress is
  carried out to the end even if you close the board. A command that was about
  to start after the close is stopped right after it starts.
- Stopping the group is best effort: when the `kill` command is missing or
  fails, the group is not stopped, and on a timeout only the command itself is
  stopped.
- A process that closed its stdout and stderr (`>/dev/null 2>&1`) survives only
  when the command finishes within its timeout. To keep a process alive even
  after a timeout, start it outside the group (for example with `setsid`).
- Only processes on the host are stopped: a process that
  `CCFOCUS_CONTAINER_TMUX` starts inside a container (for example with
  `docker exec`) is out of reach, though the docker client on the host is
  stopped.
- The tmux popup (the `d` key) and Windows are not covered.

### Refresh

The board never runs `git fetch`; it reads local refs only. Session files and
events are re-read every 2 seconds, worktrees every 30 seconds, and `gh` results
are cached for 10 minutes (`r` does not refresh `gh`).

### Housekeeping

The board (`ccfocus` and cctop's Focus panel) runs a cleanup at start-up and
every hour. It deletes event logs (`events/*.tsv`) older than 90 days and hook
temp files (`.<id>.json.<pid>.<nanos>.tmp`) older than 10 minutes.

## Privacy

- The hook never stores prompt, message, or tool bodies.
- The hook reads only two things from the session's transcript: the last
  custom title (`custom-title` line) for the project name, when the hook input
  has no `session_title`; and the first `timestamp`, to recover the start time
  of a resumed session.
- The board never reads prompts or transcripts.
- What leaves your machine: `gh` asks GitHub only for a search of your own
  (`@me`) PRs and, in each session's repo, `gh pr list --author @me`.
- The board does not trust session files that a container wrote, and the
  command it runs for a container comes only from the host's env; see "What the
  board trusts" under [Jumping to a container session](#jumping-to-a-container-session).
