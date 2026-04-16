# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

`tidysync` is a Rust async CLI that polls the Syncthing REST API for `ItemFinished` events and moves synced files to a configured destination directory. It is intentionally a Rust learning project. `PLAN.md` is the authoritative development roadmap and must be read before making significant changes.

## Commands

```sh
cargo build
cargo check
cargo clippy

# Run (API key required for every subcommand)
SYNCTHING_API_KEY=<key> cargo run -- ping
SYNCTHING_API_KEY=<key> cargo run -- watch
SYNCTHING_API_KEY=<key> cargo run -- --config /path/to/config.toml watch
```

The `--api-key` flag or `SYNCTHING_API_KEY` env var is required for every subcommand. Default Syncthing URL is `http://localhost:8384`. There are no automated tests yet (Phase 6 of PLAN.md).

## Architecture

The watch flow: `main.rs` loads config → creates `SyncThingClient` → calls `move_existing_files()` to pre-scan the source folder → enters the `watch_events()` long-poll loop → on each `ItemFinished` event, calls `move_file()`.

| File | Role |
|---|---|
| `src/main.rs` | Entry point; parses CLI, routes subcommands, orchestrates watch lifecycle |
| `src/cli.rs` | `clap` derive structs — `CLI` + `Command` enum (Ping, Status, Folders, Devices, Watch, Config) |
| `src/client.rs` | `SyncThingClient` — all async HTTP calls, long-poll event loop |
| `src/types.rs` | Serde structs/enums for every API response shape |
| `src/config.rs` | `Config::load` — reads TOML or launches interactive first-run wizard |
| `src/mover.rs` | `move_file()` (rename with cross-device copy+delete fallback) and `move_existing_files()` |

## Critical architectural details

### Event polling cursor (`client.rs`)
`watch_events` seeds its `since` cursor via `latest_event_id()`, which uses **the exact same `events=` filter string** as the main poll loop. Event IDs in Syncthing are per-filter — mixing filter strings gives IDs from a different sequence, causing missed events or indefinite blocking. Never change one filter string without changing the other.

### `EventData` untagged enum (`types.rs`)
`EventData` uses `#[serde(untagged)]`; **variant order is load-bearing**. Serde tries each variant top-to-bottom and picks the first match. The discriminating fields are:
- `DeviceConnected` — requires `"addr"`
- `DeviceDisconnected` — requires `"error"` + `"id"`
- `DevicePauseOrResume` — requires `"device"`
- `ItemFinished` — requires `"folder"` + `"item"`
- `Other(serde_json::Value)` — catch-all fallback

Reordering variants will silently mis-parse events.

### Config first-run wizard (`config.rs`)
`Config::load` checks whether the config file exists. If not, it calls `create_new_config_file`, which fetches folders from Syncthing and prompts the user interactively on stdin. Config is stored at `~/.config/tidysync/config.toml` on Linux. Current schema is flat — single `source_folder_id` + `target_directory`. The multi-rule schema in PLAN.md Phase 3 is not yet implemented.

### Error handling convention
Every `?` at a network or deserialization boundary must be followed by `.context("descriptive message")` using `anyhow::Context`. Bare `?` without context is a code smell here.

### API structs resilience
All Syncthing API response structs use `#[serde(default)]` + `#[derive(Default)]` to tolerate unknown fields. New structs must follow this pattern.

## Planned future work

- **Logging**: Replace `println!` with `tracing` + `tracing-subscriber` (env-filter). Initialize with `EnvFilter::from_default_env()` so users can do `RUST_LOG=debug tidysync watch`. Info for user-visible output, debug for per-event detail.
- **Multi-rule config**: Migrate config schema from flat `source_folder_id`/`target_directory` to `Vec<Rule>` where each rule has `folder_id`, optional `glob` pattern, and `destination`. Add `glob = "0.3"` for pattern matching.
- **`--dry-run` flag**: Add to the `Watch` subcommand; log intended moves without executing them.
- **Graceful shutdown**: Use `tokio::signal::ctrl_c()` with `tokio::select!` in the watch loop.
- **Tests**: Add `tempfile` to dev-dependencies. Priority: config parsing, rule matching logic, and `move_file` same-device/cross-device cases.
