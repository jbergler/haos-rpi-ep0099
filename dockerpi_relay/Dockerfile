ARG BUILD_FROM
FROM $BUILD_FROM

RUN apk add --no-cache python3 py3-pip i2c-tools \
 && pip3 install --no-cache-dir --break-system-packages \
      smbus2==0.4.3 paho-mqtt==2.1.0

COPY run.sh /run.sh
COPY relay.py /relay.py
RUN chmod a+x /run.sh

CMD ["/run.sh"]
