# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

`tidysync` is a Rust async CLI that polls the Syncthing REST API for `ItemFinished` events and moves synced files to a configured destination directory. It can run either as a foreground CLI or as a system tray application. It is intentionally a Rust learning project.

Requires Rust 1.89+ (`std::fs::File::try_lock`, used for the single-instance lock).

## Commands

```sh
cargo build
cargo check
cargo clippy --all-targets
cargo test

# CLI subcommands
cargo run -- ping
cargo run -- watch
cargo run -- --config /path/to/config.toml watch

# Tray mode (needs an existing config; will not run the wizard)
cargo run -- --tray

# Verbose output
RUST_LOG=debug cargo run -- watch
RUST_LOG=tidysync=debug cargo run -- watch   # ours only
```

Default Syncthing URL is `http://localhost:8384`.

`clippy --all-targets` currently reports 8 pre-existing warnings (an unused import in `cli.rs` and `config.rs`, the `CLI` acronym name, and dead-code fields in `types.rs`). Treat any *new* warning as something to fix.

## API key resolution (`apikey.rs`)

Tried in order, first hit wins:

1. `--api-key`
2. `SYNCTHING_API_KEY` in the environment
3. `SYNCTHING_API_KEY` in `~/.env` (read with `dotenvy`'s iterator form, which deliberately does **not** import the file's other variables into the process)
4. the `<apikey>` element in Syncthing's own config — `$XDG_STATE_HOME/syncthing/config.xml`, falling back to `$XDG_CONFIG_HOME/syncthing/config.xml`

Steps 3 and 4 exist so tray mode works when launched from a desktop entry, where there is no shell environment.

The `--api-key` arg sets `hide_env_values = true`; without it `--help` prints the key itself whenever the env var is set.

## Architecture

CLI watch flow: `main.rs` resolves the API key → takes the single-instance lock → loads config → spawns `WatcherHandle` → waits on `ctrl_c`.

Tray flow: same, plus a tray thread running a GTK loop that drives the watcher through a `WatcherControl`.

| File | Role |
|---|---|
| `src/main.rs` | Entry point; parses CLI, routes subcommands, orchestrates both lifecycles |
| `src/cli.rs` | `clap` derive structs — `CLI` + `Command` enum (Ping, Status, Folders, Devices, Watch, Config) |
| `src/client.rs` | `SyncThingClient` — all async HTTP calls, long-poll event loop |
| `src/types.rs` | Serde structs/enums for every API response shape |
| `src/config.rs` | `Config::load` — reads TOML or launches interactive first-run wizard; `exists`, `validate` |
| `src/mover.rs` | `move_file()` (rename with cross-device copy+delete fallback) and `move_existing_files()` |
| `src/watcher.rs` | `WatcherHandle`/`WatcherControl`/`Status` — supervises watch sessions, owns the on/off switch |
| `src/tray.rs` | Tray icon, menu, GTK loop, procedurally drawn icon |
| `src/logging.rs` | `tracing` setup for both modes; `RecentLog` ring buffer behind the tray menu |
| `src/apikey.rs` | API key resolution chain |
| `src/lockfile.rs` | `WatchLock` — single-instance guard |

## Critical architectural details

### Event polling cursor (`client.rs`)
`watch_events` seeds its `since` cursor via `latest_event_id()`, which must use **the exact same `events=` filter string** as the main poll loop. Event IDs in Syncthing are per-filter — mixing filter strings gives IDs from a different sequence, causing missed events or indefinite blocking. Both call sites read the `EVENT_FILTER` constant; keep it that way rather than inlining the string again.

### `EventData` untagged enum (`types.rs`)
`EventData` uses `#[serde(untagged)]`; **variant order is load-bearing**. Serde tries each variant top-to-bottom and picks the first match. The discriminating fields are:
- `DeviceConnected` — requires `"addr"`
- `DeviceDisconnected` — requires `"error"` + `"id"`
- `DevicePauseOrResume` — requires `"device"`
- `ItemFinished` — requires `"folder"` + `"item"`
- `Other(serde_json::Value)` — catch-all fallback

Reordering variants will silently mis-parse events.

