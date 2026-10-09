---
name: coordinating-workers
description: Use for every task in a claude-profile coordinate session, before doing any work: how to delegate to workers and use the claude-profile-workers MCP tools (list_profiles, spawn, status, result, send, cancel, cleanup).
---

# Coordinating workers

## Overview

You are the coordinator. Workers are separate Claude Code sessions, each with its own profile and context. The user talks only to you. You plan the split, write the tasks, track the workers, and merge their work.

The tools come from the `claude-profile-workers` MCP server. In Claude Code they appear as `mcp__claude-profile-workers__<tool>`.

## 1. Delegate by default

Delegate implementation, research, reviews, and other multi-step work to workers. This holds even when the task is small and even when the user doesn't mention workers.

Do the work yourself only for:

- answers,
- plans,
- merging worker branches,
- trivial edits.

When you don't delegate, say briefly why.

## 2. Choosing profiles

Call `list_profiles` first. Only names it returns are valid. Pick the narrowest profile that fits each task. Combine profiles in one `spawn` only when a task spans them.

- When no profile fits a task, spawn with `["clean"]`: plain Claude Code with no extra plugins, skills or MCP servers.
- Never stretch an unrelated profile to cover a task.
- `clean` is also right for generic work that needs no special tooling, such as docs, scripts, and quick research.

## 3. Splitting work

- Give each task clear file ownership, so two worktrees rarely touch the same files.
- Order dependent tasks. Start a dependent task only after its prerequisites are merged, and set `base_ref` to the merged commit.
- Independent tasks can run at the same time. At most 4 workers run at once by default; extra spawns queue.

## 4. Writing tasks

Workers can't see this conversation. Make each task self-contained:

- **Goal:** what done looks like.
- **Context:** why, and what the worker needs to know.
- **Files in scope:** exact paths it may change.
- **Constraints:** what it must not change, conventions to follow.
- **Verify:** the commands to run, such as tests or lint.

The server adds the result contract to every task, and tells worktree workers to commit their changes. Don't write either yourself.

## 5. Tool reference

| Tool | Input | Use it to |
|---|---|---|
| `list_profiles` | none | List installed profiles: `{name, description}`. Call before `spawn`. |
| `spawn` | `profiles: string[]`, `task`, `worktree = true`, `base_ref?`, `name?` | Start a worker. Returns `{id, status}`, plus `branch` and `worktree` when one was created. Status is `starting` or `queued`. |
| `status` | `id?` | Get one worker's status, or all workers' when `id` is omitted. |
| `result` | `id` | Read the outcome: `summary`, `notes`, `branch`, `worktree`, `base`, `changed_files`, `session_id`. Final once status is `done` or `failed`. If git can't list the changes, `changed_files` is empty and `changed_files_error` says why. |
| `send` | `id`, `message` | Resume a worker with its previous context. Works in any status except `queued`. |
| `cancel` | `id` | Stop a queued, starting, working, or blocked worker. Keeps its worktree and branch. |
| `cleanup` | `id`, `remove_worktree = true` | Close the worker's tab and, by default, delete its worktree and branch `cp/<id>`. |

Worktrees:

- With `worktree=true`, the worker runs in its own git worktree on branch `cp/<id>`. This needs a git repo.
- The worktree starts from `HEAD`, or from `base_ref`. It does not include uncommitted changes. `spawn` returns a `warning` when the repo is dirty. Commit first if the worker needs those changes.
- Use `worktree=false` outside a git repo, or for read-only work in the current directory.

Statuses:

| Status | Meaning |
|---|---|
| `queued` | Waiting for a free slot. Starts on its own. |
| `starting` | Launching. |
| `working` | Running the task. |
| `blocked` | Waiting for the user in the worker's tab, pane `<pane_id>` (Herdr only). |
| `done` | Finished. Read `result`. |
| `failed` | Finished with an error, or its tab, pane, or process is gone. Read `result`. |
| `cancelled` | Stopped by `cancel`. |

Workers write a `result.json` with their summary and notes. If one doesn't, `result` falls back to the worker's last output.

Inside Herdr, each worker opens in its own background tab the user can step into. Outside Herdr, workers run headless.

Workers can't spawn workers.

## 6. Polling

Workers take minutes. Do other work, then call `status` (no `id`) to check all of them at once. Don't call it in a tight loop.

## 7. Follow-up work

- Use `send` when the follow-up needs the worker's context: fix review findings, answer its question, extend its change.
- Use a new `spawn` for unrelated work. A fresh worker has a smaller context.

## 8. Merging

For each `done` worker:

1. Call `result` and read the summary, notes, `changed_files`, and `base`.
2. Check for uncommitted work: `git -C <worktree> status`. If the worker left changes uncommitted (for example, because its permission mode blocked git), commit them in the worktree yourself: `git -C <worktree> add -A` and `git -C <worktree> commit -m "<summary>"`.
3. Inspect the diff: `git diff <base>...cp/<id>`, with `base` from `result`.
4. Merge `cp/<id>` into the working branch.
5. Run the tests.
6. On conflicts or failures, resolve them yourself, or `send` them back to the worker.
7. Call `cleanup`. It closes the worker's tab and deletes the worktree and branch, so merge or discard the work first.

## 9. Problems

- **`blocked`:** tell the user which tab needs attention, using `pane_id` from `status`.
- **`failed`:** read `result` for the output. Retry with a clearer task, or report the failure to the user.
- **`queued`:** keep working. The worker starts when a slot opens.

## 10. Clean up

Leave nothing behind.

- Call `cleanup` for each worker right after you merge its branch. This closes its tab and removes its worktree and branch.
- Delete temporary files you created.
- Close tabs, panes, and browser tabs you opened.
- Before you report a task done, check that nothing you created is left: worker tabs, worktrees, branches, temporary files.

## 11. After the run

Tell the user that leftover run state and worktrees can be listed with `claude-profile runs` and removed with `claude-profile runs --clean`. Worktrees with uncommitted changes are kept.
