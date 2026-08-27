## Comments and docstrings

**Default to no comment.** Most code needs none — assume an experienced reader. Signatures and
type names are the documentation.

When a comment does earn its place:

- **Single line, and only the *why*.** Never restate what the code does. The cases worth a
  comment are non-obvious constraints and rejected alternatives — a threshold chosen for
  hysteresis rather than the break-even point, a bound that must stay conservative, an
  invariant a caller has to uphold.
- **Never document changes.** No "changed from A to B", "now uses B", "previously did A", and no
  commented-out old versions. The code is the current state; its history is git's job.
- **When touching existing comments, consider shortening or deleting them.** A comment that has
  drifted from the code, restates it, or narrates an old edit should go — removing it is a real
  improvement, not a side task.

## Project

`hy` — one binary that installs a Hytale dedicated server, provisions Java, supervises the
process, and takes backups. Cargo workspace, edition 2024. README.md documents the CLI and
the behavioural constraints behind it; read it before changing how a command behaves.

### Crates

| | |
|---|---|
| `hy-instance` | `hytale.toml`, directory layout, JVM options |
| `hy-auth` | Account credentials: OAuth device flow, encrypted store |
| `hy-dist` | Server downloads: asset service client, Maven metadata, payload verification |
| `hy-java` | Java discovery, Adoptium downloads, managed store, version resolution and pins |
| `hy-run` | Process supervision: command building, staging, instance lock, signals, console I/O |
| `hy-backup` | Snapshot archives, manifest, restore history, prune |
| `hy-cli` | clap definitions only — no logic |
| `hytale-manager` | the `hy` binary: `src/commands/` one module per subcommand, `src/tui/` the ratatui console |

Crates below `hytale-manager` are library-only and must stay printing-free; user-facing
output goes through `printer.rs` / `progress.rs` in the binary.

### Notes

- `var/` is gitignored scratch: test server instances and a `uv` checkout kept as a
  reference for workspace shape.
- `cfg(windows)` branches are written blind from Linux; check them with
  `cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu`.
- Don't test process handling by spawning real processes or stub scripts — add a seam and
  fake it in-process.
