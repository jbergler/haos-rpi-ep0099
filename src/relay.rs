use std::collections::BTreeSet;

use anyhow::{anyhow, Result};

use crate::config::{parse_addr, Options};

#[derive(Debug, Clone)]
pub struct Relay {
    pub board_addr: u8,
    pub channel: u8,
    pub name: String,
    pub invert: bool,
    pub unique_id: String,
    pub cmd_topic: String,
    pub stat_topic: String,
}

impl Relay {
    pub fn register(&self) -> u8 {
        self.channel
    }
}

pub fn build_relays(opts: &Options) -> Result<Vec<Relay>> {
    if opts.relays.is_empty() {
        return Err(anyhow!(
            "options.relays is empty; declare at least one channel"
        ));
    }

    // Parse all addresses once; decide whether to disambiguate names by address.
    let mut parsed: Vec<(u8, &crate::config::RelayEntry)> = Vec::with_capacity(opts.relays.len());
    let mut addr_set: BTreeSet<u8> = BTreeSet::new();
    for e in &opts.relays {
        let addr = parse_addr(&e.address)?;
        addr_set.insert(addr);
        parsed.push((addr, e));
    }
    let multi = addr_set.len() > 1;

    let mut seen: BTreeSet<(u8, u8)> = BTreeSet::new();
    let mut relays = Vec::with_capacity(parsed.len());
    for (addr, e) in parsed {
        let ch = e.channel;
        if !seen.insert((addr, ch)) {
            return Err(anyhow!("duplicate relay entry for 0x{addr:02x} ch={ch}"));
        }
        let default_name = if multi {
            format!("Relay 0x{addr:02x}/{ch}")
        } else {
            format!("Relay {ch}")
        };
        let name = e
            .name
            .clone()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(default_name);
        let wiring = e
            .wiring
            .clone()
            .unwrap_or_else(|| "normally_open".into())
            .to_lowercase();
        let invert = wiring == "normally_closed";
        let (tag, unique) = (
            if multi {
                format!("{:02x}_{}", addr, ch)
            } else {
                ch.to_string()
            },
            format!("dockerpi_relay_{:02x}_{}", addr, ch),
        );
        relays.push(Relay {
            board_addr: addr,
            channel: ch,
            name,
            invert,
            unique_id: unique,
            cmd_topic: format!("{}/{}/set", opts.base_topic, tag),
            stat_topic: format!("{}/{}/state", opts.base_topic, tag),
        });
    }
    Ok(relays)
}
