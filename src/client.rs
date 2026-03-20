use crate::types::{Device, Folder, ItemFinishedEvent, SystemStatus};
use anyhow::Result;
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
            .await?
            .error_for_status()?;

        Ok(())
    }

    pub async fn system_status(&self) -> Result<SystemStatus> {
        let url = format!("{}/rest/system/status", self.base_url);
        let status = self
            .client
            .get(&url)
            .header("X-API-Key", &self.api_key)
            .send()
            .await?
            .error_for_status()?
            .json::<SystemStatus>()
            .await?;
        Ok(status)
    }

    pub async fn folders(&self) -> Result<Vec<Folder>> {
        let url = format!("{}/rest/config/folders", self.base_url);
        let folders = self
            .client
            .get(&url)
            .header("X-API-Key", &self.api_key)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<Folder>>()
            .await?;
        Ok(folders)
    }

    pub async fn devices(&self) -> Result<Vec<Device>> {
        let url = format!("{}/rest/config/devices", self.base_url);
        let devices = self
            .client
            .get(&url)
            .header("X-API-Key", &self.api_key)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<Device>>()
            .await?;
        Ok(devices)
    }

    pub async fn watch_item_finished(&self) -> Result<()> {
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
                .await?
                .error_for_status()?
                .json::<Vec<ItemFinishedEvent>>()
                .await?;

            for event in &events {
                println!(
                    "[{}] {}/{} — {}",
                    event.data.action,
                    event.data.folder,
                    event.data.item,
                    event.data.error.as_deref().unwrap_or("ok")
                );
            }

            if let Some(last) = events.last() {
                since = last.id;
            }
        }
    }
}
