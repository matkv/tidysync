use std::{
    fs,
    io::{self, Write},
    path::{self, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::client::SyncThingClient;

#[derive(Deserialize, Serialize, Debug)]
pub struct Config {
    pub source_folder_id: String,
    pub target_directory: PathBuf,
}

impl Config {
    pub fn default_path() -> Result<PathBuf> {
        let dir = dirs::config_dir().context("could not find config directory")?;
        Ok(dir.join("tidysync").join("config.toml"))
    }

    pub async fn load(path: Option<&Path>, client: &SyncThingClient) -> Result<Self> {
        let config_path = match path {
            Some(p) => p.to_path_buf(),
            None => Self::default_path()?,
        };

        if !config_path.exists() {
            return Self::create_new_config_file(&config_path, client).await;
        }

        let contents = fs::read_to_string(&config_path).context("failed to read config file")?;
        toml::from_str(&contents).context("failed to parse config file") // it is inferred that the type is Config here
    }

    async fn create_new_config_file(
        config_path: &PathBuf,
        client: &SyncThingClient,
    ) -> Result<Self> {
        println!(
            "Config file not found at {}. Creating a new one.",
            config_path.display()
        );

        let folders = client.folders().await?;
        if folders.is_empty() {
            bail!(
                "No folders found in Syncthing. Please create a folder in Syncthing before running tidysync."
            );
        }

        println!();
        println!("Available folders:");

        for folder in &folders {
            println!(
                "ID: {}, Label: {}, Path: {}",
                folder.id, folder.label, folder.path
            );
        }

        print!("\nEnter the ID of the source folder to watch: ");
        io::stdout().flush().context("failed to flush stdout")?;

        let mut folder_id = String::new();
        io::stdin()
            .read_line(&mut folder_id)
            .context("failed to read folder ID from stdin")?;

        if !folders.iter().any(|f| f.id == folder_id.trim()) {
            bail!(
                "Folder ID '{}' not found in Syncthing folders",
                folder_id.trim()
            );
        }

        print!("Enter the target directory to move completed files to: ");
        io::stdout().flush().context("failed to flush stdout")?;

        let mut target = String::new();
        io::stdin()
            .read_line(&mut target)
            .context("failed to read target directory from stdin")?;

        let config = Config {
            source_folder_id: folder_id,
            target_directory: PathBuf::from(target.trim()),
        };

        // check if config directory exists (~/.config/tidysync), if not create it
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).context("failed to create config directory")?;
        }

        // serialize config to TOML and write to file
        let toml_str =
            toml::to_string_pretty(&config).context("failed to serialize config to TOML")?;
        fs::write(config_path, toml_str).context("failed to write config file");

        println!("Config file created at {}", config_path.display());
        Ok(config)
    }
}
