use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    #[serde(rename = "myID")]
    pub my_id: String,
    pub uptime: u64,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    #[serde(rename = "deviceID")]
    pub device_id: String,
    pub name: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
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

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SyncThingEvent {
    pub id: u64,
    pub global_id: Option<u64>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: EventData,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ItemFinishedData {
    pub folder: String,
    pub item: String,
    pub action: String,
    pub error: Option<String>,
    #[serde(rename = "type")]
    pub item_type: String, // "file" or "dir"
}
