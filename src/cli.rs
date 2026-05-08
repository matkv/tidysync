use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tidysync",
    version,
    about = "A CLI for cleaning up files synced with Syncthing"
)]

pub struct CLI {
    #[arg(short, long, default_value = "http://localhost:8384")]
    pub url: String,
    #[arg(long, env = "SYNCTHING_API_KEY")]
    pub api_key: Option<String>,
    #[arg(long, value_name = "CONFIG_PATH")]
    pub config: Option<PathBuf>,
    #[arg(long, help = "Run in system tray mode")]
    pub tray: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    Ping,
    Status,
    Folders,
    Devices,
    Watch,
    Config,
}
