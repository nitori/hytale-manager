# hytale-manager

> **Written by an AI.** Nearly every line of this project — code, tests, and this sentence —
> was produced by an LLM, directed by a human. Read the code before you point it at a server
> you care about. Its shape is loosely inspired by [uv](https://github.com/astral-sh/uv):
> a workspace of small crates, a thin binary, and toolchains provisioned automatically.

A single binary, `hy`, for running a Hytale dedicated server: it installs the server,
provisions Java, supervises the process, and takes backups.

**Status:** early. Everything below works. The server updates itself in place; there is no
`hy update` command to drive that yet.

## Install

```sh
curl -LsSf https://raw.githubusercontent.com/nitori/hytale-manager/master/install.sh | sh
```

A static binary into `~/.local/bin`, with no runtime dependency but `ca-certificates`.
Debian and Ubuntu add `~/.local/bin` to `PATH` only when it exists at login, so a first
install may need a re-login. `HY_INSTALL_DIR=/usr/local/bin` installs system-wide.

Windows binaries are on the [releases page](https://github.com/nitori/hytale-manager/releases/latest);
there is no Windows installer and no self-update there.

## Commands

`hy` searches upwards for the instance, so every command works from inside `Server/` too.

| Command | |
|---|---|
| `hy init [DIR]` | Write `hytale.toml`, adopting an existing install if there is one |
| `hy install [DIR]` | Authenticate, download, and unpack a server |
| `hy auth` | Authenticate this instance against a Hytale account |
| `hy run [-- ARGS]` | Run the server; `ARGS` pass through to it |
| `hy status` | Instance state, layout, version, Java, backup settings |
| `hy backup create\|list\|restore\|prune` | Snapshot and roll back server state |
| `hy java install\|list\|find\|pin\|uninstall\|dir` | Inspect and provision Java runtimes |
| `hy systemd` | Write a systemd unit for this instance |
| `hy completions <SHELL>` | Print a completion script (`bash`, `zsh`, `fish`, `powershell`, `elvish`) |
| `hy self update [--check]` | Replace this binary with the newest release |

Per-command flags:

| | |
|---|---|
| `install` | `--version`, `--patchline <release\|pre-release>`, `--force` |
| `run` | `--no-tui`, `--tui`, `--no-install` |
| `backup create` | `--force` |
| `backup restore` | `<ID>`, `--all`, `--include universe,config.json`, `--force` |
| `backup prune` | `--keep N` |
| `java install` | `[VERSION]`, `--force` |
| `java list` | `--only-managed`, `--only-system` |
| `java find` | `--executable` |
| `java pin` | `<VERSION>`, `--no-resolve` |
| `java uninstall` | `<KEY>`, as shown by `hy java list` |
| `systemd` | `--name`, `--user`, `--group`, `--scope <system\|user>`, `--exec`, `-o PATH` |

Global: `-j/--java <VERSION\|PATH>`, `--dir <PATH>`, `--no-java-download`, `--offline`,
`-q`/`-v`, `--color <auto\|always\|never>`.

Environment: `HY_HOME`, `HY_JAVA`, `HY_JAVA_DOWNLOADS` (`automatic`/`manual`/`never`),
`HY_DIR`, `HY_OFFLINE`, `HY_LOG`, `HY_INSTALL_DIR`, `HY_UPDATE_BASE_URL`.

## Notes

### Installing

`hy install` never runs the jar — it drives the OAuth device flow itself, fetches the
signed payload, verifies its SHA-256, and unpacks it. No Java is needed to install, only
to run.

- **Authenticating needs an interactive terminal**, since you have to be shown a code.
  `hy run` installs a missing server for you, but only when it can show one; under systemd
  or in CI it fails and tells you to run `hy install` first. Credentials are then reused.
- Only the newest build of a patchline can be installed — the asset service publishes
  exactly one — so `--version` is refused if it names anything else.
- `start.sh` and `start.bat` are generated and delegate to `hy run`. They must exist: the
  server disables its own update checker without a launcher beside `Assets.zip`.

### Running

`hy run` replaces `start.sh` — same working directory, same arguments, same exit-code-8
restart protocol, with `[java] options` instead of `jvm.options`.

- **Foreground only.** It propagates the server's exit code; use tmux, screen, or a systemd
  unit to keep it alive. There is no `hy start`/`stop` and no control socket.
- **Console UI** in a terminal: output scrolls above, your commands go in a box below.
  Up/Down recalls history, PageUp scrolls back, Esc follows the tail again, Ctrl-C stops
  the server. `--no-tui` gives plain output; a redirect or systemd unit falls back to it.
- **Not in mintty** (Git Bash's own window) — resizing corrupts the display there. Git Bash
  inside Windows Terminal is fine. `--tui` forces it anyway.
- Ctrl-C or `SIGTERM` asks the server to stop and waits for it to save; a second one kills
  it. A stop you asked for exits 0 even though the server reports 130, so systemd does not
  read it as a failure.
- One `hy run` per instance, enforced by a lock file: two servers sharing a `universe/`
  will corrupt it.
- The server runs with `Server/` as its working directory, so **relative paths in `-- ARGS`
  resolve against `Server/`**, not your shell. Use absolute paths there.

### Backups

The server takes its own backups every `--backup-frequency` minutes, but they cover
`universe/` only and it has no restore command. `hy backup` lists both origins together and
can restore either.

- Snapshots capture `[backup] include` — `universe/` plus `config.json`, `bans.json`,
  `permissions.json`, `whitelist.json`. `mods/` is excluded: jars are re-obtainable, not
  state.
- **Restoring rolls back only the world by default.** Reinstating an old `whitelist.json`
  would lock out anyone added since, and bans and config are the same story. `--all` or
  `--include` take more. A snapshot of the current state is always taken first.
- Restoring forks the history, so `snapshots/history.toml` records each one and `list`
  marks backups from before a restore as superseded — "restore the newest" after a rollback
  cannot silently drop you onto the abandoned branch.
- Backing up a running server is refused without `--force`: a live world is being written
  as it is read.

### systemd

```sh
hy systemd | sudo tee /etc/systemd/system/hy-main.service
sudo systemctl enable --now hy-main.service
```

The unit goes to stdout and the advice to stderr, so a redirect gets only the unit. `-o
PATH` writes the file instead, adding `.service` if you left it off, and the unit takes its
name from that file.

- **Install as the account the service will run as, first.** Authenticating needs a device
  code typed into a browser, and the Java store and credentials are per-user — so
  `sudo -u hytale hy install` is a prerequisite, not an optimisation. `hy systemd` warns
  when the account differs from yours.
- `--scope user` produces a `systemctl --user` unit, which needs `loginctl enable-linger`
  to survive logout.
- The unit sets `KillMode=mixed` and `TimeoutStopSec=120` deliberately: the defaults would
  send the JVM its own `SIGTERM` and `SIGKILL` it after 90 s, both of which cut across `hy`
  asking the server to save.

### Java

The server needs Java 25, and `hy` provisions it automatically — you should never install a
JDK by hand. The `hy java` commands exist for inspection and for pre-warming in CI.

**Which version** — `-j` → `.java-version` → `[java] version` in `hytale.toml` → `>=25`.
**Where from** — managed store → a system JDK that satisfies it → download from Adoptium.

Version requests: `25` · `25.0.4` · `>=25` · `25+` · `lts` · `latest` · `temurin@25` · a
path to a JDK. An open request like `>=25` resolves to the newest **LTS**, not the newest
release: Java 26 is out, but the manual specifies 25 and non-LTS releases stop getting
patches after ~6 months. Use `latest` to opt in.

`hytale.toml` states what the server *needs* (a range); `.java-version` records what this
instance *uses* (a pin), written the first time a command provisions Java. A pin that
contradicts the requirement is an error naming both files. Pins stay portable
(`temurin-25.0.4.1+1` — no OS or architecture), so an instance can move between machines.

Runtimes live in `~/.local/share/hy/java/` (`%APPDATA%\hy` on Windows), keyed as
`temurin-25.0.4.1+1-linux-x86_64`.

## `hytale.toml`

Written by `hy init`. Commented-out keys are the defaults.

```toml
[server]
patchline = "release"          # or "pre-release"
version   = "0.5.9"            # installed version; hy maintains this
bind      = "0.0.0.0:5520"     # QUIC over UDP

[java]
version = ">=25"               # requirement; the resolved pin goes in .java-version
options = ["-Xms2G", "-Xmx4G"] # JVM arguments, passed through untouched

[backup]
keep    = 10
include = ["universe", "config.json", "bans.json", "permissions.json", "whitelist.json"]

[server.hot_backup]            # performed by the server itself
enabled   = true
frequency = 30                 # minutes
```

`[java] options` is the only source of truth for JVM arguments. An existing `jvm.options`
is imported once by `hy init` and then ignored; `hy status` says so if a leftover copy
disagrees.

## Build

Requires Rust 1.97+ (edition 2024).

```sh
cargo build --release      # -> target/release/hy
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The `cfg(windows)` branches are otherwise written blind, so they are checked from Linux
with `cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu`.

## License

MIT
