# hytale-manager

A CLI for managing Hytale dedicated servers — installation, backups, and running the
server. The binary is called `hy`.

**Status:** early. `hy init`, `hy install`, `hy run`, `hy status`, and `hy java` work;
backups and updates do not yet. See [PLAN.md](PLAN.md) for the roadmap.

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
| `hy status` | Instance state, layout, version, Java, and backup settings |

`hy` searches upwards for the instance, so these work from inside `Server/`.

### `hy install`

`Assets.zip` is 3.3 GB and is not on maven, so it cannot simply be downloaded. `hy` fetches
the server jar, runs it in `--bootstrap` mode, and drives the console: it sends
`/auth login device`, shows you the device code to enter in a browser, then sends
`/update download` to pull the payload.

That means installing **needs an interactive terminal**. `hy run` will install a missing
server for you, but only when it can actually show you a code — under systemd or in CI it
fails and tells you to run `hy install` by hand first.

### `hy run`

Replaces `start.sh`: same working directory and arguments, same exit-code-8 restart
protocol, with `[java] options` instead of `jvm.options`. It runs in the **foreground** and
propagates the server's exit code — use tmux/screen or a systemd unit to keep it alive.
There is no `hy start`/`stop`, and no control socket.

Ctrl-C (or `SIGTERM`) asks the server to stop and waits for it to save; a second one kills
it. One `hy run` per instance is enforced by a lock file, since two servers sharing a
`universe/` will corrupt it.

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
is out, but the server manual specifies 25, `HytaleServer.aot` won't load on a different
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
