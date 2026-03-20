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
}
