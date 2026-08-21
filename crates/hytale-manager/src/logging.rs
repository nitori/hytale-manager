//! Tracing setup.
//!
//! Diagnostic logging is separate from user-facing output: [`crate::printer`] speaks to the
//! operator, tracing speaks to whoever is debugging. `-v` raises the level; `HY_LOG`
//! overrides it entirely.

use tracing_subscriber::EnvFilter;

pub fn init(verbose: u8, quiet: bool) {
    let default = if quiet {
        "error"
    } else {
        match verbose {
            0 => "warn",
            1 => "hy=debug,hy_java=debug",
            _ => "debug",
        }
    };

    let filter = EnvFilter::try_from_env("HY_LOG")
        .unwrap_or_else(|_| EnvFilter::new(default));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(verbose > 1)
        .without_time()
        .init();
}
