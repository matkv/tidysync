# tidysync – Development Plan

## What this project does

`tidysync` is a CLI tool that connects to the [SyncThing](https://syncthing.net/) REST API
and automatically moves files from one pre-configured folder to another whenever SyncThing
finishes syncing a file. It is also intentionally used as a learning project for Rust best
practices.

## Current state (as of initial planning)

The SyncThing API integration works end-to-end:

| File | What it does |
|---|---|
| `src/cli.rs` | CLI arg parsing via `clap` derive macros; global `--url` / `--api-key` |
| `src/client.rs` | `SyncThingClient` — async HTTP client; `ping`, `system_status`, `folders`, `devices`, `watch_item_finished` |
| `src/types.rs` | Serde structs for API responses: `SystemStatus`, `Device`, `Folder`, `ItemFinishedEvent` |
| `src/main.rs` | Entry point; routes CLI commands to client methods |

**What works:** ping, status, list folders, list devices, poll `ItemFinished` events in a loop.

**What's missing:** the watch loop detects events but does nothing with them — no files are
actually moved. There is no config file, no logging, and a handful of code quality issues.

**Known rough edges in the current code:**
- `use core::sync;` in `main.rs` — unused import, doesn't compile cleanly
- No `anyhow::Context` on `?` sites — errors are bare and hard to diagnose
- `action` and `item_type` in `ItemFinishedData` are plain `String`; should be enums
- No `#[serde(default)]` on API structs — unknown fields from future API versions will panic
- All output uses `println!` — no structured logging, no log levels, nothing to grep in production
- Watch loop has no graceful shutdown, no backoff on failure, no Ctrl+C handling

---

## Phases

### Phase 1 — Code Cleanup

**Goal:** Make the existing code idiomatic and robust before building on top of it.  
**Rust concepts covered:** `anyhow::Context`, enums + `serde`, doc comments, linting.

#### Tasks

**1.1 — Remove unused import**  
File: `src/main.rs`, line 1  
Delete `use core::sync;`. It was likely a mistake; nothing in the codebase uses it.

**1.2 — Add error context**  
File: `src/client.rs`  
Add `.context("...")` after every `?` that hits a network or deserialization boundary.
Requires adding `use anyhow::Context;`. Examples:

```rust
// Before
.send().await?.error_for_status()?

// After
.send().await.context("HTTP request to /rest/system/ping failed")?
.error_for_status().context("Syncthing returned an error status")?
```

Do this for every method: `ping`, `system_status`, `folders`, `devices`, `watch_item_finished`.

**1.3 — Replace string fields with enums**  
File: `src/types.rs`  
`ItemFinishedData.action` is always one of `"update"`, `"delete"`, `"metadata"`.  
`ItemFinishedData.item_type` is always `"file"` or `"dir"`.  
Replace both with enums:

```rust
#[derive(Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ItemAction {
    Update,
    Delete,
    Metadata,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    File,
    Dir,
}
```

Update `ItemFinishedData` to use them. Update the `println!` in `watch_item_finished`
accordingly (they implement `Debug`, or add `Display`).

**1.4 — Add serde resilience**  
File: `src/types.rs`  
Add `#[serde(default)]` to all structs so unknown/missing fields from the SyncThing API
don't panic during deserialization. This is especially important for `ItemFinishedEvent`
and `ItemFinishedData`.

**1.5 — Add doc comments**  
Files: `src/cli.rs`, `src/client.rs`, `src/types.rs`  
Add `///` doc comments to all public structs, their fields, and all `impl` methods. Follow
rustdoc conventions: first line is a short summary, then an optional blank line and longer
description.

---

### Phase 2 — Logging

**Goal:** Replace ad-hoc `println!` with structured, levelled, filterable logging.  
**Rust concepts covered:** `tracing`, `EnvFilter`, spans, async-aware instrumentation.

#### Tasks

**2.1 — Add dependencies**  
File: `Cargo.toml`  
Add:
```toml
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

**2.2 — Initialize subscriber in main**  
File: `src/main.rs`  
Add this near the top of `main()`, before anything else:

```rust
tracing_subscriber::fmt()
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    .init();
```

This lets users control verbosity at runtime: `RUST_LOG=debug tidysync watch`.

**2.3 — Replace println! with tracing macros**  
Files: `src/main.rs`, `src/client.rs`

Mapping guide:
- Status/progress output that users always see → `tracing::info!`
- Detailed per-event output in the watch loop → `tracing::debug!`
- Errors / unexpected states → `tracing::error!` or `tracing::warn!`

For the watch loop, wrap the whole function body in a span:
```rust
let _span = tracing::info_span!("watch_item_finished").entered();
```

---

### Phase 3 — Config File

**Goal:** Let users define source→destination folder rules in a TOML config file.  
**Rust concepts covered:** `serde` for deserialization, `dirs` crate for platform paths, validation, newtype pattern.

#### Tasks

**3.1 — Add dependencies**  
File: `Cargo.toml`  
Add:
```toml
toml = "0.8"
dirs = "5"
```

**3.2 — Create `src/config.rs`**  
Define the config structure. A rule maps a SyncThing folder (by its folder ID) plus an
optional glob pattern to a destination directory:

```rust
#[derive(Deserialize, Debug)]
pub struct Config {
    pub rules: Vec<Rule>,
}

#[derive(Deserialize, Debug)]
pub struct Rule {
    /// The SyncThing folder ID this rule applies to (e.g. "photos-eu")
    pub folder_id: String,
    /// Optional glob pattern relative to folder root. If omitted, matches all files.
    pub pattern: Option<String>,
    /// Absolute path to the destination directory
    pub destination: PathBuf,
}
```

Example `~/.config/tidysync/config.toml`:
```toml
[[rules]]
folder_id = "photos-eu"
pattern = "*.jpg"
destination = "/home/user/sorted/photos"

[[rules]]
folder_id = "documents"
destination = "/home/user/archive"
```

**3.3 — Implement `Config::load`**  
In `src/config.rs`, implement:
```rust
impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self>
}
```
- If `path` is `None`, default to `dirs::config_dir().unwrap() / "tidysync" / "config.toml"`
- Read the file with `std::fs::read_to_string`, parse with `toml::from_str`
- Return a clear error if the file is missing or malformed

**3.4 — Validate config at startup**  
In `Config::load` or a separate `Config::validate`, check:
- Each `destination` path exists and is a directory (not a file)
- Each `destination` is writable (try creating a temp file)
- No duplicate `folder_id` + `pattern` combos

Emit `tracing::warn!` for non-fatal issues (e.g., destination doesn't exist yet but can be
created). Return `Err` for fatal issues.

**3.5 — Wire config into CLI**  
File: `src/cli.rs`  
Add a global `--config` flag to `CLI`:
```rust
#[arg(long, value_name = "FILE")]
pub config: Option<PathBuf>,
```

File: `src/main.rs`  
Load config early in `main()` and pass it through to the watch command.

**3.6 — Add `config` subcommand**  
File: `src/cli.rs`  
Add `Config` to the `Command` enum.  
File: `src/main.rs`  
Handle it by loading and pretty-printing the resolved config:
```rust
cli::Command::Config => {
    let config = Config::load(args.config.as_deref())?;
    println!("{:#?}", config);
}
```
This is invaluable for debugging.

---

### Phase 4 — File Moving

**Goal:** Implement the actual core feature: move files based on config rules.  
**Rust concepts covered:** `std::fs`, cross-device moves, glob matching, error isolation.

#### Tasks

**4.1 — Add glob dependency**  
File: `Cargo.toml`  
Add:
```toml
glob = "0.3"
```

**4.2 — Rule matching**  
File: `src/client.rs` (or a new `src/mover.rs` module)  
When an `ItemFinished` event arrives with no error:
1. Find all rules where `rule.folder_id == event.data.folder`
2. For each matching rule, check the optional glob pattern against `event.data.item`
3. Collect all rules that match

```rust
fn matching_rules<'a>(config: &'a Config, event: &ItemFinishedData) -> Vec<&'a Rule> {
    config.rules.iter().filter(|rule| {
        rule.folder_id == event.folder && matches_pattern(rule, &event.item)
    }).collect()
}
```

**4.3 — Resolve full source path**  
The `ItemFinishedData.item` is a path relative to the SyncThing folder root.  
To get the absolute source path, look up the folder root from the list of SyncThing folders
(already fetched via `client.folders()`), then join:

```rust
let folder_root = folders.iter().find(|f| f.id == event.folder)
    .ok_or_else(|| anyhow!("Unknown folder: {}", event.folder))?;
