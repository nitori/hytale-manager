# hytale-manager

> **Written by an AI.** Nearly every line of this project — the code, the tests, the
> roadmap in [PLAN.md](PLAN.md), and this sentence — was produced by an LLM, directed
> by a human. Treat it accordingly: read the code before you point it at a server
> you care about.
>
> Its shape is loosely inspired by [uv](https://github.com/astral-sh/uv) — a workspace of
> small focused crates, a thin binary, and toolchains provisioned automatically rather than
> installed by hand.

A CLI for managing Hytale dedicated servers — installation, backups, and running the
server. The binary is called `hy`.

**Status:** early. `hy init`, `hy install`, `hy run`, `hy backup`, `hy status`, `hy java`,
`hy systemd`, and `hy completions` work; updates do not yet. See [PLAN.md](PLAN.md) for the
roadmap.

## Build

Requires Rust 1.97+ (edition 2024).

```sh
cargo build --release      # -> target/release/hy
cargo test --workspace
```

## Commands

| Command | Description |
|---|---|
| `hy init [DIR]` | Write `hytale.toml`, adopting an existing install if there is one |
| `hy install [DIR]` | Download a server and authenticate it (`--version`, `--patchline`) |
| `hy run [-- ARGS]` | Run the server; `ARGS` pass through to it |
| `hy backup create\|list\|restore\|prune` | Snapshot and roll back server state |
| `hy status` | Instance state, layout, version, Java, and backup settings |
| `hy systemd` | Write a systemd unit for this instance |
| `hy completions <SHELL>` | Print a completion script |

`hy` searches upwards for the instance, so these work from inside `Server/`.

### `hy install`

`Assets.zip` is 3.3 GB and is not on maven, so it cannot simply be downloaded. `hy` fetches
the server jar, runs it in `--bootstrap` mode, and drives the console: it sends
`/auth login device`, shows you the device code to enter in a browser, then sends
`/update download` to pull the payload.

That means installing **needs an interactive terminal**. `hy run` will install a missing
server for you, but only when it can actually show you a code — under systemd or in CI it
fails and tells you to run `hy install` by hand first.

### `hy backup`

The server takes its own backups every `--backup-frequency` minutes, but they cover
`universe/` only and it has **no restore command**. `hy backup` fills both gaps: it lists
the server's archives alongside its own, and can restore either.

Snapshots capture `universe/` plus `config.json`, `bans.json`, `permissions.json`, and
`whitelist.json` — configurable via `[backup] include`. `mods/` is excluded by default:
jars are re-obtainable, not state.

**Restoring rolls back only the world by default.** Reinstating an old `whitelist.json`
would lock out anyone added since, and bans and config are the same story. Use `--all`, or
`--include universe,config.json`, to take more. A snapshot of the current state is always
taken first.

Because restoring forks the history, `snapshots/history.toml` records each one and `list`
marks backups from before a restore as superseded — so "restore the newest" after a
rollback can't silently drop you onto the abandoned branch.

Backups of a running server are refused unless you pass `--force`, since a live world is
being written as it's read.

### `hy run`

Replaces `start.sh`: same working directory and arguments, same exit-code-8 restart
protocol, with `[java] options` instead of `jvm.options`. It runs in the **foreground** and
propagates the server's exit code — use tmux/screen or a systemd unit to keep it alive.
There is no `hy start`/`stop`, and no control socket.

In a terminal it opens a **console UI**: server output scrolls above, your commands go in a
box below, so typing isn't interleaved with log lines. Up/Down recalls history, PageUp
scrolls back (Esc follows the tail again), Ctrl-C stops the server. `--no-tui` gives the
plain scrolling output instead, and a redirect or systemd unit falls back to it
automatically.

**Not in mintty** (Git Bash's own window) — resizing corrupts the display there, so `hy`
uses plain output and you lose nothing but the panes. Git Bash inside Windows Terminal is
fine. `--tui` forces it if you want to try anyway.

Ctrl-C (or `SIGTERM`) asks the server to stop and waits for it to save; a second one kills
it. A stop you asked for exits 0 even though the server reports 130, so systemd does not
read it as a failure. One `hy run` per instance is enforced by a lock file, since two
servers sharing a `universe/` will corrupt it.

The server runs with `Server/` as its working directory — it disables its own update
checker otherwise — so **relative paths in `-- ARGS` resolve against `Server/`**, not your
shell. Use absolute paths there.

### `hy systemd`

Writes a unit for the instance to stdout, so it can go straight where it belongs:

```sh
hy systemd | sudo tee /etc/systemd/system/hy-main.service
sudo systemctl enable --now hy-main.service
```

`-o PATH` writes the file instead, adding `.service` if you left it off — `-o hytale`
produces `hytale.service`, and the unit takes its name from that file.

It runs as your user and group by default; `--user`/`--group` name another account, and
`--scope user` produces a `systemctl --user` unit instead (which needs
`loginctl enable-linger` to survive logout). The advice prints on stderr, so redirecting
stdout gets only the unit.

**Install as the account the service will run as, first.** Authenticating needs a device
code typed into a browser — a service cannot do it — and the Java store and credentials are
per-user, so `sudo -u hytale hy install` is a prerequisite, not an optimisation. `hy systemd`
warns when the account differs from yours.

The generated unit sets `KillMode=mixed` and `TimeoutStopSec=120` deliberately: the default
would send the JVM its own `SIGTERM` and then `SIGKILL` it after 90s, both of which cut
across `hy` asking the server to save.

### Shell completions

```sh
hy completions bash > /etc/bash_completion.d/hy      # or: eval "$(hy completions bash)"
hy completions zsh  > ~/.zfunc/_hy
hy completions fish > ~/.config/fish/completions/hy.fish
```

`powershell` and `elvish` work too.

### `hy java`

The Hytale server needs Java 25. `hy` provisions it automatically — you should never have
to install a JDK by hand. These commands exist for inspection and for pre-warming in CI.

| Command | Description |
|---|---|
| `hy java install [VERSION]` | Install a runtime (`--force` to reinstall) |
| `hy java list` | List runtimes (`--only-managed`, `--only-system`) |
| `hy java find` | Show which runtime would be used (`--executable` for just the path) |
| `hy java pin <VERSION>` | Write `.java-version` (`--no-resolve` to skip the check) |
| `hy java uninstall <KEY>` | Remove a managed runtime, by key from `hy java list` |
| `hy java dir` | Print the managed runtime directory |

### Version requests

`25` · `25.0.4` · `>=25` · `25+` · `lts` · `latest` · `temurin@25` · a path to a JDK

An open request like `>=25` resolves to the newest **LTS**, not the newest release. Java 26
is out, but the server manual specifies 25, the server is untested on a different
JVM, and non-LTS releases stop getting patches after ~6 months. Use `latest` to opt in.

### Global options

| Flag | Description |
|---|---|
| `-j`, `--java <VERSION\|PATH>` | Which Java to use; installed automatically if missing |
| `--dir <PATH>` | Instance directory (default: cwd) |
| `--no-java-download` | Never download a runtime |
| `--offline` | No network access |
| `-q` / `-v` | Quieter / more verbose |
| `--color <auto\|always\|never>` | Color output |

Environment: `HY_HOME`, `HY_JAVA`, `HY_JAVA_DOWNLOADS` (`automatic`/`manual`/`never`),
`HY_DIR`, `HY_OFFLINE`, `HY_LOG`.

## How Java is chosen

**Which version** — `-j` → `.java-version` → `[java] version` in `hytale.toml` → `>=25`.

**Where from** — managed store → a system JDK that satisfies it → download from Adoptium.

`hytale.toml` states what the server *needs* (a range); `.java-version` records what this
instance *uses* (a pin), and is written the first time a command provisions Java. JVM arguments live in
`[java] options`; an existing `jvm.options` is imported once by `hy init` and then unused. A pin that contradicts the
requirement is an error naming both files. Pins stay portable (`temurin-25.0.4.1+1`, no
OS or architecture), so an instance can move between machines.

Runtimes live in `~/.local/share/hy/java/` (`%LOCALAPPDATA%\hy` on Windows), keyed as
`temurin-25.0.4.1+1-linux-x86_64`.

## License

MIT
