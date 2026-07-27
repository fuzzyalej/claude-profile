//! Ambient progress reporting. `main::run` installs a reporter for the whole
//! process; everything below it reports through the free functions here, which
//! no-op when no reporter is installed (so unit tests stay silent and unchanged).

use indicatif::{ProgressBar, ProgressStyle};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

pub trait Reporter {
    /// Work starting. On a TTY this replaces the spinner's message.
    fn step(&self, msg: &str);
    /// A finished unit of work, printed as a line above the spinner.
    fn done(&self, msg: &str);
    /// Run `f` with the spinner paused, so it can't draw over a stdin prompt.
    fn suspend(&self, f: &mut dyn FnMut());
    /// Stop and erase any live rendering. Must be idempotent.
    fn finish(&self);
}

thread_local! {
    /// The reporter in effect on this thread. Thread-local rather than global so
    /// parallel `cargo test` threads can each install their own recorder.
    static ACTIVE: RefCell<Option<Rc<dyn Reporter>>> = const { RefCell::new(None) };
}

/// Swap the active reporter, returning the previous one.
fn set_active(next: Option<Rc<dyn Reporter>>) -> Option<Rc<dyn Reporter>> {
    ACTIVE.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), next))
}

/// Clone the active reporter out of the slot. Cloning (rather than calling
/// through the borrow) keeps a reporter free to re-enter these functions.
fn active() -> Option<Rc<dyn Reporter>> {
    ACTIVE.with(|slot| slot.borrow().clone())
}

/// Uninstalls the reporter it was created with, restoring whatever was active
/// before, and finishes the outgoing reporter so no half-drawn spinner line is
/// left in front of a later `error:` message.
pub struct Guard {
    previous: Option<Rc<dyn Reporter>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(outgoing) = set_active(self.previous.take()) {
            outgoing.finish();
        }
    }
}

pub fn install_reporter(reporter: Rc<dyn Reporter>) -> Guard {
    Guard { previous: set_active(Some(reporter)) }
}

/// Install the reporter appropriate for this process: a spinner when stderr is a
/// terminal, plain completion lines otherwise.
pub fn install() -> Guard {
    if console::Term::stderr().is_term() {
        install_reporter(Rc::new(SpinnerReporter::new()))
    } else {
        install_reporter(Rc::new(PlainReporter))
    }
}

pub fn step(msg: &str) {
    if let Some(r) = active() {
        r.step(msg);
    }
}

pub fn done(msg: &str) {
    if let Some(r) = active() {
        r.done(msg);
    }
}

/// Stop rendering. Call before printing a command's result to stdout, and before
/// handing the terminal to another process (`launch::spawn`).
pub fn clear() {
    if let Some(r) = active() {
        r.finish();
    }
}

pub fn suspend<T>(f: impl FnOnce() -> T) -> T {
    match active() {
        None => f(),
        Some(r) => {
            let mut f = Some(f);
            let mut out = None;
            r.suspend(&mut || out = Some((f.take().expect("suspend ran twice"))()));
            out.expect("reporter did not run the suspended closure")
        }
    }
}

struct SpinnerReporter {
    pb: ProgressBar,
}

impl SpinnerReporter {
    fn new() -> Self {
        // ProgressBar::new_spinner draws to stderr by default, which is what we want.
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        pb.enable_steady_tick(Duration::from_millis(80));
        Self { pb }
    }
}

impl Reporter for SpinnerReporter {
    fn step(&self, msg: &str) {
        self.pb.set_message(msg.to_string());
    }
    fn done(&self, msg: &str) {
        // println on the bar prints above the live spinner instead of over it.
        self.pb.println(format!("✓ {msg}"));
    }
    fn suspend(&self, f: &mut dyn FnMut()) {
        self.pb.suspend(f);
    }
    fn finish(&self) {
        self.pb.finish_and_clear();
    }
}

/// Non-TTY reporter: no spinner frames, no ANSI, completion lines only, so piped
/// and CI output stays stable.
struct PlainReporter;

impl Reporter for PlainReporter {
    fn step(&self, _msg: &str) {}
    fn done(&self, msg: &str) {
        eprintln!("{msg}");
    }
    fn suspend(&self, f: &mut dyn FnMut()) {
        f();
    }
    fn finish(&self) {}
}

/// Test reporter that records the calls a code path makes. Used by the unit
/// tests in `pack`, `provision`, and `index::fetch` as well as this module's.
#[cfg(test)]
#[derive(Default)]
pub struct RecordingReporter {
    events: RefCell<Vec<String>>,
}

#[cfg(test)]
impl RecordingReporter {
    pub fn events(&self) -> Vec<String> {
        self.events.borrow().clone()
    }
}

#[cfg(test)]
impl Reporter for RecordingReporter {
    fn step(&self, msg: &str) {
        self.events.borrow_mut().push(format!("step: {msg}"));
    }
    fn done(&self, msg: &str) {
        self.events.borrow_mut().push(format!("done: {msg}"));
    }
    fn suspend(&self, f: &mut dyn FnMut()) {
        f();
    }
    fn finish(&self) {}
}

/// Install a `RecordingReporter` for the rest of the current scope. Hold the
/// guard (`let (rec, _g) = record();`) — dropping it uninstalls the recorder.
#[cfg(test)]
pub fn record() -> (Rc<RecordingReporter>, Guard) {
    let rec = Rc::new(RecordingReporter::default());
    let guard = install_reporter(rec.clone());
    (rec, guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_are_noops_when_nothing_installed() {
        step("working");
        done("worked");
        clear();
        assert_eq!(suspend(|| 7), 7);
    }

    #[test]
    fn recorder_captures_step_and_done_in_order() {
        let (rec, _g) = record();
        step("pulling pack a");
        done("updated pack a");
        assert_eq!(
            rec.events().as_slice(),
            &["step: pulling pack a", "done: updated pack a"]
        );
    }

    #[test]
    fn dropping_guard_restores_the_noop_slot() {
        let rec = {
            let (rec, _g) = record();
            step("inside");
            rec
        };
        step("outside");
        assert_eq!(rec.events().as_slice(), &["step: inside"]);
    }

    #[test]
    fn nested_install_replaces_then_restores_outer() {
        let (outer, _g) = record();
        {
            let (inner, _g2) = record();
            step("inner");
            assert_eq!(inner.events().as_slice(), &["step: inner"]);
        }
        step("outer");
        assert_eq!(outer.events().as_slice(), &["step: outer"]);
    }

    #[test]
    fn suspend_runs_closure_once_and_returns_its_value() {
        let (_rec, _g) = record();
        let mut calls = 0;
        let out = suspend(|| {
            calls += 1;
            "yes"
        });
        assert_eq!(out, "yes");
        assert_eq!(calls, 1);
    }
}
