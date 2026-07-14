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
