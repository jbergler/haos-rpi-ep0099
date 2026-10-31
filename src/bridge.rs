use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rumqttc::{Client, QoS};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::bank::Bank;
use crate::relay::Relay;

const DISCOVERY_PREFIX: &str = "homeassistant";
const RESTORE_WAIT: Duration = Duration::from_secs(2);
const DISCOVERY_WILDCARD: &str = "homeassistant/switch/+/config";

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Mutex<Inner>>,
    client: Client,
    bank: Arc<Bank>,
    device_name: String,
    device_identifier: String,
    availability_topic: String,
    startup_state: String,
}

struct Inner {
    relays: Vec<Relay>,
    by_cmd: HashMap<String, usize>,
    by_state: HashMap<String, usize>,
    // Per-first-connect scratch.
    restore_collecting: bool,
    restore_pending: HashMap<usize, bool>, // relay index -> desired state
    cleanup_collecting: bool,
    cleanup_candidates: HashMap<String, DiscoveryPayload>, // config topic -> payload
    restored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiscoveryPayload {
    #[serde(default)]
    unique_id: String,
    #[serde(default)]
    state_topic: Option<String>,
    #[serde(default)]
    device: Option<DeviceInfo>,
    // Keep unknown fields on parse so we don't fail on new HA additions.
    #[serde(flatten)]
    _rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceInfo {
    #[serde(default)]
    identifiers: Vec<String>,
    #[serde(flatten)]
    _rest: serde_json::Map<String, serde_json::Value>,
}

impl Bridge {
    pub fn new(
        client: Client,
        bank: Arc<Bank>,
        relays: Vec<Relay>,
        base_topic: String,
        device_name: String,
        startup_state: String,
        availability_topic: String,
    ) -> Self {
        let device_identifier = format!("dockerpi_relay_{}", base_topic);
        let mut by_cmd = HashMap::new();
        let mut by_state = HashMap::new();
        for (i, r) in relays.iter().enumerate() {
            by_cmd.insert(r.cmd_topic.clone(), i);
            by_state.insert(r.stat_topic.clone(), i);
        }
        Self {
            inner: Arc::new(Mutex::new(Inner {
                relays,
                by_cmd,
                by_state,
                restore_collecting: false,
                restore_pending: HashMap::new(),
                cleanup_collecting: false,
                cleanup_candidates: HashMap::new(),
                restored: false,
            })),
            client,
            bank,
            device_name,
            device_identifier,
            availability_topic,
            startup_state,
        }
    }

    // ------------------------------------------------------------------
    // MQTT event entry points
    // ------------------------------------------------------------------

    /// Called when ConnAck arrives. Dispatches first-connect to a worker
    /// thread so the MQTT event loop stays free to deliver retained messages.
    pub fn on_connect(&self) {
        log::info!("MQTT connected");
        let already = {
            let g = self.inner.lock().unwrap();
            g.restored
        };
        if already {
            self.reconnect();
        } else {
            let me = self.clone();
            thread::spawn(move || me.first_connect());
        }
    }

    pub fn on_message(&self, topic: &str, payload: &[u8]) {
        // Dispatch order: restore state topics -> cleanup discovery -> command.
        let mut g = self.inner.lock().unwrap();

        if g.restore_collecting {
            if let Some(&idx) = g.by_state.get(topic) {
                let text = std::str::from_utf8(payload)
                    .unwrap_or("")
                    .trim()
                    .to_uppercase();
                if (text == "ON" || text == "OFF") && !g.restore_pending.contains_key(&idx) {
                    g.restore_pending.insert(idx, text == "ON");
                }
                return;
            }
        }

        if g.cleanup_collecting
            && topic.starts_with("homeassistant/switch/")
            && topic.ends_with("/config")
        {
            if payload.is_empty() {
                return;
            }
            if let Ok(parsed) = serde_json::from_slice::<DiscoveryPayload>(payload) {
                let belongs = parsed
                    .device
                    .as_ref()
                    .map(|d| d.identifiers.iter().any(|id| id == &self.device_identifier))
                    .unwrap_or(false);
                if belongs {
                    g.cleanup_candidates.insert(topic.to_string(), parsed);
                }
            }
            return;
        }

        // Command topic.
        if let Some(&idx) = g.by_cmd.get(topic) {
            let text = std::str::from_utf8(payload)
                .unwrap_or("")
                .trim()
                .to_uppercase();
            if text != "ON" && text != "OFF" {
                log::warn!("ignoring payload {text:?} on {topic}");
                return;
            }
            let logical = text == "ON";
            let relay = g.relays[idx].clone();
            // Release lock before I2C + publish to avoid holding it across I/O.
            drop(g);
            if self.bank.write(&relay, logical, "mqtt command") {
                let _ =
                    self.client
                        .publish(&relay.stat_topic, QoS::AtLeastOnce, true, text.as_bytes());
            }
        }
    }

    // ------------------------------------------------------------------
    // First connect (restore + cleanup + discovery)
    // ------------------------------------------------------------------

    fn first_connect(&self) {
        self.begin_cleanup();
        if self.startup_state == "restore" {
            self.begin_restore();
        }
        thread::sleep(RESTORE_WAIT);
        self.finish_cleanup();
        match self.startup_state.as_str() {
            "restore" => self.finish_restore(),
            "on" => self.force_all(true),
            _ => self.force_all(false),
        }
        self.publish_discovery();
        let _ = self.client.publish(
            &self.availability_topic,
            QoS::AtLeastOnce,
            true,
            b"online".as_slice(),
        );
        self.publish_cached_state();
        self.subscribe_command_topics();

        {
            let mut g = self.inner.lock().unwrap();
            g.restored = true;
            let board_count = g
                .relays
                .iter()
                .map(|r| r.board_addr)
                .collect::<BTreeSet<_>>()
                .len();
            log::info!(
                "ready: {} channels across {} board(s)",
                g.relays.len(),
                board_count
            );
        }
    }

    fn reconnect(&self) {
        self.publish_discovery();
        let _ = self.client.publish(
            &self.availability_topic,
            QoS::AtLeastOnce,
            true,
            b"online".as_slice(),
        );
        self.publish_cached_state();
        self.subscribe_command_topics();
        log::info!("reconnected; re-asserted availability and state");
    }

    // ---- Cleanup -----------------------------------------------------

    fn begin_cleanup(&self) {
        {
            let mut g = self.inner.lock().unwrap();
            g.cleanup_candidates.clear();
            g.cleanup_collecting = true;
        }
        if let Err(e) = self.client.subscribe(DISCOVERY_WILDCARD, QoS::AtLeastOnce) {
            log::warn!("subscribe {DISCOVERY_WILDCARD} failed: {e}");
        }
    }

    fn finish_cleanup(&self) {
        let _ = self.client.unsubscribe(DISCOVERY_WILDCARD);
        let (current_ids, found) = {
            let mut g = self.inner.lock().unwrap();
            g.cleanup_collecting = false;
            let current: BTreeSet<String> = g.relays.iter().map(|r| r.unique_id.clone()).collect();
            let found = std::mem::take(&mut g.cleanup_candidates);
            (current, found)
        };
        let mut removed = 0;
        for (topic, payload) in &found {
            if current_ids.contains(&payload.unique_id) {
                continue;
            }
            let _ = self
                .client
                .publish(topic.clone(), QoS::AtLeastOnce, true, Vec::<u8>::new());
            if let Some(stat) = &payload.state_topic {
                let _ = self
                    .client
                    .publish(stat.clone(), QoS::AtLeastOnce, true, Vec::<u8>::new());
            }
            log::info!(
                "cleanup: removed stale entity {} (topic={})",
                payload.unique_id,
                topic
            );
            removed += 1;
        }
        if removed > 0 {
            log::info!("cleanup: removed {removed} stale entity/entities");
        } else if !found.is_empty() {
            log::info!(
                "cleanup: {} existing entities, all still configured",
                found.len()
            );
        }
    }

    // ---- Restore -----------------------------------------------------

    fn begin_restore(&self) {
        log::info!(
            "startup_state=restore: collecting retained state ({:.1}s window)",
            RESTORE_WAIT.as_secs_f32()
        );
        let stat_topics: Vec<String> = {
            let mut g = self.inner.lock().unwrap();
            g.restore_pending.clear();
            g.restore_collecting = true;
            g.relays.iter().map(|r| r.stat_topic.clone()).collect()
        };
        for t in stat_topics {
            if let Err(e) = self.client.subscribe(t.clone(), QoS::AtLeastOnce) {
                log::warn!("subscribe {t} failed: {e}");
            }
        }
    }

    fn finish_restore(&self) {
        // Unsubscribe + apply.
        let (pending, relays) = {
            let mut g = self.inner.lock().unwrap();
            g.restore_collecting = false;
            let pending = std::mem::take(&mut g.restore_pending);
            let relays = g.relays.clone();
            (pending, relays)
        };
        for r in &relays {
            let _ = self.client.unsubscribe(r.stat_topic.clone());
        }
        for (i, r) in relays.iter().enumerate() {
            let (logical, src) = match pending.get(&i) {
                Some(&v) => (v, "retained"),
                None => (false, "default-off"),
            };
            self.bank.write(r, logical, &format!("restore:{src}"));
        }
    }

    fn force_all(&self, logical: bool) {
        let reason = format!("startup_state={}", self.startup_state);
        let relays = { self.inner.lock().unwrap().relays.clone() };
        for r in &relays {
            self.bank.write(r, logical, &reason);
        }
    }

    // ---- Discovery + state publishing -------------------------------

    fn publish_discovery(&self) {
        let device = json!({
            "identifiers": [self.device_identifier],
            "name": self.device_name,
            "manufacturer": "52Pi / GeeekPi",
            "model": "DockerPi 4-ch Relay HAT (EP-0099)",
        });
        let relays = { self.inner.lock().unwrap().relays.clone() };
        for r in &relays {
            let topic = format!("{}/switch/{}/config", DISCOVERY_PREFIX, r.unique_id);
            let payload = json!({
                "name": r.name,
                "unique_id": r.unique_id,
                "command_topic": r.cmd_topic,
                "state_topic": r.stat_topic,
                "payload_on": "ON",
                "payload_off": "OFF",
                "state_on": "ON",
                "state_off": "OFF",
                "optimistic": false,
                "availability_topic": self.availability_topic,
                "payload_available": "online",
                "payload_not_available": "offline",
                "device": device,
            });
            let bytes = serde_json::to_vec(&payload).unwrap_or_default();
            let _ = self.client.publish(topic, QoS::AtLeastOnce, true, bytes);
        }
    }

    fn publish_cached_state(&self) {
        let relays = { self.inner.lock().unwrap().relays.clone() };
        for r in &relays {
            let msg = if self.bank.get(r) { "ON" } else { "OFF" };
            let _ =
                self.client
                    .publish(r.stat_topic.clone(), QoS::AtLeastOnce, true, msg.as_bytes());
        }
    }

    fn subscribe_command_topics(&self) {
        let topics: Vec<String> = {
            self.inner
                .lock()
                .unwrap()
                .relays
                .iter()
                .map(|r| r.cmd_topic.clone())
                .collect()
        };
        for t in topics {
            if let Err(e) = self.client.subscribe(t.clone(), QoS::AtLeastOnce) {
                log::warn!("subscribe {t} failed: {e}");
            }
        }
    }

    pub fn publish_offline_sync(&self) {
        let _ = self.client.publish(
            &self.availability_topic,
            QoS::AtLeastOnce,
            true,
            b"offline".as_slice(),
        );
    }
}
