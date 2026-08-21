//! `hy init` — set up a server instance.

use anyhow::{Context as _, Result};
use hy_cli::InitArgs;
use hy_java::VersionRequest;
use owo_colors::OwoColorize;

use crate::commands::Context;

pub fn init(args: InitArgs, ctx: &Context) -> Result<()> {
    let root = match args.dir {
        Some(dir) => dir,
        None => ctx.dir.clone(),
    };

    // The config records what the server needs, not where one JDK happens to live today.
    let requirement = match args.selector.java.as_deref() {
        Some(raw) => raw
            .parse::<VersionRequest>()
            .with_context(|| format!("invalid Java requirement `{raw}`"))?,
        None => VersionRequest::default_requirement(),
    };

    let result = hy_instance::init(&root, &requirement.to_string())?;
    let layout = result.instance.layout();

    if result.adopted {
        ctx.printer.event(format!(
            "Adopted the server install at {}",
            root.display().bold()
        ));
    } else {
        ctx.printer
            .event(format!("Initialised {}", root.display().bold()));
    }

    ctx.printer
        .detail(format!("wrote {}", layout.config().display()));
    ctx.printer
        .detail(format!("java requirement `{requirement}`"));

    if !result.imported_options.is_empty() {
        ctx.printer.event(format!(
            "Imported {} JVM {} from jvm.options",
            result.imported_options.len(),
            if result.imported_options.len() == 1 {
                "argument"
            } else {
                "arguments"
            }
        ));
        ctx.printer.detail(result.imported_options.join(" "));
        ctx.printer
            .detail("now maintained as `[java] options`; jvm.options is no longer read");
    }

    if !result.adopted {
        ctx.printer
            .detail("no server install here yet — `hy install` will fetch one");
    }

    Ok(())
}
