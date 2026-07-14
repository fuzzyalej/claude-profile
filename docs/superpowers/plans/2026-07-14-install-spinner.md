# Install/Remove Progress Spinner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show a live "working" indicator (animated spinner in a terminal, plain text otherwise) while `claude-profile` shells out to slow `claude plugin ...` subcommands during provisioning and garbage collection.

**Architecture:** A single reusable helper, `spinner::spin`, wraps a closure that performs the blocking subprocess call. It detects whether stdout is a real terminal and either drives an `indicatif` spinner or falls back to plain `println!` lines. `provision.rs` and `commands/gc.rs` call it around each `ClaudeCli` mutation call (`marketplace_add`, `install_plugin`, `uninstall_plugin`, `marketplace_remove`).

**Tech Stack:** Rust, `indicatif` 0.17 (new dependency, pulls in `console`), existing `anyhow` error handling.

## Global Constraints

- No behavior change to install/remove logic, error propagation, or the y/N `confirm()` prompt.
- `list_plugins`/`list_marketplaces` calls are NOT wrapped.
- Non-terminal (piped/CI) output must contain no raw ANSI/carriage-return sequences.
- Exact message strings (from spec):
  - `"Adding marketplace {name}..."` / `"✔ Added marketplace {name}"`
  - `"Installing plugin {id}..."` / `"✔ Installed plugin {id}"`
  - `"Removing plugin {id}..."` / `"✔ Removed plugin {id}"`
  - `"Removing marketplace {name}..."` / `"✔ Removed marketplace {name}"`
- Existing `provision`/`gc` mock-based tests must keep passing unmodified (they assert on mock call recordings, not stdout).

---

### Task 1: Add `indicatif` dependency and `spinner::spin` helper

**Files:**
- Modify: `Cargo.toml`
- Create: `src/spinner.rs`
- Modify: `src/main.rs:5-19` (add `mod spinner;` in the alphabetically-sorted `mod` block)

**Interfaces:**
- Produces: `pub fn spin<T>(msg: &str, done: &str, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T>` in `crate::spinner`. `provision.rs` and `commands/gc.rs` (Task 2) call this directly.

- [ ] **Step 1: Add the dependencies**

Edit `Cargo.toml`, in the `[dependencies]` block, add:

```toml
indicatif = "0.17"
console = "0.15"
```

`console` is a transitive dependency of `indicatif` (it provides `indicatif`'s tty detection internally), but Rust doesn't let a crate call transitive dependencies by name — `spinner.rs` needs to call `console::Term::stdout().is_term()` directly, so it must also be declared as a direct dependency here.

Run: `cargo build` (just to fetch/verify the dependencies resolve)
Expected: builds successfully (no other code changes yet, so no new warnings from unused code — `spinner.rs` doesn't exist yet).

- [ ] **Step 2: Write `src/spinner.rs`**

```rust
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub fn spin<T>(msg: &str, done: &str, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    if console::Term::stdout().is_term() {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(Duration::from_millis(80));
        let result = f();
        pb.finish_and_clear();
        match result {
            Ok(v) => {
                println!("{done}");
                Ok(v)
            }
            Err(e) => Err(e),
        }
    } else {
        println!("{msg}");
        let result = f()?;
        println!("{done}");
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn runs_closure_once_and_returns_ok_value() {
        let calls = Cell::new(0);
        let result = spin("working...", "done", || {
            calls.set(calls.get() + 1);
            Ok(42)
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn propagates_closure_error_unchanged() {
        let result: anyhow::Result<()> =
            spin("working...", "done", || anyhow::bail!("boom"));
        assert_eq!(result.unwrap_err().to_string(), "boom");
    }
}
```

- [ ] **Step 3: Register the module**

In `src/main.rs`, in the `mod` block (currently lines 5-19, alphabetically sorted), add a line so it reads:

```rust
mod refmap;
mod resolve;
mod spinner;
```

(insert `mod spinner;` after `mod resolve;`, keeping alphabetical order — check the surrounding lines in the file since more modules may exist below `resolve` already).

- [ ] **Step 4: Run the new unit tests**

Run: `cargo test spinner::`
Expected: `running 2 tests ... test result: ok. 2 passed`

Both tests exercise the non-terminal branch (test runs have no tty attached to stdout, so `console::Term::stdout().is_term()` is `false`), which is exactly the fallback path the spec requires to be plain and side-effect-predictable.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/spinner.rs src/main.rs
git commit -m "feat: add spinner helper for slow CLI subprocess calls"
```

---

### Task 2: Wrap install/remove call sites in `provision.rs` and `gc.rs`

**Files:**
- Modify: `src/provision.rs:63-68` (the `provision` function's apply loops)
- Modify: `src/commands/gc.rs:16-23` (the `run` function's removal loops)

**Interfaces:**
- Consumes: `crate::spinner::spin<T>(msg: &str, done: &str, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T>` from Task 1.

- [ ] **Step 1: Update `provision.rs`'s apply loops**

In `src/provision.rs`, add `use crate::spinner::spin;` near the top (with the other `use` statements at lines 1-6), then replace the loop bodies at lines 63-68:

```rust
    for (name, src) in &plan.marketplaces {
        spin(
            &format!("Adding marketplace {name}..."),
            &format!("✔ Added marketplace {name}"),
            || cli.marketplace_add(src),
        )?;
    }
    for id in &plan.plugins {
        spin(
            &format!("Installing plugin {id}..."),
            &format!("✔ Installed plugin {id}"),
            || cli.install_plugin(id),
        )?;
    }
```

Note the loop over `plan.marketplaces` previously destructured `(_name, src)` (name unused) — it now needs `name` for the message, so change `(_name, src)` to `(name, src)`.

- [ ] **Step 2: Run existing provision tests to confirm no regressions**

Run: `cargo test provision::`
Expected: all existing tests in `src/provision.rs`'s `tests` module pass unchanged (they assert on `MockCli`'s recorded calls, e.g. `cli.added.borrow()`, not stdout — the spinner's printed output doesn't affect assertions, and tests run without a tty so `spin` takes the plain-text branch and calls `f()` exactly once, same as before).

- [ ] **Step 3: Update `gc.rs`'s removal loops**

In `src/commands/gc.rs`, add `use crate::spinner::spin;` near the top (with the other `use` statements at lines 1-3), then replace the loop bodies at lines 17-22:

```rust
        for id in &removed_plugins {
            spin(
                &format!("Removing plugin {id}..."),
                &format!("✔ Removed plugin {id}"),
                || cli.uninstall_plugin(id),
            )?;
        }
        for name in &removed_marketplaces {
            spin(
                &format!("Removing marketplace {name}..."),
                &format!("✔ Removed marketplace {name}"),
                || cli.marketplace_remove(name),
            )?;
        }
```

- [ ] **Step 4: Run existing gc tests to confirm no regressions**

Run: `cargo test gc::`
Expected: `real_run_uninstalls_and_removes` and `dry_run_reports_without_removing` both pass unchanged (same reasoning as Step 2 — `dry_run_reports_without_removing` never enters the removal loops at all since `dry_run` is `true`).

- [ ] **Step 5: Run the full test suite**

Run: `cargo test`
Expected: all tests pass, no failures.

- [ ] **Step 6: Manual smoke test**

Run: `cargo run -- --help` to confirm the binary still builds and runs (a full provisioning flow requires a real `claude` binary and profile, which isn't available in this environment — the automated tests above are the primary verification for the call-site wiring).

- [ ] **Step 7: Commit**

```bash
git add src/provision.rs src/commands/gc.rs
git commit -m "feat: show progress spinner during plugin/marketplace install and removal"
```
