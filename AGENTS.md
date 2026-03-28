# AGENTS.md — tidysync

Rust async CLI that polls the [Syncthing REST API](https://docs.syncthing.net/dev/rest.html) for `ItemFinished` events and moves synced files to a configured destination. Intentionally a Rust learning project; `PLAN.md` is the authoritative development roadmap and must be read before making significant changes.

## Source layout

| File | Role |
|---|---|
| `src/main.rs` | Entry point; parses CLI, routes subcommands |
| `src/cli.rs` | `clap` derive structs — `CLI` + `Command` enum |
| `src/client.rs` | `SyncThingClient` — all async HTTP calls, long-poll event loop |
| `src/types.rs` | Serde structs/enums for every API response shape |
| `src/config.rs` | `Config::load` — reads TOML or launches interactive first-run wizard |

## Essential commands

```sh
# Build
cargo build

# Run (API key required)
SYNCTHING_API_KEY=<key> cargo run -- ping
SYNCTHING_API_KEY=<key> cargo run -- watch
SYNCTHING_API_KEY=<key> cargo run -- --config /path/to/config.toml watch

# Check for errors without building a binary
cargo check

# Lint
cargo clippy
```

The `--api-key` flag or `SYNCTHING_API_KEY` env var is **required** for every subcommand. Default Syncthing URL is `http://localhost:8384`.

## Critical architectural details

### Event polling cursor (`client.rs`)
`watch_events` seeds its `since` cursor via `latest_event_id()`, which uses **the exact same `events=` filter string** as the main loop. The event IDs are per-filter in Syncthing — mixing filters gives IDs from a different sequence and causes events to be missed or the poll to block indefinitely. Never change one filter string without changing the other.

### `EventData` untagged enum (`types.rs`)
`EventData` uses `#[serde(untagged)]`; **variant order is load-bearing**. Serde tries each variant top-to-bottom and picks the first that succeeds. The discriminating fields are:
- `DeviceConnected` — requires `"addr"`
- `DeviceDisconnected` — requires `"error"` + `"id"`
- `DevicePauseOrResume` — requires `"device"`
- `ItemFinished` — requires `"folder"` + `"item"`
- `Other(serde_json::Value)` — catch-all fallback

Reordering variants will silently mis-parse events.

### Config first-run wizard (`config.rs`)
`Config::load` checks whether the config file exists. If not, it calls `create_new_config_file`, which fetches folders from Syncthing and prompts the user interactively on stdin. This is intentional UX — don't remove the existence check.

Config is stored at `dirs::config_dir()/tidysync/config.toml` (e.g. `~/.config/tidysync/config.toml` on Linux). Current schema is flat — single `source_folder_id` + `target_directory`. The multi-rule schema in `PLAN.md` Phase 3 is **not yet implemented**.

### Error handling convention
Every `?` at a network or deserialization boundary must be followed by `.context("descriptive message")` using `anyhow::Context`. Bare `?` without context is a code smell here.

### API structs resilience
All Syncthing API response structs use `#[serde(default)]` + `#[derive(Default)]` to tolerate unknown fields from future API versions. New structs must follow this pattern.

## Current state vs. PLAN.md

`watch_events` detects `ItemFinished` events and logs the intended move target but **does not yet move files** (Phase 4 of `PLAN.md` is pending). Output still uses `println!` rather than `tracing` (Phase 2 pending). When implementing either phase, follow the exact patterns described in `PLAN.md`.

