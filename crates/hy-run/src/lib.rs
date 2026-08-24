//! Supervising the Hytale server process.
//!
//! `hy run` replaces `start.sh`: same working directory, same argument shape, same exit-8
//! restart protocol, with `[java] options` in place of the `jvm.options` argfile.
//!
//! `hy` owns the child's stdin rather than inheriting it: a terminal that turns Ctrl-C into
//! a literal `0x03` byte leaves the server's console reader dead and the server unstoppable,
//! so stopping goes through its own `shutdown` command instead. Nothing listens on a socket.

pub mod command;
pub mod console;
pub mod error;
pub mod lock;
pub mod output;
pub mod process;
pub mod shell;
pub mod signal;
pub mod staging;
pub mod supervisor;

pub use command::ServerCommand;
pub use console::Console;
pub use error::{Error, Result};
pub use lock::RunLock;
pub use output::{Output, OutputSink, StopHandle};
pub use shell::Shell;
pub use supervisor::{NoReporter, Outcome, RESTART_EXIT_CODE, RunOptions, RunReporter, run};