let source = PathBuf::from(&folder_root.path).join(&event.item);
```

Fetch `folders` once at watch startup and cache them for the loop's lifetime.

**4.4 — Implement the move**  
Try `std::fs::rename` first (fast, atomic on same device).  
If it fails with a cross-device error, fall back to `std::fs::copy` + `std::fs::remove_file`.

```rust
fn move_file(src: &Path, dst: &Path) -> Result<()> {
    // Ensure destination directory exists
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(/* EXDEV */ 18) => {
            std::fs::copy(src, dst).context("cross-device copy failed")?;
            std::fs::remove_file(src).context("failed to delete source after copy")?;
            Ok(())
        }
        Err(e) => Err(e).context("rename failed"),
    }
}
```

**4.5 — Add `--dry-run` flag**  
File: `src/cli.rs`  
Add to the `Watch` variant:
```rust
Watch {
    #[arg(long)]
    dry_run: bool,
}
```
File: `src/client.rs` / mover  
When `dry_run` is true, log with `tracing::info!("Would move {} → {}", src, dst)` and skip
the actual file operation.

**4.6 — Error isolation in the watch loop**  
A failed move for one file should never stop the watch loop. Wrap each move attempt:
```rust
if let Err(e) = move_file(&source, &destination) {
    tracing::error!("Failed to move {}: {:#}", source.display(), e);
    // continue to next event
}
```

Also add a sleep + backoff if the HTTP poll itself fails, so a temporary SyncThing outage
doesn't spam errors:
```rust
tokio::time::sleep(Duration::from_secs(5)).await;
```

---

### Phase 5 — Graceful Shutdown (bonus)

**Goal:** Handle Ctrl+C cleanly so in-progress operations complete before exit.  
**Rust concepts covered:** `tokio::signal`, `select!`, cancellation.

Add to `Cargo.toml`: `tokio` feature `signal`.

In the watch loop, use `tokio::select!` to race between the next poll and a shutdown signal:
```rust
tokio::select! {
    result = poll_events() => { /* handle */ }
    _ = tokio::signal::ctrl_c() => {
        tracing::info!("Shutting down...");
        break;
    }
}
```

---

### Phase 6 — Tests (bonus)

**Goal:** Build confidence in the rule-matching and file-moving logic.  
**Rust concepts covered:** `#[cfg(test)]`, `tempfile` crate, test organization.

