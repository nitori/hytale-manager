//! `hy completions` — print a completion script.
//!
//! Straight to stdout with nothing else on any stream: the usual use is
//! `eval "$(hy completions bash)"` in a shell profile, where a stray note would print on
//! every login.

use std::io::Write;

use anyhow::Result;
use hy_cli::CompletionsArgs;

use crate::commands::Context;

pub fn completions(args: CompletionsArgs, _ctx: &Context) -> Result<()> {
    // Rendered to a buffer first because clap_complete panics on a write error, and
    // `hy completions bash | head` is a reasonable thing to type.
    let mut script = Vec::new();
    hy_cli::completions(args.shell, &mut script);

    match std::io::stdout().lock().write_all(&script) {
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
}
