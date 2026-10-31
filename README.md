# HAOS add-on: DockerPi Relay (EP-0099)

Exposes the 52Pi / GeeekPi **DockerPi 4-channel I²C relay HAT** (`EP-0099`,
default I²C address `0x10`) as native Home Assistant switches via MQTT
discovery.

## Hardware

- Raspberry Pi 4B running Home Assistant OS.
- EP-0099 HAT seated on the 40-pin header.
- I²C bus `/dev/i2c-1`, device address `0x10` (DIP all off).
- Registers: `0x01`..`0x04` → relays 1..4. Write `0xFF` = on, `0x00` = off.

## One-time Pi setup

1. SSH into the Pi (or use the **Terminal & SSH** add-on) and edit
   `/boot/config.txt` on the HAOS boot partition. Append:
   ```
   dtparam=i2c_arm=on
   ```
   Reboot. Verify from any add-on with shell access:
   ```
   ls /dev/i2c-1
   i2cdetect -y 1   # should show "10" at row 10
   ```

## Install the add-on

Recommended (one-click): open the Home Assistant add-on store, **⋮ →
Repositories**, paste this repo's URL, then install *DockerPi Relay (EP-0099)*
from the new section.

[![Add repository to Home Assistant](https://my.home-assistant.io/badges/supervisor_add_addon_repository.svg)](https://my.home-assistant.io/redirect/supervisor_add_addon_repository/?repository_url=https%3A%2F%2Fgithub.com%2Fjbergler%2Fhaos-rpi-ep0099)

Manual (local add-on, no GitHub needed):

1. Install the **Mosquitto broker** add-on, start it, and configure the **MQTT**
   integration — the add-on auto-pulls credentials via the Supervisor services
   API, no manual broker config needed.
2. Install the **Samba share** or **SSH & Web Terminal** add-on so you can drop
   files into `/addons`.
3. Copy `dockerpi_relay/` from this repo to `/addons/dockerpi_relay/` on the Pi.
4. In HA: **Settings → Add-ons → Add-on Store → ⋮ → Check for updates**. A
   **Local add-ons** section appears with *DockerPi Relay (EP-0099)*.
5. Install, leave options at defaults for a single HAT, **Start**. Four
   `switch.relay_1`..`switch.relay_4` entities appear automatically under the
   *DockerPi Relay* device.

## Options

| Key              | Default            | Notes                                         |
| ---------------- | ------------------ | --------------------------------------------- |
| `i2c_bus`        | `1`                | `/dev/i2c-1`                                  |
| `i2c_addresses`  | `["0x10"]`         | Add more entries for stacked boards (0x11…). |
| `num_relays`     | `4`                | Channels per board.                           |
| `base_topic`     | `dockerpi_relay`   | MQTT topic root.                              |
| `device_name`    | `DockerPi Relay`   | Shown in HA device registry.                  |
| `poll_interval`  | `0`                | Seconds; `>0` re-publishes retained state.    |

## Behaviour

- On startup every relay is forced **OFF** so cached state matches hardware
  (the chip has no readback).
- MQTT LWT publishes `offline` to `<base_topic>/availability` if the add-on
  dies; entities go unavailable in HA.
- Discovery topics are retained, so a Mosquitto restart re-populates entities
  without restarting the add-on.

## Troubleshooting

- `i2cdetect -y 1` shows nothing → I²C not enabled in `config.txt`, or ribbon
  seated wrong.
- Entities appear but toggling does nothing → check add-on log for
  `I2C write failed`; usually DIP switches put the board on a different
  address, add it to `i2c_addresses`.
- `MQTT connect failed` → the Mosquitto add-on isn't running, or the MQTT
  integration hasn't been configured in HA (needed so the Supervisor advertises
  the service).
