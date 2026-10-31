#!/usr/bin/env python3
"""One-shot BLE sniffer: prints every advertisement with manufacturer 0x0067
(the one Safiery STAR-Tank uses). Run from any environment with bleak + host BT.

Usage:
    pip install bleak
    sudo python3 scan_startank.py           # scan 30 s
    sudo python3 scan_startank.py 120       # scan 120 s

On HAOS: install the "Advanced SSH & Web Terminal" add-on with protection
disabled, then `pip install bleak && python3 scan_startank.py`.
"""
from __future__ import annotations

import asyncio
import sys
from bleak import BleakScanner

MANUF_ID = 0x0067  # 'GN Hearing', used by Safiery


def decode(data: bytes) -> dict:
    """Mirror of ldenisey/venus-os-dbus-ble-sensors-py ble_device_safiery.py."""
    if len(data) != 10:
        return {}
    hw_id = data[0] & 0x7F
    battery_v = (data[1] & 0x7F) / 32
    temp_c = (data[2] & 0x7F) - 40
    sync_btn = (data[2] >> 7) & 0x01
    raw_u16 = data[3] | (data[4] << 8)
    raw_distance_cm = (raw_u16 & 0x3FFF) / 10
    nic = data[5:8].hex(":").upper()
    ax = int.from_bytes(data[8:9], "little", signed=True) / 1024
    ay = int.from_bytes(data[9:10], "little", signed=True) / 1024
    return {
        "hw_id": hw_id,
        "battery_v": round(battery_v, 2),
        "temp_c": temp_c,
        "sync_btn": sync_btn,
        "distance_cm": raw_distance_cm,
        "nic_last3": nic,
        "accel_x_g": round(ax, 3),
        "accel_y_g": round(ay, 3),
    }


def on_adv(device, adv):
    data = adv.manufacturer_data.get(MANUF_ID)
    if not data:
        return
    decoded = decode(data) or {"raw": data.hex()}
    print(f"{device.address}  rssi={adv.rssi}  {decoded}")


async def main(duration: float) -> None:
    print(f"Scanning for Safiery STAR-Tank adverts (manufacturer 0x{MANUF_ID:04x}) "
          f"for {duration:.0f}s...  Ctrl-C to stop early.")
    scanner = BleakScanner(detection_callback=on_adv)
    await scanner.start()
    try:
        await asyncio.sleep(duration)
    finally:
        await scanner.stop()


if __name__ == "__main__":
    secs = float(sys.argv[1]) if len(sys.argv) > 1 else 30.0
    try:
        asyncio.run(main(secs))
    except KeyboardInterrupt:
        pass
