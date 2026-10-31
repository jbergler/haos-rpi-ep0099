#!/usr/bin/with-contenv bashio
set -euo pipefail

export MQTT_HOST="$(bashio::services mqtt 'host')"
export MQTT_PORT="$(bashio::services mqtt 'port')"
export MQTT_USER="$(bashio::services mqtt 'username')"
export MQTT_PASS="$(bashio::services mqtt 'password')"

export I2C_BUS="$(bashio::config 'i2c_bus')"
export NUM_RELAYS="$(bashio::config 'num_relays')"
export BASE_TOPIC="$(bashio::config 'base_topic')"
export DEVICE_NAME="$(bashio::config 'device_name')"
export POLL_INTERVAL="$(bashio::config 'poll_interval')"
export I2C_ADDRESSES="$(bashio::config 'i2c_addresses | join(\",\")')"

bashio::log.info "Starting DockerPi Relay bridge on bus ${I2C_BUS}, addrs=${I2C_ADDRESSES}"
exec python3 -u /relay.py
