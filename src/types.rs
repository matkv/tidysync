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
