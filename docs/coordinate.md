# Coordinate many sessions

## Overview

`claude-profile coordinate` launches a coordinator session that delegates tasks to worker
sessions. Each worker is a separate Claude Code session with its own profile and context. By
default, each worker runs in its own git worktree on its own branch, so workers don't overwrite
each other's files.

You talk only to the coordinator. It plans the split, starts workers, tracks them, and merges
their branches.

The coordinator gets seven MCP tools from the `claude-profile-workers` server, and a bundled
plugin with a `coordinating-workers` skill that teaches it how to use them. See
[How it works](how-it-works.md#coordinator-and-workers).

## Quick start

1. Install at least one profile. Run `claude-profile list` to check.
2. From inside a git repo, start the coordinator with the profile it should use:

   ```sh
   claude-profile coordinate rust-developer -- --permission-mode acceptEdits
   ```

   Headless workers can't ask you to approve edits, so give them a permission mode that allows
   them.

3. Ask for work that splits into independent parts:

   > Use two workers. One adds input validation to `src/parse.rs`. The other adds tests for
   > `src/render.rs`. Merge both when they finish.

4. The coordinator lists profiles, spawns the workers, polls their status, reads their results,
   merges the branches, and cleans up.

Arguments after `--` go to the coordinator's `claude`. Of those, only the permission flags also
reach workers. See [Permissions](#permissions).

If workers run headless and you pass no permission flag, `coordinate` prints a warning.

For all options, see [`coordinate`](commands.md#coordinate).

## How workers run

Workers start in one of two modes.

| Mode | When | Behavior |
|---|---|---|
| Herdr | `HERDR_ENV=1` and `HERDR_PANE_ID` are set (the coordinator runs inside a Herdr pane) | Each worker opens in its own background tab in the coordinator's workspace. The tab doesn't take focus. |
| Headless | Everything else, or `--headless` | Each worker runs in the background. You don't see it. |

`--headless` forces headless mode, even inside Herdr.

Before a worker starts, the server provisions its profiles, one worker at a time. If that
fails, `spawn` returns the error and starts nothing.

Each worker launches as `claude-profile <profiles> --yes -- <session args> --add-dir <run_dir>
<permission flags>`. The session args are set by the server, for example the session ID. Of
the arguments you pass after `--`, only the permission flags reach workers. The `--add-dir`
argument lets the worker write its result file to the run directory. A headless worker gets
its task on stdin.

When the worker finishes, it writes a `result.json` with a status, a summary, and notes. If it
writes none, the coordinator reads the worker's last output instead. A worker in a worktree
is told to commit its changes on its branch before it finishes.

In Herdr mode, you can step into a worker's tab to watch it or answer a prompt. If you close a
worker's tab, the worker is marked `failed`. A later `send` to that worker falls back to a
headless resume of its session. If a worker's tab fails to start, the server closes it
and reports its last output in the error.

In headless mode, `cancel` stops the worker's `claude` process.

### Worker statuses

| Status | Meaning |
|---|---|
| `queued` | Waiting for a free slot. Starts on its own. |
| `starting` | Launching. |
| `working` | Running the task. |
| `blocked` | Waiting for you in the worker's tab (Herdr only). |
| `done` | Finished. Read the result. |
| `failed` | Finished with an error, or its tab or process is gone. |
| `cancelled` | Stopped by the coordinator. |

## The clean profile

`clean` is a bundled profile with no plugins, skills, or MCP servers: plain Claude Code. When no
other profile fits a task, the coordinator spawns the worker with `["clean"]` instead of
stretching an unrelated profile. It is also the right choice for generic work such as docs,
scripts, and quick research.

## Coordinator behavior

`coordinate` adds a standing instruction to the coordinator's system prompt. Workers and plain
`claude-profile` launches don't get it. The instruction tells the coordinator to:

- Delegate first. Implementation, research, reviews, and other multi-step work go to workers.
  The coordinator works itself only on answers, plans, merging, and trivial edits, and says why
  when it doesn't delegate.
- Delegate tasks that no profile fits to a worker with the `clean` profile, instead of doing
  them itself.
- Load the `coordinating-workers` skill before it starts any task.
- Keep the work invisible. It reports results and decisions, not worker status chatter.
- Clean up. After it merges a worker's branch, it calls `cleanup`, which closes the worker's
  tab and removes its worktree. It also deletes temporary files it created, closes tabs, panes,
  and browser tabs it opened, and checks that nothing is left behind before it reports a task
  done.

If you pass `--append-system-prompt` after `--`, the coordinator keeps your text. `coordinate`
joins your text and its own into one flag, separated by a blank line.

## Worktrees and merging

With `worktree=true` (the default), `spawn` creates a worktree and a branch for the worker:

- Worktree path: `~/.claude-profiles/worktrees/<repo-name>-<hash>/<worker-id>`
- Branch: `cp/<worker-id>`
- Start point: `HEAD`, or the `base_ref` argument when given.

The worktree doesn't include uncommitted changes from your working directory. Commit first if a
worker needs them.

When a worker is `done`, the coordinator reads its result, which includes the branch's `base`
commit. It commits any changes the worker left uncommitted, inspects `git diff
<base>...cp/<worker-id>`, merges `cp/<worker-id>` into your working branch, runs the tests, and
calls `cleanup`.

`cleanup` closes the worker's tab and deletes its worktree and branch. It keeps the worktree
when the worktree has uncommitted changes.

Use `worktree=false` to run a worker in your current directory. This is required outside a git
repo.

## Permissions

Workers inherit only the permission flags you pass after `--` when you launch the
coordinator. Other arguments, such as `--model`, reach only the coordinator.

- `--permission-mode`
- `--dangerously-skip-permissions`
- `--allowedTools` or `--allowed-tools`, with every value up to the next flag
- `--disallowedTools` or `--disallowed-tools`, with every value up to the next flag

Changing the mode during the session, for example with Shift+Tab, doesn't reach workers. They
keep the flags from launch.

Under a restrictive mode, headless workers can't run commands the mode blocks, because nobody
can approve them. For example, a headless worker under `acceptEdits` can't run `git`. In Herdr
mode, the worker becomes `blocked` and you approve the prompt in its tab.

To let headless workers run a specific command, allow it at launch:

```sh
claude-profile coordinate rust-developer -- --permission-mode acceptEdits --allowedTools "Bash(git:*)"
```

Your global `~/.claude` hooks, `CLAUDE.md`, and memory still load in workers. This is the same
limitation as in any profiled session. See the
[README](../README.md#important-limitation-your-global-claudemd-and-memory-are-not-gated).

## Limits

- **Concurrency.** At most 4 workers run at once. Change it with `--max-workers`. Extra spawns
  queue and start when a slot frees up.
- **No nesting.** Workers can't spawn workers.
- **Self-contained tasks.** A worker can't see the coordinator's conversation. The task must
  include every path, requirement, and constraint.

## Cleaning up

Each run keeps its state under `~/.claude-profiles/runs/<run-id>/`. List runs:

```sh
claude-profile runs
```

Each line shows the run ID, its working directory, and worker counts by status. A run whose
coordinator is still open ends with ` (active)`.

Remove the worktrees and branches of runs that are no longer active, and then the runs:

```sh
claude-profile runs --clean            # all runs
claude-profile runs --clean <run-id>   # one run
```

`runs --clean` keeps anything you could lose, and tells you what it kept:

```
skipped (in use): <run-id>
kept (uncommitted changes): <worktree path>
kept (unmerged branch): cp/<worker-id>
```

- It skips a run whose coordinator is still open.
- In a run that is no longer active, it treats every worker as finished, because their
  processes are gone.
- It keeps a worktree that has uncommitted changes.
- It keeps a branch that has commits not merged into the repo's current `HEAD`.
- It keeps a run when anything in it was kept.

## Troubleshooting

**A worker is `blocked`.** The worker is waiting for a permission prompt. Open its tab in
Herdr and approve or deny it. Headless workers can't be blocked. They fail the blocked command
instead. Relaunch with the needed `--allowedTools`, or use a less restrictive mode.

**A worker is `failed`.** Ask the coordinator to read the result. It contains the worker's
summary, or its last output if it wrote no result file. Retry with a clearer task, or fix the
cause and `send` the worker a follow-up.

**A tab was closed.** The worker is marked `failed`. If you `send` to it, the message falls
back to a headless resume of the same session. The worktree and branch are kept.

**A spawn stays `queued`.** The run is at its concurrency limit. It starts when a worker
finishes. Raise the limit with `--max-workers`.

**`spawn` fails outside a git repo.** Worktrees need a git repo. Start the coordinator inside
one, or spawn with `worktree=false`.

## Manual smoke test

Contributors: run these checks before a release. They need a real `claude` login.

### Headless

1. In a scratch git repo with two independent files, run:

   ```sh
   claude-profile coordinate <profile> --headless
   ```

2. Ask for two workers, one per file.
3. Wait until both report `done`.
4. Ask the coordinator to merge both branches and call `cleanup` on both workers.
5. Exit, then run:

   ```sh
   claude-profile runs --clean
   ```

   Check that no worktrees or `cp/*` branches remain.

### Herdr

1. Repeat the headless test inside a Herdr pane, without `--headless`.
2. Check that each worker opens in its own tab, and that focus stays on the
   coordinator.
3. Trigger a permission prompt in a worker tab, for example with `--permission-mode
   acceptEdits` and a task that runs `git`. Approve it in the tab. Check that the worker
   continues.
4. Close one worker's tab. Check that the worker becomes `failed`, and that `send` to it falls
   back to a headless resume.
