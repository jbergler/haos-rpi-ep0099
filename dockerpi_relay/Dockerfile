# Prebuilt image: GH Actions cross-compiles the static musl binary and
# docker buildx assembles this image per arch. Supervisor pulls the
# resulting multi-arch manifest via config.yaml `image:` so there is NO
# build on the user's Pi.
#
# Expected build context contains a single file at ./dockerpi-relay
# (the already-compiled musl binary for the target architecture).

FROM scratch

LABEL org.opencontainers.image.title="Home Assistant Add-on: DockerPi Relay (EP-0099)"
LABEL org.opencontainers.image.description="I2C relay HAT (EP-0099) bridge with MQTT discovery"
LABEL org.opencontainers.image.source="https://github.com/jbergler/haos-rpi-ep0099"
LABEL org.opencontainers.image.licenses="MIT"

COPY dockerpi-relay /dockerpi-relay

ENTRYPOINT ["/dockerpi-relay"]
