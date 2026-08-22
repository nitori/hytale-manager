//! Supervising the Hytale server process.
//!
//! `hy run` replaces `start.sh`: same working directory, same argument shape, same exit-8
//! restart protocol, with `[java] options` in place of the `jvm.options` argfile.
//!
//! Stdio is inherited, so the terminal is the server console and journald captures the
//! output under systemd. Nothing listens on a socket.

pub mod command;
pub mod console;
pub mod error;
pub mod lock;
pub mod session;
pub mod shell;
pub mod signal;
pub mod staging;
pub mod supervisor;

pub use command::ServerCommand;
pub use console::Console;
pub use error::{Error, Result};
pub use lock::RunLock;
pub use session::Session;
pub use shell::Shell;
pub use supervisor::{NoReporter, Outcome, RESTART_EXIT_CODE, RunOptions, RunReporter, run};
