use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};

use crate::{cli::CLI, client::SyncThingClient, config::Config};

mod apikey;
mod cli;
mod client;
mod config;
mod lockfile;
mod logging;
mod mover;
mod tray;
mod types;
mod watcher;

/// Run headless with a tray icon instead of a terminal.
async fn run_tray(
    syncthing: SyncThingClient,
    config_path: Option<&std::path::Path>,
    logging: logging::TrayLogging,
) -> Result<()> {
    let _lock = lockfile::WatchLock::acquire()?;

    // There is no terminal to prompt on, so refuse rather than hanging on stdin
    // waiting for answers nobody can give.
    if !Config::exists(config_path)? {
        anyhow::bail!("no config yet — run `tidysync config` once before using tray mode");
    }

    let config = Config::load(config_path, &syncthing).await?;
    config.validate()?;

    let url = syncthing.base_url.clone();
    let watcher = watcher::WatcherHandle::spawn(Arc::new(syncthing), Arc::new(config), true);

    let (quit_tx, mut quit_rx) = tokio::sync::mpsc::unbounded_channel();
    let tray_thread = tray::spawn(watcher.control(), url, logging, quit_tx);

    tokio::select! {
        _ = quit_rx.recv() => tracing::info!("Quit from tray"),
        result = tokio::signal::ctrl_c() => {
            result.context("failed to listen for ctrl-c")?;
            tracing::info!("Interrupted");
        }
    }

    watcher.shutdown().await;
    let _ = tray_thread.join();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = CLI::parse();

    // Tray mode adds the menu buffer and a log file on top of stdout, and a
    // subscriber can only be installed once, so the mode has to be known first.
    let tray_logging = if args.tray {
        Some(logging::init_tray()?)
    } else {
        logging::init();
        None
    };

    let api_key = apikey::resolve(args.api_key);
    let syncthing = SyncThingClient::new(args.url, api_key);

    if let Some(logging) = tray_logging {
        return run_tray(syncthing, args.config.as_deref(), logging).await;
    }

    let Some(command) = &args.command else {
        CLI::command().print_help()?;
        println!();
        return Ok(());
    };

    match command {
        cli::Command::Ping => {
            syncthing.ping().await?;
            println!("Syncthing is responsive!");
        }
        cli::Command::Status => {
            let status = syncthing.system_status().await?;
            println!("Device ID: {}", status.my_id);
            println!("Uptime: {} seconds", status.uptime);
        }
        cli::Command::Folders => {
            let folders = syncthing.folders().await?;
            for folder in folders {
                println!(
                    "ID: {}, Label: {}, Path: {}",
                    folder.id, folder.label, folder.path
                );
            }
        }
        cli::Command::Devices => {
            let devices = syncthing.devices().await?;
            for device in devices {
                println!("ID: {}, Name: {}", device.device_id, device.name);
            }
        }
        cli::Command::Watch => {
            // Held for the rest of the watch: a second watcher would race this
            // one for every file. Taken before the config wizard so two fresh
            // instances cannot both prompt on stdin.
            let _lock = lockfile::WatchLock::acquire()?;

            let config = Config::load(args.config.as_deref(), &syncthing).await?;
            config.validate()?;

            tracing::info!(
                "Watching for changes in folder ID: {}, moving files to: {}",
                config.source_folder_id,
                config.target_directory.display()
            );

            let watcher =
                watcher::WatcherHandle::spawn(Arc::new(syncthing), Arc::new(config), true);

            tokio::signal::ctrl_c()
                .await
                .context("failed to listen for ctrl-c")?;

            tracing::info!("Shutting down...");
            let moved = watcher.status().moved;
            watcher.shutdown().await;
            tracing::info!("Moved {moved} file(s) this session");
        }
        cli::Command::Config => {
            let config = Config::load(args.config.as_deref(), &syncthing).await?; // TODO check what as_deref does
            println!("Current config:");
            println!("Source folder ID: {}", config.source_folder_id);
            println!("Target directory: {}", config.target_directory.display());
        }
    }
    Ok(())
}
