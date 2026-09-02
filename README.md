# tidysync

Automatically clean up & move files after Syncthing finishes syncing them, using the
[Syncthing REST API](https://docs.syncthing.net/dev/rest.html). A project for learning Rust.

Runs either as a foreground CLI or as a system tray app.

## Setup

Needs Rust 1.89+ and a running Syncthing.

```sh
cargo build --release
```

Run the setup wizard once — it lists your Syncthing folders and asks which one to watch
and where moved files should go:

```sh
cargo run -- config
```

The API key is found automatically, in this order: `--api-key`, `SYNCTHING_API_KEY` in the
environment, `SYNCTHING_API_KEY` in `~/.env`, and finally the key in Syncthing's own
`config.xml`. On a normal desktop install the last one means there is nothing to configure.

## Usage

```sh
tidysync watch     # foreground, Ctrl-C to stop
tidysync --tray    # system tray icon
```

The tray menu shows the current state and how many files have been moved, a checkbox to
pause and resume watching, the last few log lines, and shortcuts to open the Syncthing web
UI and the log file.

Pausing stops files being moved. Resuming re-scans the source folder, so anything Syncthing
delivered while you were paused gets swept up then.

Other subcommands: `ping`, `status`, `folders`, `devices`.

Set `RUST_LOG=debug` for more detail, or `RUST_LOG=tidysync=debug` for just this program.

## Files

- `~/.config/tidysync/config.toml` — configuration
- `~/.local/state/tidysync/tidysync.log` — log file, written in tray mode
- `~/.local/state/tidysync/tidysync.lock` — stops two copies running at once

Only one watcher may run at a time, so starting the tray while `tidysync watch` is running
will refuse with the other process's pid.
