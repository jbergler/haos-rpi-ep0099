use std::fs;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RelayEntry {
    pub address: String,
    pub channel: u8,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub wiring: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Options {
    #[serde(default = "default_i2c_bus")]
    pub i2c_bus: u8,
    #[serde(default = "default_base_topic")]
    pub base_topic: String,
    #[serde(default = "default_device_name")]
    pub device_name: String,
    #[serde(default = "default_startup_state")]
    pub startup_state: String,
    #[serde(default)]
    pub relays: Vec<RelayEntry>,
}

fn default_i2c_bus() -> u8 {
    1
}
fn default_base_topic() -> String {
    "dockerpi_relay".into()
}
fn default_device_name() -> String {
    "DockerPi Relay".into()
}
fn default_startup_state() -> String {
    "restore".into()
}

pub fn load(path: &str) -> Result<Options> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {path}"))?;
    let mut opts: Options = serde_json::from_str(&raw).with_context(|| format!("parse {path}"))?;
    opts.base_topic = opts.base_topic.trim_matches('/').to_string();
    let ss = opts.startup_state.to_lowercase();
    opts.startup_state = match ss.as_str() {
        "restore" | "off" | "on" => ss,
        other => {
            log::warn!("unknown startup_state {other:?}, falling back to 'restore'");
            "restore".into()
        }
    };
    if opts.relays.is_empty() {
        return Err(anyhow!(
            "options.relays is empty; declare at least one channel"
        ));
    }
    Ok(opts)
}

pub fn parse_addr(raw: &str) -> Result<u8> {
    let s = raw.trim();
    let (base, body) = if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16u32, rest)
    } else {
        (10u32, s)
    };
    u8::from_str_radix(body, base).with_context(|| format!("invalid i2c address {raw:?}"))
}
