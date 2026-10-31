"""MQTT <-> I2C bridge for the 52Pi/GeeekPi DockerPi 4-channel relay HAT (EP-0099).

Each board exposes four 1-byte registers (0x01..0x04). Writing 0xFF energises
the coil, 0x00 de-energises it. Up to four boards may be stacked at 0x10..0x13.

Published as Home Assistant MQTT discovery `switch` entities.

Design notes:
* The chip is write-only -- there is no readback. Cached state in this
  process is the only in-process truth; MQTT retained state is the only
  cross-restart truth. See `startup_state` for how we reconcile them.
* `wiring: normally_closed` inverts the chip write so that HA `ON` always
  means "load energised" regardless of which contact the user wired through.
"""
from __future__ import annotations

import json
import logging
import os
import signal
import sys
import threading
import time
from dataclasses import dataclass, field

import paho.mqtt.client as mqtt
from smbus2 import SMBus

LOG = logging.getLogger("dockerpi_relay")
logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")

DISCOVERY_PREFIX = "homeassistant"
RESTORE_WAIT_SEC = 2.0


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

@dataclass(frozen=True)
class Relay:
    board_addr: int   # I2C address (e.g. 0x10)
    channel: int      # 1..num_relays
    name: str
    invert: bool      # True for normally_closed wiring
    unique_id: str
    cmd_topic: str
    stat_topic: str

    @property
    def register(self) -> int:
        return self.channel  # registers are 0x01..0x04


def _parse_addr(raw: str) -> int:
    return int(raw, 0)


def build_relays(opts: dict, base_topic: str) -> list[Relay]:
    entries = opts.get("relays") or []
    if not entries:
        raise ValueError("options.relays is empty; declare at least one channel")

    # Decide whether to disambiguate names/topics by address: only if more
    # than one distinct address is in play.
    addr_set = {_parse_addr(e["address"]) for e in entries}
    multi = len(addr_set) > 1

    seen: set[tuple[int, int]] = set()
    relays: list[Relay] = []
    for e in entries:
        addr = _parse_addr(e["address"])
        ch = int(e["channel"])
        key = (addr, ch)
        if key in seen:
            raise ValueError(f"duplicate relay entry for 0x{addr:02x} ch={ch}")
        seen.add(key)

        default_name = f"Relay {addr:#04x}/{ch}" if multi else f"Relay {ch}"
        name = (e.get("name") or default_name).strip() or default_name
        wiring = (e.get("wiring") or "normally_open").lower()
        invert = wiring == "normally_closed"
        tag = f"{addr:02x}_{ch}" if multi else f"{ch}"
        unique = f"dockerpi_relay_{addr:02x}_{ch}"
        relays.append(
            Relay(
                board_addr=addr,
                channel=ch,
                name=name,
                invert=invert,
                unique_id=unique,
                cmd_topic=f"{base_topic}/{tag}/set",
                stat_topic=f"{base_topic}/{tag}/state",
            )
        )
    return relays


# ---------------------------------------------------------------------------
# Hardware
# ---------------------------------------------------------------------------

class RelayBank:
    """Thread-safe wrapper around SMBus plus cached logical state."""

    def __init__(self, bus_no: int) -> None:
        self._bus = SMBus(bus_no)
        self._lock = threading.Lock()
        self._state: dict[tuple[int, int], bool] = {}

    def write(self, relay: Relay, logical_on: bool, *, reason: str) -> bool:
        # logical_on is what HA sees. XOR with invert for the chip.
        chip_on = logical_on ^ relay.invert
        byte = 0xFF if chip_on else 0x00
        try:
            with self._lock:
                self._bus.write_byte_data(relay.board_addr, relay.register, byte)
                self._state[(relay.board_addr, relay.channel)] = logical_on
        except OSError as exc:
            LOG.error("relay %s (0x%02x/%d) write FAILED: %s (reason=%s)",
                      relay.name, relay.board_addr, relay.channel, exc, reason)
            return False
        LOG.info("relay %s (0x%02x/%d) -> %s (chip=0x%02x, reason=%s)",
                 relay.name, relay.board_addr, relay.channel,
                 "ON" if logical_on else "OFF", byte, reason)
        return True

    def get(self, relay: Relay) -> bool:
        return self._state.get((relay.board_addr, relay.channel), False)

    def close(self) -> None:
        with self._lock:
            self._bus.close()