### Cancellation is only observed between event batches (`client.rs`)
The `tokio::select!` in the poll loop wraps the long poll and the backoff sleep — both safe to abandon. Once a batch of events starts being processed it runs to completion. **This is what guarantees switching the watcher off can never interrupt a file mid-move.** The pre-scan takes a `should_continue` predicate checked between files for the same reason. Do not widen the `select!` to cover event processing.

### Watcher supervision (`watcher.rs`)
A single `watch::Sender<Signal>` carries `Running`/`Paused`/`Shutdown`. Shutdown is an explicit signal rather than "everyone dropped the sender", because the tray thread holds a `WatcherControl` clone and drop ordering would otherwise decide whether stopping works.

Toggling off cancels the session; toggling on starts a **fresh** one, which re-runs the pre-scan and so sweeps up everything Syncthing delivered while paused. That is the intended behaviour, not an accident — there is no separate catch-up path.

`Status` carries a `generation` counter, bumped on every change, so the tray can skip redundant menu redraws.

### Tray threading (`tray.rs`)
GTK needs an event loop, and the tray must be built on whichever thread runs it — but **not necessarily the main thread**. So `main` stays a normal `#[tokio::main]` async fn and the tray gets its own `std::thread`. A `glib::timeout_add_local` every 200 ms drains `MenuEvent::receiver()` and refreshes labels.

`tray-icon` is depended on with `default-features = false, features = ["gtk"]` — the default `libxdo` feature only powers predefined Copy/Cut/Paste items, which we don't use, and requires `xdotool` to be installed. `glib` is used via `gtk::glib` rather than as its own dependency so the versions cannot drift. `muda` comes through `tray_icon::menu`.

muda has no `set_visible`, so the "Recent" rows are inserted as the ring buffer fills rather than padded with blanks, and removed again if the count drops (which only "Clear log file" causes).

"Clear log file" **truncates**, never deletes. `tracing-appender` holds the file open, so unlinking it would leave the writer appending to an unreachable inode and every later line would vanish until restart. It opens with `O_APPEND`, so writes resume at offset 0 after truncation — verified, no sparse hole.

### Config first-run wizard (`config.rs`)
`Config::load` prompts interactively on stdin when the file is missing. Tray mode has no terminal, so it calls `Config::exists` first and bails with a message rather than hanging. Config is stored at `~/.config/tidysync/config.toml` on Linux. Schema is flat — single `source_folder_id` + `target_directory`.

### Error handling convention
Every `?` at a network or deserialization boundary must be followed by `.context("descriptive message")` using `anyhow::Context`. Bare `?` without context is a code smell here.

Inside the watch loop the convention is different: a single bad file, unknown folder, or transport error must **log and continue**, never propagate. Only a failure to start a session at all returns `Err`, which the supervisor retries.

### API structs resilience
All Syncthing API response structs use `#[serde(default)]` + `#[derive(Default)]` to tolerate unknown fields. New structs must follow this pattern.

### Logging conventions (`logging.rs`)
Info is user-visible output; debug is per-event detail. `move_file` logs `Moved <filename>` at info and the full paths at debug — the info line is what appears in the tray menu, where a pair of absolute paths would be truncated into uselessness.

The HTTP stack is pinned at info even under `RUST_LOG=debug`, because hyper's connection-pool logging buries everything else.

`println!` is still correct for terminal output that is not logging: the config wizard's prompts and the query results of `ping`/`status`/`folders`/`devices`/`config`.

## State on disk

| Path | Purpose |
|---|---|
| `~/.config/tidysync/config.toml` | Config |
| `~/.local/state/tidysync/tidysync.lock` | Single-instance lock (holder's pid written inside) |
| `~/.local/state/tidysync/tidysync.log` | Tray-mode log; single file, currently never rotated |

## Planned future work

- **Multi-rule config**: Migrate config schema from flat `source_folder_id`/`target_directory` to `Vec<Rule>` where each rule has `folder_id`, optional `glob` pattern, and `destination`. Add `glob = "0.3"` for pattern matching.
- **`--dry-run` flag**: Add to the `Watch` subcommand; log intended moves without executing them.
- **Log rotation**: `tracing-appender` supports daily rotation with `max_log_files`, but the filename then carries a date, so "Open log file" would need to resolve the newest file.
- **Autostart**: a `.desktop` entry so tray mode starts with the session.
- **More tests**: config parsing and the event-dispatch logic in `watch_events` are the notable untested areas.
