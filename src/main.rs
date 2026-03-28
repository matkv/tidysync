use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};

use crate::{cli::CLI, client::SyncThingClient, config::Config};

mod cli;
mod client;
mod config;
mod mover;
mod types;

#[tokio::main]
async fn main() -> Result<()> {
    let args = CLI::parse();
    let syncthing = SyncThingClient::new(args.url, args.api_key);

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
            let config = Config::load(args.config.as_deref(), &syncthing).await?;

            std::fs::create_dir_all(&config.target_directory)
                .context("failed to create target directory if it doesn't exist")?;

            println!(
                "Watching for changes in folder ID: {}",
                config.source_folder_id
            );

            syncthing
                .watch_events(&config.source_folder_id, &config.target_directory)
                .await?;
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