Add `tempfile = "3"` to `[dev-dependencies]`.

Tests to write:
- `config::tests::parse_valid_config` — parse a known-good TOML string
- `config::tests::parse_missing_destination` — expect a validation error
- `mover::tests::rule_matches_folder_id` — verify matching by folder ID
- `mover::tests::rule_matches_glob_pattern` — verify glob filtering
- `mover::tests::rule_ignores_non_matching` — verify non-match returns empty list
- `mover::tests::move_file_same_device` — move within a tempdir
- `mover::tests::move_file_preserves_content` — verify bytes are identical after move

---

## Dependency plan (Cargo.toml additions)

| Phase | Crate | Why |
|---|---|---|
| 2 | `tracing` | Structured logging |
| 2 | `tracing-subscriber` (env-filter) | Log output + runtime filtering |
| 3 | `toml` | Config file parsing |
| 3 | `dirs` | Platform-specific config directory |
| 4 | `glob` | Pattern matching on file paths |
| 6 (dev) | `tempfile` | Temporary directories in tests |

---

## File layout after all phases

```
src/
  main.rs        # entry point, subscriber init, command routing
  cli.rs         # clap structs: CLI, Command (+ Watch { dry_run }, Config)
  client.rs      # SyncThingClient HTTP methods
  types.rs       # API response structs + ItemAction / ItemType enums
  config.rs      # Config, Rule, Config::load, Config::validate
  mover.rs       # move_file, matching_rules (introduced in phase 4)
```
