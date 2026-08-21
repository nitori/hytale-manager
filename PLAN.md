# hytale-manager — Implementation Plan

## Context

There is no good tool for running a Hytale dedicated server. The official story is a
`HytaleServer.jar` plus a ~40-line `start.sh` wrapper loop, a JDK you install by hand, and
admin actions typed as `/commands` into the server's stdin. That works, but leaves the
operator to manage Java, instance layout, update staging, and backups manually.

`hytale-manager` replaces that with a single binary, `hy`, that owns the whole lifecycle:
provision the JDK, install the server, supervise it, apply updates, and take backups.

Structure follows [uv](https://github.com/astral-sh/uv): a Cargo workspace of small
focused crates, clap definitions separated from command implementations, and a layered
settings resolution. uv is 72 crates; we start at 8 and split only when a boundary hurts.

---

## Background: what the server actually expects

From the [Hytale Server Manual](https://support.hytale.com/hc/en-us/articles/45326769420827-Hytale-Server-Manual)
(retrieved 2026-08-21) and live endpoint checks.

### Required on-disk layout

```
game/
├── Assets.zip                 ~3.3 GB
├── start.sh / start.bat       wrapper script — we replace this
├── jvm.options                optional, one JVM arg per line, # comments
└── Server/                    server runs with cwd HERE
    ├── HytaleServer.jar
    ├── HytaleServer.aot       AOT cache; -XX:AOTCache= is a large boot-time win
    ├── auth.enc / auth.key    device-flow OAuth credentials
    ├── config.json
    └── (created at runtime)
        universe/              worlds + player saves
        logs/  mods/  .cache/  backups/
        bans.json  permissions.json  whitelist.json
```

The updater is **disabled** unless this exact layout is detected: server started from
`Server/`, with `Assets.zip` and a launcher script in the parent.

### Facts that drive design

| Fact | Consequence |
|---|---|
| Server exits with **code 8** to request an update-restart; it never restarts itself | `hy run` is a supervisor loop, not a one-shot spawn |
| Staged updates land in `updater/staging/`; wrapper copies them over, preserving config/saves/mods | We reimplement selective staging application |
| All admin actions are `/commands` on stdin (`/auth`, `/update`) | One durable stdin/stdout channel, shared by `install`, `auth`, `update`, `console` |
| Requires **Java 25**, x64 + arm64 | JDK is a managed dependency (see below) |
| Runs QUIC over **UDP 5520** (not TCP) | Port-forward guidance; no TCP health checks |
| Config JSONs are rewritten at runtime by in-game actions | Cold backups only for full fidelity; never edit config while running |
| Built-in backups exist: `--backup --backup-dir backups --backup-frequency 30` | We delegate hot/periodic backups, own cold snapshots |
| `Assets.zip` ≈ 3.3 GB | Resumable downloads + progress; exclude from backups |
| Sentry crash reporting is on by default | Expose `--disable-sentry` for plugin development |
| `HYTALE_DISABLE_UPDATES` env var disables the update system | Honour it; don't fight the server's own updater |

### Live endpoints (verified 2026-08-21)

**Server versions** — public, no auth:

```
https://maven.hytale.com/release/com/hypixel/hytale/Server/maven-metadata.xml
https://maven.hytale.com/pre-release/com/hypixel/hytale/Server/maven-metadata.xml
```

> **Correction to the manual.** The manual's example version `2026.01.22-6f8bdbdc4` is
> stale. Live metadata is semver: release `0.5.9` (0.5.5–0.5.9 retained), pre-release
> `0.6.0-pre.13`. Parse as semver-with-prerelease, **not** date-based.
>
> **Only five versions are retained per patchline.** A pinned old version will 404
> eventually — `hy` must fail with a clear "version no longer published" error and list
> what is currently available.

**JDK** — public, no auth:

```
GET api.adoptium.net/v3/info/available_releases
    → available_lts_releases [8,11,17,21,25], most_recent_lts 25

GET api.adoptium.net/v3/assets/latest/25/hotspot
      ?os=linux&architecture=x64&image_type=jdk&vendor=eclipse
    → jdk-25.0.4.1+1, OpenJDK25U-jdk_x64_linux_hotspot_25.0.4.1_1.tar.gz
      141.3 MB, sha256 supplied, hosted on GitHub releases
```

Adoptium is the project; **Temurin is its JDK build** — the manual's "we recommend
Adoptium" means Temurin. One request yields URL + checksum.

---

## Architecture

```
Cargo.toml                     [workspace] members = ["crates/*"], resolver 2, edition 2024
                               all internal deps via [workspace.dependencies]
crates/
  hytale-manager/   bin `hy`. Thin. lib.rs, settings.rs, printer.rs, logging.rs,
                    exit.rs, commands/{init,install,run,console,status,auth,
                    update,backup,java}.rs — each returns ExitStatus
  hy-cli/           clap derive structs ONLY, no implementation deps
  hy-settings/      layered resolution: CLI flag > HY_* env > <instance>/hytale.toml
                    > ~/.config/hy/hy.toml > defaults
  hy-instance/      the game/ layout: discover, validate, create, path accessors,
                    installed-version stamp, .java-version read/write
  hy-java/          Adoptium client, managed store, system discovery, version requests,
                    the resolution ladder, auto-provisioning
  hy-dist/          maven-metadata version listing, bootstrap acquisition, staged-update
                    inspection
  hy-run/           supervisor: JVM spawn, exit-8 loop, staging application, stdin
                    console channel, graceful shutdown, log tee
  hy-backup/        cold snapshot, restore, list, prune
```

Rationale for the split: `hy-java` and `hy-run` are the two genuinely hard subsystems and
both are independently testable. `hy-cli` stays dependency-light so `--help` output and
completions can be generated without building the world (uv does this).

### Key dependencies

`clap` (derive), `tokio` (process/signal/fs), `reqwest` (rustls-tls, streaming),
`serde`/`serde_json`, `toml`, `thiserror` + `anyhow`, `tracing`/`tracing-subscriber`,
`indicatif`, `anstream`/`owo-colors`, `sha2`, `zip`, `tar` + `flate2`, `fs4` (lockfiles),
`tempfile`, `etcetera` (XDG dirs), `which`, `semver`, `quick-xml`.

rustls over openssl — no system OpenSSL dependency, keeps cross-compilation simple.

---

## Java provisioning (automatic)

**Requirement: the operator never installs Java by hand.** Any command needing a JVM
provisions one transparently. `hy java install` exists only for pre-warming and CI.

### Resolution: two stages

**Stage A — decide which version is wanted.** First match wins:

1. `-p` / `--java <version|path>` on the command line
2. `.java-version` pin file in the instance directory
3. `[java] version` requirement in `hytale.toml`
4. Built-in default: `>=25`

**Stage B — satisfy it.** First match wins:

1. Managed store — newest install satisfying the requirement
2. System discovery — `JAVA_HOME`, `PATH`, `/usr/lib/jvm`, macOS `/usr/libexec/java_home`,
   Windows registry — **only if it satisfies the requirement**
3. **Auto-download** the newest matching Temurin from Adoptium into the managed store

Then, if no `.java-version` exists, write the resolved version to it so subsequent runs are
reproducible.

Stage B step 3 prints what it is doing and why (`no Java 25+ found; installing
temurin-25.0.4.1+1`), never silently. A system JDK that is present but too old is reported
as such, so the operator understands why a download happened.

### Default is newest LTS, not newest release

Verified 2026-08-21 against the Adoptium API:

```
available:        [8, 11, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26]
LTS:              [8, 11, 17, 21, 25]
most recent LTS:  25
most recent GA:   26        ← jdk-26.0.2+10, downloadable today
tip (dev):        28
```

An open requirement like `>=25` resolves to **25**, not 26. Rationale:

- The manual specifies Java 25; 26 is untested against Hytale's build.
- **`HytaleServer.aot` is version-stamped.** An AOT cache built for 25 will not load on a
  26 JVM, silently forfeiting the boot-time win the manual ships it for.
- Non-LTS feature releases receive ~6 months of updates. An internet-exposed UDP server
  that auto-tracks feature releases will periodically sit on an unpatched JVM.

`-p 26`, `-p latest`, or an explicit `[java] version = "26"` opt in deliberately. "Latest"
remains reachable; it is simply not what an unattended `hy run` selects on its own.

### `.java-version` pin file

Same split uv has between `requires-python` (a range) and `.python-version` (a pin):
`hytale.toml` states what the server *needs*, `.java-version` records what this instance
*uses*. Written automatically on first resolve, changed with `hy java pin`.

Format is one line, and must stay **portable** — instances get moved between machines and
architectures, so the file never contains the platform key:

```
temurin-25.0.4.1+1      # vendor + version
25                      # or just a major version
```

OS/arch is resolved locally at use time. `.java-version` is the well-established jenv
convention, so existing tooling reads it too.

If the pin does not satisfy the `hytale.toml` requirement, that is a hard error naming both
files — never a silent override in either direction.

### Escape hatches

Mirrors uv's `python-downloads` setting:

| Control | Effect |
|---|---|
| `--no-java-download` / `java-downloads = "never"` | Fail at step 4 instead of downloading |
| `java-downloads = "manual"` | Only `hy java install` may download |
| `--offline` | No network at all |
| `HY_JAVA_DOWNLOADS`, `HY_JAVA` | Env equivalents |

### Managed store

```
~/.local/share/hy/                          (etcetera; %LOCALAPPDATA%\hy on Windows)
├── java/
│   └── temurin-25.0.4.1+1-linux-x86_64/    key: vendor-version-os-arch
└── cache/
    └── downloads/
```

Correctness requirements:

- **Atomic installs** — stream to temp, verify sha256 against the API-supplied digest,
  extract to a temp dir, then atomically rename into the keyed directory. A partial
  extraction must never be discoverable as a usable JDK.
- **Cross-process locking** — `fs4` lockfile per key, so two concurrent `hy run` calls
  don't race on the same download.
- **Resumable** — HTTP range requests; 141 MB (and later 3.3 GB assets) should not restart
  from zero.
- **macOS nesting** — Temurin macOS tarballs nest the JDK under `Contents/Home/`; normalise
  so `java_home()` is uniform across platforms.

### Version requests

Accept `25`, `25.0.4`, `>=25`, `temurin@25`, plus the aliases `lts` and `latest`. Default
requirement is `>=25` (per the manual), overridable in config — the server's Java floor
will move over time and must not be hardcoded at a call site.

An open range resolves to the newest **LTS** that satisfies it; `latest` is required to
cross onto a non-LTS feature release.

Distribution is modelled as a `JavaDistribution` enum with **Temurin the only variant
implemented**, leaving room for Corretto/Zulu/Graal without designing for them now
(mirrors `uv-python`'s implementation enum).

---

## Command surface

```
hy init <dir>                     scaffold instance + hytale.toml
hy install [<dir>]                provision Java → bootstrap jar → device auth → /update setup
hy auth login|status              drive /auth on stdin, surface the device code
hy run [-- <server args>]         replaces start.sh
hy console <cmd>                  send a command to the running server
hy status                         instance version, Java, running state, port
hy update check|download|apply|patchline
hy backup create|list|restore <id>|prune --keep N
hy java install|list|find|pin <version>|uninstall
hy version | hy self update
```

Global flags: `-q/-v`, `--color`, `--offline`, `--dir <instance>`, `--no-java-download`.

`-p` / `--java <version|path>` is accepted by every JVM-touching command (`run`, `install`,
`update`, `console`, `status`), mirroring `uv run -p`. If the requested version is absent it
is installed automatically, subject to the escape hatches above.

### Files `hy` adds to an instance

Both sit in the instance root, alongside the manual's `Assets.zip` / `start.sh` /
`jvm.options`:

```
game/
├── hytale.toml       instance settings (below)
└── .java-version     resolved Java pin, auto-written on first resolve
```

`jvm.options` is read by `start.sh`, not by the server — the wrapper passes it to the JVM
as an `@argfile`. Since `hy run` replaces the wrapper, **`hy` is its only consumer**, so
`[java] options` in `hytale.toml` is the sole source of truth and is passed directly on
the `java` command line. No argfile, and `hy` never writes `jvm.options`.

`hy` reads that file exactly once: at `hy init` / first adoption of an existing instance,
its contents are imported into `[java] options`, so hand-tuned settings like `-Xmx8G`
survive the switch. Afterwards it is ignored — and if a leftover copy disagrees with
`[java] options`, `hy status` says so rather than letting it look effective.

`start.sh` itself must stay on disk: the server disables its update checker unless it
detects a launcher script beside `Assets.zip`. `hy` therefore leaves it alone (the
server's own updater reinstalls it anyway) and simply does not use it.

`-XX:AOTCache=` is governed by `[java] aot`; specifying it by hand in `options` is a
conflict `hy` reports rather than merges.

### `hytale.toml` (per instance)

```toml
[server]
patchline = "release"        # or "pre-release"
version   = "0.5.9"          # installed version stamp
bind      = "0.0.0.0:5520"

[java]
version = ">=25"             # requirement, not a pin — resolves to newest LTS (25)
options = ["-Xms2G", "-Xmx4G"]   # sole source of truth; passed straight to `java`
aot     = true               # -XX:AOTCache=HytaleServer.aot

[backup]
keep = 10
exclude = ["Assets.zip", ".cache", "logs"]

[server.hot_backup]          # delegated to the server's own --backup
enabled   = true
frequency = 30
```

---

## Implementation phases

**✅ Phase 0 — workspace scaffold.** Root `Cargo.toml` (`members = ["crates/*"]`),
`hy-cli` command tree, `printer`/`logging`/`progress`, exit-code plumbing.

> Deviation: three crates exist (`hytale-manager`, `hy-cli`, `hy-java`) rather than eight
> skeletons. Empty placeholder crates are clutter; the remaining five arrive with the phases
> that fill them. `config.rs` in the binary is a deliberate stand-in for `hy-instance`,
> reading only `[java] version`, and is superseded in phase 2.

**✅ Phase 1 — `hy-java`.** Adoptium client, managed store with atomic+locked
installs, system discovery, the two-stage resolution, newest-LTS selection,
`.java-version` read/write, auto-provisioning. `hy java install|list|find|pin|uninstall|dir`
and `-p`/`--java`. 23 tests, clippy clean.

**⬜ Phase 2 — `hy-instance`.** Layout discovery/validation/creation, `hytale.toml` parsing,
version stamp, one-time `jvm.options` import. Deliverable: `hy init`, `hy status` on an
existing hand-made install.

**⬜ Phase 3 — `hy-run`.** The core. Spawn the JVM with resolved Java + `[java] options` +
AOT cache from the correct cwd; the exit-8 loop; selective staging application; the stdin console
channel; graceful shutdown on SIGINT/SIGTERM (and Windows Ctrl-C); log tee to `logs/`.
Deliverable: `hy run` fully replaces `start.sh`. **Build the console channel first** —
phases 4 and 5 depend on it.

**⬜ Phase 4 — `hy-dist` + acquisition.** maven-metadata version listing, the `--bootstrap`
flow driven over phase 3's console channel, device-code surfacing with a countdown,
payload extraction, `+x` on `start.sh`, auth-file migration into `Server/`. Deliverable:
`hy install`, `hy auth login`, `hy update *`.

**⬜ Phase 5 — `hy-backup`.** Cold snapshot of `universe/` + config JSONs + `mods/` into a
timestamped archive, with stop-then-restart-if-running. Restore with a pre-restore safety
snapshot. `list`, `prune --keep N`. Deliverable: `hy backup *`.

**⬜ Phase 6 — polish.** `hy self update`, shell completions from `hy-cli`, README, systemd
unit generation if wanted.

---

## Verification

Runnable without any Hytale credentials:

- `cargo test --workspace`; `cargo clippy --workspace -- -D warnings`
- **Java, end to end:** `hy java install 25` on a machine with no JDK → verify checksum
  path, atomic rename, `hy java find` resolves it, a wrong-version system JDK is correctly
  rejected in favour of a download
- **LTS default:** an open `>=25` requirement resolves to 25, not the GA 26; `-p 26` and
  `-p latest` reach 26 explicitly
- **Pin round-trip:** first resolve writes `.java-version`; a subsequent run honours it
  without re-resolving; a pin conflicting with the `hytale.toml` requirement errors naming
  both files; a pin written on x86_64 resolves correctly on aarch64
- **Concurrency:** two simultaneous `hy java install 25` → one downloads, one waits, store
  is not corrupted
- **Version listing:** `hy update check` against live maven-metadata → parses semver,
  distinguishes patchlines
- **Offline:** `--offline` and `--no-java-download` fail with actionable messages

Requires a real server payload:

- **Supervisor:** a stub JAR exiting with code 8 → verify the relaunch loop and that
  staging application preserves `config.json`, `universe/`, `mods/`
- **Signals:** SIGTERM during run → graceful shutdown, no world corruption
- **Backup round-trip:** create → mutate world → restore → verify `universe/` matches
- **Full path:** `hy init` → `hy install` → device auth → `hy run` → connect a client on
  UDP 5520 → `hy backup create` → `hy update check`

---

## Open questions / deferred

- **`flock` is unreliable on some filesystems.** The store lock is the primary guard against
  concurrent installs, but WSL2's `/mnt/*` Windows mounts are v9fs and NFS behaves similarly,
  where advisory locking may be a no-op. `download.rs` therefore also tolerates losing a
  rename race, and the checksum is the final backstop. Worth a warning if `HY_HOME` is
  detected on such a filesystem.
- **Full `--help` flag list uncaptured.** The manual truncates `java -jar HytaleServer.jar
  --help` at `--backup-frequency`. Capture the real list during phase 4 and reconcile.
- **Multi-instance registry** deferred. cwd/`--dir` addressing only; a global named registry
  can be layered on later without breaking the model.
- **systemd unit generation** deferred to phase 6.
- **Protocol version tolerance:** currently client and server must match exactly, so a
  server must update immediately on release. The manual promises ±2 tolerance later —
  until then `hy update` should treat "update available" as urgent, not optional.
