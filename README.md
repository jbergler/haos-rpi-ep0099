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

Ships with a default `relays:` list for a single HAT at `0x10` (channels 1–4,
normally-open wiring). Edit it in the add-on's **Configuration** tab.

| Key              | Default            | Notes                                                                 |
| ---------------- | ------------------ | --------------------------------------------------------------------- |
| `i2c_bus`        | `1`                | `/dev/i2c-1`                                                          |
| `base_topic`     | `dockerpi_relay`   | MQTT topic root.                                                      |
| `device_name`    | `DockerPi Relay`   | Shown in HA device registry.                                          |
| `startup_state`  | `restore`          | `restore` / `off` / `on`. See **Behaviour**.                          |
| `poll_interval`  | `0`                | Seconds; `>0` re-publishes retained state (recovers from broker wipe).|
| `relays[]`       | 4× `0x10` ch 1–4   | One entry per channel. `name` and `wiring` optional.                  |

Each `relays[]` entry:

| Field     | Required | Values                                           |
| --------- | -------- | ------------------------------------------------ |
| `address` | yes      | `0x10` / `0x11` / `0x12` / `0x13` (DIP-selected) |
| `channel` | yes      | `1`–`4`                                          |
| `name`    | no       | Entity name in HA (default `Relay <n>`).         |
| `wiring`  | no       | `normally_open` (default) or `normally_closed`.  |

Stacked boards: add more entries (e.g. `address: "0x11"`, `channel: 1`). Names
then default to `Relay 0x11/1` form to disambiguate.

`wiring: normally_closed` inverts the I²C write so HA **ON** always means
"load energised", regardless of which contact the load is wired through.

## Behaviour

- **Startup (`restore`, default).** On the first MQTT connect, the add-on
  subscribes to its own state topics, waits up to 2 s for the broker to
  replay retained state, and writes it to the chip. Hardware ends up
  matching Home Assistant's last known intent across both cold reboots
  (chip reset by power loss) and warm restarts (chip kept its state, our
  cache was lost).
- **Startup (`off` / `on`).** Skip restore; force every channel to that
  state. Use `off` for safety-critical loads where "state uncertain after
  power event → treat as off" is the requirement.
- **Reconnect.** If Mosquitto bounces, the add-on only re-publishes
  availability + current cached state; it does **not** re-restore from
  retained (hardware already matches our cache).
- **Availability.** MQTT LWT publishes `offline` to `<base_topic>/availability`
  if the add-on crashes; entities go unavailable in HA.
- **Discovery.** Topics are retained, so a Mosquitto restart re-populates
  entities without restarting the add-on.

## Troubleshooting

- `i2cdetect -y 1` shows nothing → I²C not enabled in `config.txt`, or ribbon
  seated wrong.
- Entities appear but toggling does nothing → check add-on log for
  `I2C write failed`; usually DIP switches put the board on a different
  address — fix the `address` field on that `relays[]` entry.
- `MQTT connect failed` → Mosquitto add-on not running, or the MQTT
  integration hasn't been configured in HA (needed so Supervisor advertises
  the service to the add-on).
