use crate::types::{Device, Folder, SyncThingEvent, SystemStatus};
use anyhow::{Context, Result};
use reqwest::Client;

pub struct SyncThingClient {
    pub base_url: String,
    pub api_key: String,
    client: Client,
}

impl SyncThingClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            base_url,
            api_key,
            client: Client::new(),
        }
    }

    pub async fn ping(&self) -> Result<()> {
        let url = format!("{}/rest/system/ping", self.base_url);

        self.client
            .get(&url)
            .header("X-API-Key", &self.api_key)
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
            .header("X-API-Key", &self.api_key)
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
            .header("X-API-Key", &self.api_key)
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
            .header("X-API-Key", &self.api_key)
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

    pub async fn watch_item_finished(
        &self,
        source_folder_id: &str,
        target_directory: &std::path::Path,
    ) -> Result<()> {
        let mut since: u64 = 0;

        loop {
            let url = format!(
                "{}/rest/events?events=ItemFinished&since={}",
                self.base_url, since
            );
            let events = self
                .client
                .get(&url)
                .header("X-API-Key", &self.api_key)
                .send()
                .await
                .context("failed to reach /rest/events")?
                .error_for_status()
                .context("Syncthing returned an error on events endpoint")?
                .json::<Vec<SyncThingEvent>>()
                .await
                .context("failed to parse ItemFinished events")?;

            for event in &events {
                if let crate::types::EventData::ItemFinished(data) = &event.data {
                    if data.folder != source_folder_id {
                        continue; // skip events from other folders
                    }

                    // some error while syncing the item, skip it and print the error
                    if data.error.is_some() {
                        println!(
                            "[{}] {} — skipping (error: {})",
                            data.folder,
                            data.item,
                            data.error.as_deref().unwrap()
                        );
                        continue;
                    }

                    // print everything and where the file will be moved to
                    println!(
                        "[{}] {} {} — moving to {}",
                        data.folder,
                        data.item,
                        data.action,
                        target_directory.join(&data.item).display()
                    );
                }
            }

            if let Some(last) = events.last() {
                since = last.id;
            }
        }
    }
}
