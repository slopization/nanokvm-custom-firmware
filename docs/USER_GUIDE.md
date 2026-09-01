# NanoKVM Image User Guide

> [!WARNING]
> This firmware is experimental and may be insecure. Moonlight video is sent
> without encryption. The management UI uses plaintext HTTP, including during
> root password setup. Use an isolated, trusted network and never expose these
> services directly to the Internet.

## Flash the image

Verify the checksum:

```sh
sha256sum -c moonlink-nanokvm-debian.img.sha256
```

Identify the removable device carefully. The following command destroys all
data on the selected device:

```sh
xzcat moonlink-nanokvm-debian.img.xz | \
  sudo dd of=/dev/sdX bs=4M iflag=fullblock oflag=direct status=progress
sync
```

Replace `/dev/sdX` with the whole microSD device, not a partition. Insert it in
the NanoKVM PCIe after `dd` completes.

## Cable and boot order

1. Connect Ethernet to a trusted DHCP-enabled management network.
2. Connect the NanoKVM HDMI input to the target computer.
3. Connect the NanoKVM USB device port to the target computer.
4. Power the NanoKVM and wait for Debian to boot.

The service creates one USB gadget containing keyboard, relative mouse,
absolute pointer, touchscreen, and UAC2 audio capture functions.

## Set the initial password

The image has no default root password. The root account starts locked.

When USB reaches the configured state, the OLED displays the device IPv4
address and a six-digit recovery code for five minutes.

1. From a trusted machine on the same LAN, open `http://DEVICE-IP:8080/`.
2. Use the recovery form and enter the six-digit OLED code.
3. Set a root password of 8 to 128 characters.
4. Store the password securely. It is also the management UI password.

The code is single-use. Disconnect and reconnect USB to request another code.
Because this page is HTTP, anyone observing the network can potentially read
the code and password.

After setting the password, SSH is available as `root` using the same password:

```sh
ssh root@DEVICE-IP
```

There is no default SSH password, and the locked factory account cannot log in
before password recovery completes.

## Pair Moonlight

1. Install Moonlight on the client.
2. Add the NanoKVM by its IPv4 address.
3. Start pairing in Moonlight and note its four-digit PIN.
4. Open the NanoKVM management UI and select **Moonlight devices**.
5. Enter the PIN beside the pending client.
6. Start the advertised desktop application from Moonlight.

Multiple clients can remain paired. Only one active media session is currently
supported. Revoke old clients from **Moonlight devices**.

## Configure streaming

The Web UI can change the advertised mode, codec settings, GOP, bitrate control,
and FEC. Settings are stored in `/var/lib/moonlink/settings.json`.

- Video bitrate defaults to 1000 kbps. Clear it to retain the encoder driver's
  automatic mode.
- Video FEC defaults to zero to reduce CPU use and latency.
- Stream-setting changes disconnect the active Moonlight session and take
  effect on the next connection.
- Capture and encoder devices open only while a media session is active.

The default image service expects:

```text
capture: /dev/video0, UYVY, 1920x1080 at 60 FPS
encoder: /dev/video1, HEVC
audio:   UAC2, 48 kHz stereo
HID:     combined /dev/hidg0 gadget
OLED:    /dev/dri/card0, mirrored horizontally
```

Edit `nanokvm-host.service` only if device numbering or the physical display
orientation differs.

## Network ports

| Protocol | Port | Purpose |
| --- | ---: | --- |
| TCP | 8080 | Management UI, plaintext HTTP |
| TCP | 47984 | GameStream HTTPS |
| TCP/UDP | 47989 | GameStream HTTP and discovery |
| UDP | 47998 | Video |
| UDP | 47999 | Encrypted control and input |
| UDP | 48000 | Audio |
| TCP | 48010 | RTSP session setup |

Allow these only between the NanoKVM and trusted clients.

## Service operation

From a local serial console or an SSH session configured by the administrator:

```sh
systemctl status nanokvm-host.service
journalctl -u nanokvm-host.service -f
systemctl restart nanokvm-host.service
```

Persistent state is stored under `/var/lib/moonlink/`:

- `pairing.json`: server identity and trusted Moonlight clients.
- `settings.json`: persistent stream settings.

Back up this directory before replacing the image. Protect the backup because
it contains pairing identity material.

## Troubleshooting

No IP or recovery code:

- Confirm Ethernet received a DHCP lease with `networkctl` or `nmcli`.
- Confirm USB is connected to the target and the UDC state is `configured`.
- Check `/sys/class/udc/*/state` and the service journal.

Moonlight reports RTSP or UDP port errors:

- Check the firewall and the port table above.
- Confirm `ss -lntup` shows the service listeners.
- Restart the service to clear a stale test session.

No video:

```sh
v4l2-ctl --all -d /dev/video0
v4l2-ctl --all -d /dev/video1
media-ctl -p
dmesg | grep -Ei 'lt6911|cv181x|wave4|vcodec'
```

No audio or stuttering audio:

```sh
cat /proc/asound/cards
cat /proc/asound/card*/pcm*/sub*/status
```

An `XRUN` state indicates that UAC2 capture overran. Reconnecting Moonlight or
restarting the service resets the media session.

## Reflash and recovery

The management UI can reboot the KVM. It cannot repair an unbootable image. For
boot failures, use the 115200 baud serial console or reflash the microSD card.
Reflashing removes passwords, settings, and pairings unless they were backed up
separately.
