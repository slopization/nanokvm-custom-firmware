#!/bin/sh
set -eu

GADGET_DIR="${GADGET_DIR:-/sys/kernel/config/usb_gadget/moonlink}"
UDC="${UDC:-}"
VID="${VID:-0x1d6b}"
PID="${PID:-0x0104}"
SERIAL="${SERIAL:-moonlink-nanokvm}"
MANUFACTURER="${MANUFACTURER:-moonlink}"
PRODUCT="${PRODUCT:-NanoKVM Moonlight HID}"

if [ ! -d /sys/kernel/config/usb_gadget ]; then
    mount -t configfs none /sys/kernel/config
fi

mkdir -p "$GADGET_DIR"
cd "$GADGET_DIR"

if [ -f UDC ] && [ "$(cat UDC)" != "" ]; then
    echo "" > UDC
fi

echo "$VID" > idVendor
echo "$PID" > idProduct
echo 0x0100 > bcdDevice
echo 0x0200 > bcdUSB

mkdir -p strings/0x409 configs/c.1/strings/0x409
echo "$SERIAL" > strings/0x409/serialnumber
echo "$MANUFACTURER" > strings/0x409/manufacturer
echo "$PRODUCT" > strings/0x409/product
echo "HID" > configs/c.1/strings/0x409/configuration
echo 250 > configs/c.1/MaxPower

rm -f configs/c.1/hid.usb0 configs/c.1/hid.usb1
rmdir functions/hid.usb1 2>/dev/null || true
mkdir -p functions/hid.usb0

echo 0 > functions/hid.usb0/protocol
echo 0 > functions/hid.usb0/subclass
echo 9 > functions/hid.usb0/report_length
printf '\x05\x01\x09\x06\xa1\x01\x85\x01\x05\x07\x19\xe0\x29\xe7\x15\x00\x25\x01\x75\x01\x95\x08\x81\x02\x95\x01\x75\x08\x81\x01\x95\x06\x75\x08\x15\x00\x25\x65\x05\x07\x19\x00\x29\x65\x81\x00\xc0\x05\x01\x09\x02\xa1\x01\x85\x02\x09\x01\xa1\x00\x05\x09\x19\x01\x29\x08\x15\x00\x25\x01\x95\x08\x75\x01\x81\x02\x05\x01\x09\x30\x09\x31\x16\x00\x00\x26\xff\x7f\x75\x10\x95\x02\x81\x02\x09\x38\x15\x81\x25\x7f\x75\x08\x95\x01\x81\x06\xc0\xc0' > functions/hid.usb0/report_desc

ln -sf functions/hid.usb0 configs/c.1/

if [ -z "$UDC" ]; then
    UDC="$(ls /sys/class/udc | head -n 1)"
fi

if [ -z "$UDC" ]; then
    echo "no UDC found; gadget configured but not bound" >&2
    exit 1
fi

echo "$UDC" > UDC
echo "registered $PRODUCT on UDC $UDC"
