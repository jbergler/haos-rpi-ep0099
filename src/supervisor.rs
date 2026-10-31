use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct MqttCreds {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Deserialize)]
struct MqttData {
    host: String,
    port: u16,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}

/// Fetch MQTT broker credentials via the Supervisor services API
/// (what `bashio::services mqtt` wraps). Requires `services: [mqtt:need]`
/// in config.yaml and `SUPERVISOR_TOKEN` to be in the environment.
pub fn mqtt_credentials() -> Result<MqttCreds> {
    let token = std::env::var("SUPERVISOR_TOKEN")
        .context("SUPERVISOR_TOKEN not set (not running under Supervisor?)")?;
    let resp: Envelope<MqttData> = ureq::get("http://supervisor/services/mqtt")
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .call()
        .context("GET /services/mqtt")?
        .into_json()
        .context("decode /services/mqtt body")?;
    Ok(MqttCreds {
        host: resp.data.host,
        port: resp.data.port,
        username: resp.data.username,
        password: resp.data.password,
    })
}
