use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rumqttc::{Client, Event, Incoming, LastWill, MqttOptions, QoS};

mod bank;
mod bridge;
mod config;
mod relay;
mod supervisor;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(Some(env_logger::fmt::TimestampPrecision::Seconds))
        .init();

    let opts_path = std::env::var("OPTIONS_PATH").unwrap_or_else(|_| "/data/options.json".into());
    let opts = config::load(&opts_path).with_context(|| format!("load {opts_path}"))?;
    let relays = relay::build_relays(&opts).context("build relays")?;

    let addr_set: std::collections::BTreeSet<String> = relays
        .iter()
        .map(|r| format!("0x{:02x}", r.board_addr))
        .collect();
    log::info!(
        "configured {} relay(s) on bus {} across {:?}",
        relays.len(),
        opts.i2c_bus,
        addr_set
    );

    let bank = Arc::new(bank::Bank::new(opts.i2c_bus).context("open i2c")?);
    let creds = supervisor::mqtt_credentials().context("fetch mqtt credentials from Supervisor")?;

    let availability_topic = format!("{}/availability", opts.base_topic);
    let client_id = format!("dockerpi_relay_{}", opts.base_topic);
    let mut mqttopts = MqttOptions::new(&client_id, &creds.host, creds.port);
    if !creds.username.is_empty() {
        mqttopts.set_credentials(&creds.username, &creds.password);
    }
    mqttopts.set_last_will(LastWill::new(
        &availability_topic,
        "offline",
        QoS::AtLeastOnce,
        true,
    ));
    mqttopts.set_keep_alive(Duration::from_secs(30));

    log::info!("connecting to MQTT {}:{}", creds.host, creds.port);
    let (client, mut connection) = Client::new(mqttopts, 32);

    let bridge = bridge::Bridge::new(
        client.clone(),
        bank,
        relays,
        opts.base_topic.clone(),
        opts.device_name.clone(),
        opts.startup_state.clone(),
        availability_topic.clone(),
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let sh = shutdown.clone();
        let sh_client = client.clone();
        ctrlc::set_handler(move || {
            log::info!("shutdown requested");
            sh.store(true, Ordering::SeqCst);
            // Nudge the event loop so iter() returns promptly.
            let _ = sh_client.disconnect();
        })
        .context("install signal handler")?;
    }

    for ev in connection.iter() {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        match ev {
            Ok(Event::Incoming(Incoming::ConnAck(_))) => bridge.on_connect(),
            Ok(Event::Incoming(Incoming::Publish(p))) => bridge.on_message(&p.topic, &p.payload),
            Ok(_) => {}
            Err(e) => {
                log::warn!("mqtt event error: {e}");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }

    log::info!("publishing offline and disconnecting");
    bridge.publish_offline_sync();
    std::thread::sleep(Duration::from_millis(500));
    let _ = client.disconnect();
    Ok(())
}
