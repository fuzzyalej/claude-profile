# Install/Remove Progress Spinner

## Problem

`provision()` and `gc.rs` shell out to `claude plugin marketplace add`, `claude
plugin install`, `claude plugin uninstall`, and `claude plugin marketplace
remove` via `Command::output()`, which blocks silently until the subprocess
exits. These can involve network fetches (marketplace clone, plugin download)
that take several seconds, during which the CLI shows no feedback — unlike
the animated spinners `npm install` or `brew install` show while working.

## Goals

- Show a live "working" indicator while each install/remove subprocess call
  runs, labeled with what's happening (e.g. `Installing plugin foo@bar...`).
- Match the look of familiar package-manager CLIs (animated spinner in a
  real terminal).
- Degrade gracefully when stdout isn't a terminal (CI, piping to a file):
  no raw ANSI/carriage-return sequences in logs.
- No behavior change to the underlying install/remove logic, error
  propagation, or existing tests.

## Non-goals

- No progress *bar* / percentage (the underlying `claude` subprocess gives us
  no progress signal to drive one — only start/end).
- `list_plugins` / `list_marketplaces` calls are not wrapped — they're fast
  JSON reads with no perceptible wait.
- The `confirm()` y/N prompt itself is unchanged.

## Design

### Dependency

Add `indicatif = "0.17"` to `Cargo.toml`. It depends on `console`, which we
use directly for the tty check.

### `src/spinner.rs`

```rust
pub fn spin<T>(msg: &str, done: &str, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T>
```

- **TTY path** (`console::Term::stdout().is_term()` is true): create an
  `indicatif::ProgressBar` spinner, set `msg` as its live label, enable a
  steady tick (~80ms) so it animates while `f()` runs synchronously. On
  `Ok`, clear the spinner line and print `✔ {done}`. On `Err`, clear the
  spinner line and propagate the error untouched — no extra formatting, so
  `main.rs`'s existing error printing is unaffected.
- **Non-TTY path**: print `{msg}` (with trailing newline, no carriage
  return), run `f()`, and on success print `{done}`. On error, print
  nothing extra; the error propagates normally.

This function is a thin wrapper: it does not change what `f()` returns or
which errors it can produce, only what's printed around the call.

### Call sites

`provision.rs::provision`, wrapping each existing call in the plan-apply
loops:
- `cli.marketplace_add(src)` → `"Adding marketplace {name}..."` /
  `"✔ Added marketplace {name}"`
- `cli.install_plugin(id)` → `"Installing plugin {id}..."` /
  `"✔ Installed plugin {id}"`

`commands/gc.rs`, wrapping each existing call in the cleanup loops:
- `cli.uninstall_plugin(id)` → `"Removing plugin {id}..."` /
  `"✔ Removed plugin {id}"`
- `cli.marketplace_remove(name)` → `"Removing marketplace {name}..."` /
  `"✔ Removed marketplace {name}"`

### Testing

The TTY animation path isn't meaningfully unit-testable (no real terminal in
CI). Tests cover the `spin()` helper's non-TTY/plain code path directly:
that it runs `f()` exactly once, returns `f()`'s `Ok` value unchanged, and
propagates `f()`'s `Err` unchanged. Existing `provision`/`gc` mock-based
tests are unaffected since they assert on the mock CLI's recorded calls, not
stdout content.
