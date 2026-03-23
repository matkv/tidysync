use std::fmt::Display;

use serde::Deserialize;

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemStatus {
    #[serde(rename = "myID")]
    pub my_id: String,
    pub uptime: u64,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Device {
    #[serde(rename = "deviceID")]
    pub device_id: String,
    pub name: String,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Folder {
    pub id: String,
    pub label: String,
    pub path: String,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum EventData {
    ItemFinished(ItemFinishedData),
    Other(serde_json::Value), // For events we don't specifically handle yet
}

impl Default for EventData {
    fn default() -> Self {
        EventData::Other(serde_json::Value::Null)
    }
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncThingEvent {
    pub id: u64,
    pub global_id: Option<u64>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: EventData,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ItemAction {
    #[default]
    Update,
    Delete,
    Metadata,
}

impl Display for ItemAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemAction::Update => write!(f, "update"),
            ItemAction::Delete => write!(f, "delete"),
            ItemAction::Metadata => write!(f, "metadata"),
        }
    }
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    #[default]
    File,
    Directory,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ItemFinishedData {
    pub folder: String,
    pub item: String,
    pub action: ItemAction,
    pub error: Option<String>,
    #[serde(rename = "type")]
    pub item_type: ItemType,
}
