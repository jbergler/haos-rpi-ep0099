"""MQTT <-> I2C bridge for the 52Pi/GeeekPi DockerPi 4-channel relay HAT (EP-0099).

Each board exposes four 1-byte registers (0x01..0x04). Writing 0xFF turns the
relay on, 0x00 turns it off. Up to four boards may be stacked at
0x10..0x13 via the on-board DIP switches.

Publishes Home Assistant MQTT discovery so each relay appears as a `switch`.
"""
from __future__ import annotations

import json
import logging
import os
import signal
import sys
import threading
import time
from dataclasses import dataclass

import paho.mqtt.client as mqtt
from smbus2 import SMBus

LOG = logging.getLogger("dockerpi_relay")
logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")

DISCOVERY_PREFIX = "homeassistant"


@dataclass(frozen=True)
class Relay:
    board_addr: int   # I2C address (e.g. 0x10)
    channel: int      # 1..num_relays
    unique_id: str
    name: str
    cmd_topic: str
    stat_topic: str

    @property
    def register(self) -> int:
        return self.channel  # registers are 0x01..0x04


def parse_addresses(raw: str) -> list[int]:
    out = []
    for part in raw.split(","):
        part = part.strip()
        if not part:
            continue
        out.append(int(part, 0))
    if not out:
        raise ValueError("no I2C addresses configured")
    return out


class RelayBank:
    """Thread-safe wrapper around an SMBus handle plus cached relay state."""

    def __init__(self, bus_no: int, addresses: list[int], channels: int) -> None:
        self._bus = SMBus(bus_no)
        self._lock = threading.Lock()
        self._state: dict[tuple[int, int], bool] = {
            (addr, ch): False for addr in addresses for ch in range(1, channels + 1)
        }
        # Force all off on startup so cached state matches hardware.
        for (addr, ch) in list(self._state):
            self._write(addr, ch, False)

    def _write(self, addr: int, channel: int, on: bool) -> None:
        value = 0xFF if on else 0x00
        with self._lock:
            self._bus.write_byte_data(addr, channel, value)
            self._state[(addr, channel)] = on

    def set(self, addr: int, channel: int, on: bool) -> bool:
        try:
            self._write(addr, channel, on)
            return True
        except OSError as exc:
            LOG.error("I2C write failed addr=0x%02x ch=%d: %s", addr, channel, exc)
            return False

    def get(self, addr: int, channel: int) -> bool:
        return self._state[(addr, channel)]

    def close(self) -> None:
        with self._lock:
            self._bus.close()


def build_relays(addresses: list[int], channels: int, base_topic: str) -> list[Relay]:
    relays: list[Relay] = []
    multi = len(addresses) > 1
    for addr in addresses:
        for ch in range(1, channels + 1):
            tag = f"{addr:02x}_{ch}" if multi else f"{ch}"
            unique = f"dockerpi_relay_{addr:02x}_{ch}"
            name = f"Relay {addr:#04x}/{ch}" if multi else f"Relay {ch}"
            relays.append(
                Relay(
                    board_addr=addr,
                    channel=ch,
                    unique_id=unique,
                    name=name,
                    cmd_topic=f"{base_topic}/{tag}/set",
                    stat_topic=f"{base_topic}/{tag}/state",
                )
            )
    return relays


def publish_discovery(client: mqtt.Client, relays: list[Relay], device_name: str,
                      base_topic: str) -> None:
    availability_topic = f"{base_topic}/availability"
    device = {
        "identifiers": [f"dockerpi_relay_{base_topic}"],
        "name": device_name,
        "manufacturer": "52Pi / GeeekPi",
        "model": "DockerPi 4-ch Relay HAT (EP-0099)",
    }
    for r in relays:
        topic = f"{DISCOVERY_PREFIX}/switch/{r.unique_id}/config"
        payload = {
            "name": r.name,
            "unique_id": r.unique_id,
            "command_topic": r.cmd_topic,
            "state_topic": r.stat_topic,
            "payload_on": "ON",
            "payload_off": "OFF",
            "state_on": "ON",
            "state_off": "OFF",
            "optimistic": False,
            "availability_topic": availability_topic,
            "payload_available": "online",
            "payload_not_available": "offline",
            "device": device,
        }
        client.publish(topic, json.dumps(payload), qos=1, retain=True)


def main() -> int:
    bus_no = int(os.environ["I2C_BUS"])
    channels = int(os.environ["NUM_RELAYS"])
    base_topic = os.environ["BASE_TOPIC"].strip("/")
    device_name = os.environ["DEVICE_NAME"]
    poll_interval = int(os.environ.get("POLL_INTERVAL", "0"))
    addresses = parse_addresses(os.environ["I2C_ADDRESSES"])

    bank = RelayBank(bus_no, addresses, channels)
    relays = build_relays(addresses, channels, base_topic)
    by_cmd = {r.cmd_topic: r for r in relays}

    availability_topic = f"{base_topic}/availability"

    client = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id="dockerpi_relay")
    user = os.environ.get("MQTT_USER") or None
    if user:
        client.username_pw_set(user, os.environ.get("MQTT_PASS") or None)
    client.will_set(availability_topic, "offline", qos=1, retain=True)

    def on_connect(c, _ud, _flags, reason_code, _props=None):
        if reason_code != 0:
            LOG.error("MQTT connect failed: %s", reason_code)
            return
        LOG.info("MQTT connected")
        publish_discovery(c, relays, device_name, base_topic)
        c.publish(availability_topic, "online", qos=1, retain=True)
        for r in relays:
            c.subscribe(r.cmd_topic, qos=1)
            c.publish(r.stat_topic, "ON" if bank.get(r.board_addr, r.channel) else "OFF",
                      qos=1, retain=True)

    def on_message(c, _ud, msg):
        r = by_cmd.get(msg.topic)
        if r is None:
            return
        payload = msg.payload.decode(errors="replace").strip().upper()
        if payload not in ("ON", "OFF"):
            LOG.warning("ignoring payload %r on %s", payload, msg.topic)
            return
        if bank.set(r.board_addr, r.channel, payload == "ON"):
            c.publish(r.stat_topic, payload, qos=1, retain=True)

    client.on_connect = on_connect
    client.on_message = on_message

    host = os.environ["MQTT_HOST"]
    port = int(os.environ.get("MQTT_PORT", "1883"))
    LOG.info("connecting to MQTT %s:%d", host, port)
    client.connect(host, port, keepalive=30)

    stop = threading.Event()

    def shutdown(*_):
        LOG.info("shutting down")
        try:
            client.publish(availability_topic, "offline", qos=1, retain=True).wait_for_publish(2)
        except Exception:
            pass
        stop.set()

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)

    client.loop_start()
    try:
        while not stop.is_set():
            if poll_interval > 0:
                # Re-publish retained state so HA stays authoritative after broker restarts.
                for r in relays:
                    client.publish(
                        r.stat_topic,
                        "ON" if bank.get(r.board_addr, r.channel) else "OFF",
                        qos=1, retain=True,
                    )
                stop.wait(poll_interval)
            else:
                stop.wait(1.0)
    finally:
        client.loop_stop()
        client.disconnect()
        bank.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
