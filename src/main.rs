use anyhow::Result;
use clap::Parser;

use crate::{cli::CLI, client::SyncThingClient};

mod cli;
mod client;
mod types;

#[tokio::main]
async fn main() -> Result<()> {
    let args = CLI::parse();
    let syncthing = SyncThingClient::new(args.url, args.api_key);

    match args.command {
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
    }
    Ok(())
}
