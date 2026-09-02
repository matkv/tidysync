use crate::{
    mover,
    types::{Device, Folder, SyncThingEvent, SystemStatus},
};
use anyhow::{Context, Result};
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Event types the watcher subscribes to.
///
/// Syncthing's event IDs are per-filter: seeding the cursor with one filter and
/// then polling with another yields IDs from a different sequence, which makes
/// the poll miss events or block forever. Both call sites read this constant so
/// they cannot drift apart.
const EVENT_FILTER: &str =
    "ItemFinished,DeviceConnected,DeviceDisconnected,DevicePaused,DeviceResumed";

/// Backoff bounds for reconnecting to Syncthing after a failed poll.
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(60);

pub struct SyncThingClient {
    pub base_url: String,
    pub api_key: Option<String>,
    client: Client,
}

/// Syncthing stores folder paths with a literal `~/` prefix, which the OS will
/// not expand for us.
fn expand_home(path: &str) -> Result<std::path::PathBuf> {
    match path.strip_prefix("~/") {
        Some(rest) => {
            let home = dirs::home_dir().context("could not determine home directory")?;
            Ok(home.join(rest))
        }
        None => Ok(std::path::PathBuf::from(path)),
    }
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

    /// Fetch a batch of events, long-polling until Syncthing has something to say.
    async fn fetch_events(&self, since: u64, api_key: &str) -> Result<Vec<SyncThingEvent>> {
        let url = format!(
            "{}/rest/events?events={EVENT_FILTER}&since={since}",
            self.base_url
        );

        self.client
            .get(&url)
            .header("X-API-Key", api_key)
            .send()
            .await
            .context("failed to reach /rest/events")?
            .error_for_status()
            .context("Syncthing returned an error on events endpoint")?
            .json::<Vec<SyncThingEvent>>()
            .await
            .context("failed to parse events")
    }

    async fn latest_event_id(&self) -> Result<u64> {
        // Fetch the most recent event using the same filter as the main loop —
        // see EVENT_FILTER.
        let url = format!(
            "{}/rest/events?since=0&limit=1&events={EVENT_FILTER}",
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

        let expanded_root = expand_home(&source_folder.path)?;

        info!(
            "Scanning for existing files in {}...",
            expanded_root.display()
        );
        mover::move_existing_files(&expanded_root, target_directory)
            .await
            .context("pre-scan move failed")?;
        info!("Pre-scan complete. Watching for new events...");

        let mut since: u64 = self.latest_event_id().await?;
        debug!("Seeded event cursor at {since}");

        // Resolved once, up front: a missing API key is a configuration problem
        // rather than a transient one, so it must not be swallowed by the retry
        // loop below and turned into an endless reconnect.
        let api_key = self.require_api_key()?.to_string();

        let mut backoff = RETRY_MIN;

        loop {
            let events = match self.fetch_events(since, &api_key).await {
                Ok(events) => {
                    backoff = RETRY_MIN;
                    events
                }
                Err(err) => {
                    // Syncthing restarting or the network dropping should not be
                    // fatal — the watcher has to outlive both.
                    warn!(
                        "Event poll failed, retrying in {}s: {err:#}",
                        backoff.as_secs()
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(RETRY_MAX);
                    continue;
                }
            };

            debug!("Received {} event(s) since {}", events.len(), since);

            for event in &events {
                debug!("Event {} type={}", event.id, event.event_type);

                match &event.data {
                    crate::types::EventData::DeviceConnected(data) => {
                        info!("Device connected: {}", device_label(&data.id));
                    }
                    crate::types::EventData::DeviceDisconnected(data) => {
                        info!("Device disconnected: {}", device_label(&data.id));
                    }
                    crate::types::EventData::DevicePauseOrResume(data) => {
                        match event.event_type.as_str() {
                            "DevicePaused" => info!("Device paused: {}", device_label(&data.device)),
                            _ => info!("Device resumed: {}", device_label(&data.device)),
                        }
                    }
                    _ => {}
                }

                if let crate::types::EventData::ItemFinished(data) = &event.data {
                    if data.folder != source_folder_id {
                        continue; // skip events from other folders
                    }

                    // some error while syncing the item, skip it and print the error
                    if let Some(error) = &data.error {
                        warn!(
                            "[{}] {} — skipping (error: {})",
                            data.folder, data.item, error
                        );
                        continue;
                    }

                    let Some(folder_root) = folders.iter().find(|f| f.id == data.folder) else {
                        warn!(
                            "Received event for unknown folder '{}', skipping",
                            data.folder
                        );
                        continue;
                    };

                    let expanded_root = match expand_home(&folder_root.path) {
                        Ok(root) => root,
                        Err(err) => {
                            warn!("Could not resolve path for '{}': {err:#}", data.folder);
                            continue;
                        }
                    };

                    let source = expanded_root.join(&data.item);
                    let destination = target_directory.join(&data.item);

                    // A single unmovable file must not take the watcher down with
                    // it — log and move on to the next event.
                    if let Err(err) = mover::move_file(&source, &destination).await {
                        warn!(
                            "Failed to move {} to {}: {err:#}",
                            data.item,
                            destination.display()
                        );
                    }
                }
            }

            if let Some(last) = events.last() {
                since = last.id;
            }
        }
    }
}
