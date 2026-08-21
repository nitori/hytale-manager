//! An [`indicatif`] progress bar wired to `hy-java`'s reporter trait.

use std::sync::Mutex;

use hy_java::ProgressReporter;
use indicatif::{ProgressBar, ProgressStyle};

pub struct BarReporter {
    bar: Mutex<Option<ProgressBar>>,
    enabled: bool,
}

impl BarReporter {
    pub fn new(enabled: bool) -> Self {
        Self {
            bar: Mutex::new(None),
            enabled,
        }
    }
}

impl ProgressReporter for BarReporter {
    fn start(&self, name: &str, total: u64) {
        if !self.enabled {
            return;
        }
        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::with_template(
                "  {msg} [{bar:30}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=> "),
        );
        bar.set_message(name.to_string());
        *self.bar.lock().unwrap() = Some(bar);
    }

    fn advance(&self, delta: u64) {
        if let Some(bar) = self.bar.lock().unwrap().as_ref() {
            bar.inc(delta);
        }
    }

    fn finish(&self) {
        if let Some(bar) = self.bar.lock().unwrap().take() {
            bar.finish_and_clear();
        }
    }
}
