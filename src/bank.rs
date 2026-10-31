use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Context, Result};
use i2cdev::core::I2CDevice;
use i2cdev::linux::LinuxI2CDevice;

use crate::relay::Relay;

/// One `LinuxI2CDevice` per board address, protected by a single mutex to
/// serialise access across the MQTT event loop and the restore worker.
pub struct Bank {
    inner: Mutex<Inner>,
    bus_no: u8,
}

struct Inner {
    devices: HashMap<u8, LinuxI2CDevice>,
    state: HashMap<(u8, u8), bool>,
}

impl Bank {
    pub fn new(bus_no: u8) -> Result<Self> {
        Ok(Self {
            inner: Mutex::new(Inner {
                devices: HashMap::new(),
                state: HashMap::new(),
            }),
            bus_no,
        })
    }

    fn get_or_open(inner: &mut Inner, bus_no: u8, addr: u8) -> Result<&mut LinuxI2CDevice> {
        use std::collections::hash_map::Entry;
        match inner.devices.entry(addr) {
            Entry::Occupied(e) => Ok(e.into_mut()),
            Entry::Vacant(e) => {
                let path = format!("/dev/i2c-{bus_no}");
                let dev = LinuxI2CDevice::new(&path, u16::from(addr))
                    .with_context(|| format!("open {path} addr=0x{addr:02x}"))?;
                Ok(e.insert(dev))
            }
        }
    }

    pub fn write(&self, relay: &Relay, logical_on: bool, reason: &str) -> bool {
        let chip_on = logical_on ^ relay.invert;
        let byte: u8 = if chip_on { 0xFF } else { 0x00 };
        let mut guard = self.inner.lock().unwrap();
        let bus_no = self.bus_no;
        match Self::get_or_open(&mut guard, bus_no, relay.board_addr) {
            Ok(dev) => match dev.smbus_write_byte_data(relay.register(), byte) {
                Ok(()) => {
                    guard
                        .state
                        .insert((relay.board_addr, relay.channel), logical_on);
                    log::info!(
                        "relay {} (0x{:02x}/{}) -> {} (chip=0x{:02x}, reason={})",
                        relay.name,
                        relay.board_addr,
                        relay.channel,
                        if logical_on { "ON" } else { "OFF" },
                        byte,
                        reason,
                    );
                    true
                }
                Err(e) => {
                    log::error!(
                        "relay {} (0x{:02x}/{}) write FAILED: {} (reason={})",
                        relay.name,
                        relay.board_addr,
                        relay.channel,
                        e,
                        reason,
                    );
                    false
                }
            },
            Err(e) => {
                log::error!(
                    "relay {} (0x{:02x}/{}) open FAILED: {} (reason={})",
                    relay.name,
                    relay.board_addr,
                    relay.channel,
                    e,
                    reason,
                );
                false
            }
        }
    }

    pub fn get(&self, relay: &Relay) -> bool {
        let guard = self.inner.lock().unwrap();
        *guard
            .state
            .get(&(relay.board_addr, relay.channel))
            .unwrap_or(&false)
    }
}
