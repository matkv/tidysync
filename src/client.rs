use crate::{
    mover,
    types::{Device, Folder, SyncThingEvent, SystemStatus},
};
use anyhow::{Context, Result};
use chrono::Local;
use reqwest::Client;

pub struct SyncThingClient {
    pub base_url: String,
    pub api_key: Option<String>,
    client: Client,
}

impl SyncThingClient {
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        Self {
            base_url,
            api_key,
            client: Client::new(),
        }
    }

    fn require_api_key(&self) -> Result<&str> {
        self.api_key
            .as_deref()
            .context("API key is required — set SYNCTHING_API_KEY or pass --api-key")
    }

    pub async fn ping(&self) -> Result<()> {
        let url = format!("{}/rest/system/ping", self.base_url);

        self.client
            .get(&url)
            .header("X-API-Key", self.require_api_key()?)
            .send()
            .await
            .context("failed to reach /rest/system/ping")?
            .error_for_status()
            .context("Syncthing returned an error on ping")?;

        Ok(())
    }

    pub async fn system_status(&self) -> Result<SystemStatus> {
        let url = format!("{}/rest/system/status", self.base_url);
        let status = self
            .client
            .get(&url)
            .header("X-API-Key", self.require_api_key()?)
            .send()
            .await
            .context("failed to reach /rest/system/status")?
            .error_for_status()
            .context("Syncthing returned an error on system/status")?
            .json::<SystemStatus>()
            .await
            .context("failed to parse SystemStatus response")?;
        Ok(status)
    }

    pub async fn folders(&self) -> Result<Vec<Folder>> {
        let url = format!("{}/rest/config/folders", self.base_url);
        let folders = self
            .client
            .get(&url)
            .header("X-API-Key", self.require_api_key()?)
            .send()
            .await
            .context("failed to reach /rest/config/folders")?
            .error_for_status()
            .context("Syncthing returned an error on config/folders")?
            .json::<Vec<Folder>>()
            .await
            .context("failed to parse folders list")?;
        Ok(folders)
    }

    pub async fn devices(&self) -> Result<Vec<Device>> {
        let url = format!("{}/rest/config/devices", self.base_url);
        let devices = self
            .client
            .get(&url)
            .header("X-API-Key", self.require_api_key()?)
            .send()
            .await
            .context("failed to reach /rest/config/devices")?
            .error_for_status()
            .context("Syncthing returned an error on config/devices")?
            .json::<Vec<Device>>()
            .await
            .context("failed to parse devices list")?;
        Ok(devices)
    }

    async fn latest_event_id(&self) -> Result<u64> {
        // Fetch the most recent event using the same filter as the main loop.
        // The event IDs are per-filter, so mixing filters would give us an ID
        // from a different sequence and cause the main loop to miss events or block.
        let url = format!(
            "{}/rest/events?since=0&limit=1&events=ItemFinished,DeviceConnected,DeviceDisconnected,DevicePaused,DeviceResumed",
            self.base_url
        );
        let events = self
            .client
            .get(&url)
            .header("X-API-Key", self.require_api_key()?)
            .send()
            .await
            .context("failed to reach /rest/events for seeding")?
            .error_for_status()
            .context("Syncthing returned an error while seeding event cursor")?
            .json::<Vec<SyncThingEvent>>()
            .await
            .context("failed to parse seed events")?;
        Ok(events.last().map(|e| e.id).unwrap_or(0))
    }

    pub async fn watch_events(
        &self,
        source_folder_id: &str,
        target_directory: &std::path::Path,
    ) -> Result<()> {
        let devices = self
            .devices()
            .await
            .context("failed to fetch device list")?;

        let folders = self
            .folders()
            .await
            .context("failed to fetch folder list")?;

        let device_names: std::collections::HashMap<String, String> =
            devices.into_iter().map(|d| (d.device_id, d.name)).collect();

        let device_label = |id: &str| -> String {
            match device_names.get(id) {
                Some(name) if !name.is_empty() => name.clone(),
                _ => id.to_string(),
            }
        };

        // Pre-scan: move files that already exist in the source folder before
        // we start watching for new events.
        let source_folder = folders
            .iter()
            .find(|f| f.id == source_folder_id)
            .with_context(|| {
                format!(
                    "source folder '{}' not found in Syncthing config",
                    source_folder_id
                )
            })?;

        let expanded_root = if let Some(rest) = source_folder.path.strip_prefix("~/") {
            let home = dirs::home_dir().context("could not determine home directory")?;
            home.join(rest)
        } else {
            std::path::PathBuf::from(&source_folder.path)
        };

        println!(
            "Scanning for existing files in {}...",
            expanded_root.display()
        );
        mover::move_existing_files(&expanded_root, target_directory)
            .await
            .context("pre-scan move failed")?;
        println!("[{}] Pre-scan complete. Watching for new events...", Local::now().format("%H:%M:%S"));

        let mut since: u64 = self.latest_event_id().await?;

        loop {
            let url = format!(
                "{}/rest/events?events=ItemFinished,DeviceConnected,DeviceDisconnected,DevicePaused,DeviceResumed&since={}",
                self.base_url, since
            );
            let events = self
                .client
                .get(&url)
                .header("X-API-Key", self.require_api_key()?)
                .send()
                .await
                .context("failed to reach /rest/events")?
                .error_for_status()
                .context("Syncthing returned an error on events endpoint")?
                .json::<Vec<SyncThingEvent>>()
                .await
                .context("failed to parse events")?;

            for event in &events {
                match &event.data {
                    crate::types::EventData::DeviceConnected(data) => {
                        println!("[{}] Device connected: {}", Local::now().format("%H:%M:%S"), device_label(&data.id));
                    }
                    crate::types::EventData::DeviceDisconnected(data) => {
                        println!("[{}] Device disconnected: {}", Local::now().format("%H:%M:%S"), device_label(&data.id));
                    }
                    crate::types::EventData::DevicePauseOrResume(data) => {
                        match event.event_type.as_str() {
                            "DevicePaused" => {
                                println!("[{}] Device paused: {}", Local::now().format("%H:%M:%S"), device_label(&data.device))
                            }
                            _ => println!("[{}] Device resumed: {}", Local::now().format("%H:%M:%S"), device_label(&data.device)),
                        }
                    }
                    _ => {}
                }

                if let crate::types::EventData::ItemFinished(data) = &event.data {
                    if data.folder != source_folder_id {
                        continue; // skip events from other folders
                    }

                    // some error while syncing the item, skip it and print the error
                    if data.error.is_some() {
                        println!(
                            "[{}] [{}] {} — skipping (error: {})",
                            Local::now().format("%H:%M:%S"),
                            data.folder,
                            data.item,
                            data.error.as_deref().unwrap()
                        );
                        continue;
                    }

                    let folder_root = folders
                        .iter()
                        .find(|f| f.id == data.folder)
                        .map(|f| &f.path)
                        .context("received event for unknown folder")?;

                    let expanded_root = if let Some(rest) = folder_root.strip_prefix("~/") {
                        let home =
                            dirs::home_dir().context("could not determine home directory")?;
                        home.join(rest)
                    } else {
                        std::path::PathBuf::from(folder_root)
                    };

                    let source = expanded_root.join(&data.item);
                    let destination = target_directory.join(&data.item);

                    // move the file to the target directory
                    mover::move_file(&source, &destination)
                        .await
                        .with_context(|| {
                            format!(
                                "failed to move file {} to {}",
                                data.item,
                                destination.display()
                            )
                        })?;
                }
            }

            if let Some(last) = events.last() {
                since = last.id;
            }
        }
    }
}
