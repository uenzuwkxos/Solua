#!/bin/sh
set -e
# load the I2C module now and apply the udev rule without a reboot (best effort)
modprobe i2c-dev 2>/dev/null || true
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --subsystem-match=i2c-dev 2>/dev/null || true
exit 0
