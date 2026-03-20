use anyhow::Result;
use clap::Parser;

use crate::{cli::CLI, client::SyncThingClient};

mod cli;
mod client;

#[tokio::main]
async fn main() -> Result<()> {
    let args = CLI::parse();
    let syncthing = SyncThingClient::new(args.url, args.api_key);

    match args.command {
        cli::Command::Ping => {
            syncthing.ping().await?;
            println!("Syncthing is responsive!");
        }
    }
    Ok(())
}