# ---------------------------------------------------------------------------
# Bridge
# ---------------------------------------------------------------------------

@dataclass
class Bridge:
    relays: list[Relay]
    bank: RelayBank
    client: mqtt.Client
    base_topic: str
    device_name: str
    startup_state: str  # "restore" | "off" | "on"
    availability_topic: str
    by_cmd: dict[str, Relay] = field(default_factory=dict)
    by_state: dict[str, Relay] = field(default_factory=dict)
    _restored: bool = False
    _restore_pending: dict[Relay, bool | None] = field(default_factory=dict)
    _restore_lock: threading.Lock = field(default_factory=threading.Lock)

    def __post_init__(self) -> None:
        self.by_cmd = {r.cmd_topic: r for r in self.relays}
        self.by_state = {r.stat_topic: r for r in self.relays}

    # ---- MQTT discovery / lifecycle -----------------------------------

    def publish_discovery(self) -> None:
        device = {
            "identifiers": [f"dockerpi_relay_{self.base_topic}"],
            "name": self.device_name,
            "manufacturer": "52Pi / GeeekPi",
            "model": "DockerPi 4-ch Relay HAT (EP-0099)",
        }
        for r in self.relays:
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
                "availability_topic": self.availability_topic,
                "payload_available": "online",
                "payload_not_available": "offline",
                "device": device,
            }
            self.client.publish(topic, json.dumps(payload), qos=1, retain=True)

    def publish_cached_state(self) -> None:
        for r in self.relays:
            self.client.publish(
                r.stat_topic,
                "ON" if self.bank.get(r) else "OFF",
                qos=1, retain=True,
            )

    # ---- Restore flow (first connect only) ----------------------------

    def _begin_restore(self) -> None:
        """Subscribe to our own state topics; retained values trigger _on_restore."""
        LOG.info("startup_state=restore: collecting retained state (%.1fs window)",
                 RESTORE_WAIT_SEC)
        self._restore_pending = {r: None for r in self.relays}
        for r in self.relays:
            self.client.message_callback_add(r.stat_topic, self._on_restore_msg)
            self.client.subscribe(r.stat_topic, qos=1)

    def _on_restore_msg(self, _c, _ud, msg) -> None:
        r = self.by_state.get(msg.topic)
        if r is None:
            return
        payload = msg.payload.decode(errors="replace").strip().upper()
        with self._restore_lock:
            if self._restore_pending.get(r) is None and payload in ("ON", "OFF"):
                self._restore_pending[r] = (payload == "ON")

    def _finish_restore(self) -> None:
        """After the wait window, apply desired state to hardware."""
        # Unsubscribe + drop the per-topic callbacks before we publish.
        for r in self.relays:
            try:
                self.client.unsubscribe(r.stat_topic)
                self.client.message_callback_remove(r.stat_topic)
            except Exception:
                pass
        with self._restore_lock:
            pending = dict(self._restore_pending)
        for r, desired in pending.items():
            logical = bool(desired)  # None -> False (safe default)
            src = "retained" if desired is not None else "default-off"
            self.bank.write(r, logical, reason=f"restore:{src}")

    def _force_all(self, logical_on: bool) -> None:
        reason = f"startup_state={self.startup_state}"
        for r in self.relays:
            self.bank.write(r, logical_on, reason=reason)

    def _first_connect(self) -> None:
        if self.startup_state == "restore":
            self._begin_restore()
            # Wait for retained messages to arrive (paho delivers them on the
            # loop thread via _on_restore_msg).
            time.sleep(RESTORE_WAIT_SEC)
            self._finish_restore()
        elif self.startup_state == "on":
            self._force_all(True)
        else:  # "off"
            self._force_all(False)

        self.publish_discovery()
        self.client.publish(self.availability_topic, "online", qos=1, retain=True)
        self.publish_cached_state()
        for r in self.relays:
            self.client.subscribe(r.cmd_topic, qos=1)
        self._restored = True
        LOG.info("ready: %d channels across %d board(s)",
                 len(self.relays),
                 len({r.board_addr for r in self.relays}))

    def _reconnect(self) -> None:
        # Hardware already matches cache; re-advertise and re-subscribe only.
        self.publish_discovery()
        self.client.publish(self.availability_topic, "online", qos=1, retain=True)
        self.publish_cached_state()
        for r in self.relays:
            self.client.subscribe(r.cmd_topic, qos=1)
        LOG.info("reconnected; re-asserted availability and state")

    # ---- MQTT callbacks ------------------------------------------------

    def on_connect(self, _c, _ud, _flags, reason_code, _props=None):
        if reason_code != 0:
            LOG.error("MQTT connect failed: %s", reason_code)
            return
        LOG.info("MQTT connected")
        if self._restored:
            self._reconnect()
        else:
            # Run the (blocking) restore flow on a worker so paho's loop
            # thread stays free to deliver retained messages.
            threading.Thread(target=self._first_connect, name="restore",
                             daemon=True).start()

    def on_message(self, _c, _ud, msg):
        r = self.by_cmd.get(msg.topic)
        if r is None:
            return
        payload = msg.payload.decode(errors="replace").strip().upper()
        if payload not in ("ON", "OFF"):
            LOG.warning("ignoring payload %r on %s", payload, msg.topic)
            return
        logical = (payload == "ON")
        if self.bank.write(r, logical, reason="mqtt command"):
            self.client.publish(r.stat_topic, payload, qos=1, retain=True)


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main() -> int:
    with open(os.environ.get("OPTIONS_PATH", "/data/options.json")) as f:
        opts = json.load(f)

    base_topic = str(opts.get("base_topic", "dockerpi_relay")).strip("/")
    device_name = str(opts.get("device_name", "DockerPi Relay"))
    startup_state = str(opts.get("startup_state", "restore")).lower()
    if startup_state not in ("restore", "on", "off"):
        LOG.warning("unknown startup_state %r, falling back to 'restore'", startup_state)
        startup_state = "restore"
    bus_no = int(opts.get("i2c_bus", 1))

    relays = build_relays(opts, base_topic)
    bank = RelayBank(bus_no)
    LOG.info("configured %d relay(s) on bus %d across %s",
             len(relays), bus_no,
             sorted({f"0x{r.board_addr:02x}" for r in relays}))

    availability_topic = f"{base_topic}/availability"

    client = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id=f"dockerpi_relay_{base_topic}")
    user = os.environ.get("MQTT_USER") or None
    if user:
        client.username_pw_set(user, os.environ.get("MQTT_PASS") or None)
    client.will_set(availability_topic, "offline", qos=1, retain=True)
    client.reconnect_delay_set(min_delay=1, max_delay=30)

    bridge = Bridge(
        relays=relays, bank=bank, client=client,
        base_topic=base_topic, device_name=device_name,
        startup_state=startup_state, availability_topic=availability_topic,
    )
    client.on_connect = bridge.on_connect
    client.on_message = bridge.on_message

    host = os.environ["MQTT_HOST"]
    port = int(os.environ.get("MQTT_PORT", "1883"))
    LOG.info("connecting to MQTT %s:%d", host, port)
    # Async so a slow broker on boot doesn't kill us; paho will retry.
    client.connect_async(host, port, keepalive=30)

    stop = threading.Event()

    def shutdown(*_):
        LOG.info("shutting down")
        try:
            client.publish(availability_topic, "offline", qos=1,
                           retain=True).wait_for_publish(2)
        except Exception:
            pass
        stop.set()

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)

    client.loop_start()
    try:
        while not stop.is_set():
            stop.wait(1.0)
    finally:
        client.loop_stop()
        client.disconnect()
        bank.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
