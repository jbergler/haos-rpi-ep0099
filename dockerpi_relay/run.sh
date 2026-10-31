#!/usr/bin/with-contenv bashio
set -euo pipefail

export MQTT_HOST="$(bashio::services mqtt 'host')"
export MQTT_PORT="$(bashio::services mqtt 'port')"
export MQTT_USER="$(bashio::services mqtt 'username')"
export MQTT_PASS="$(bashio::services mqtt 'password')"

export OPTIONS_PATH=/data/options.json
bashio::log.info "Starting DockerPi Relay bridge"
exec python3 -u /relay.py
