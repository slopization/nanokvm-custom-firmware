# NanoKVM Test Host

> [!WARNING]
> This host is experimental. Stream encryption is configurable, but the
> management UI uses plaintext HTTP. See [`SECURITY.md`](../../SECURITY.md).

This example exposes a Moonlight/Sunshine-compatible host while leaving capture,
DMA, and HEVC encoding selectable at the edge. The native path configures V4L2
capture and encoder devices directly; the command path accepts Annex-B HEVC from
stdout for testing.

```sh
cargo build --release -p nanokvm-host

sudo ./target/release/nanokvm-host \
  --setup-usb-gadget \
  --capture-device /dev/video0 \
  --encoder-device /dev/video1 \
  --raw-fourcc UYVY \
  --codec hevc \
  --hid /dev/hidg0
```

Pairing is interactive. Start pairing in Moonlight, then enter the displayed
four-digit PIN beside the pending client under **Moonlight devices** in the
management UI.

Cross-build for a RISC-V GNU/Linux NanoKVM userspace:

```sh
./scripts/build-nanokvm-riscv64.sh
```

The output is
`target/riscv64gc-unknown-linux-gnu/release/nanokvm-host`. This is a
dynamically linked glibc/lp64d binary and expects the target to provide
`/lib/ld-linux-riscv64-lp64d.so.1`. The script uses Cargo's
`riscv64gc-unknown-linux-gnu` target plus `riscv64-linux-gnu-gcc`, and strips
the final binary when `riscv64-linux-gnu-strip` is available.

The native V4L2 encoder path trusts a compliant mem2mem encoder to return one
encoded access unit per capture buffer and uses `V4L2_BUF_FLAG_KEYFRAME` for
Moonlight's frame marker. The Annex-B splitter is still used by the external
command path.

`--gop` is applied to the native encoder with
`V4L2_CID_MPEG_VIDEO_GOP_SIZE`. Moonlight IDR requests and reference-frame
invalidation messages are distinct in `moonlink-core`, but this host handles
both with `V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME`.

`--bitrate-kbps` is optional. When it is set, the host enables
`V4L2_CID_MPEG_VIDEO_FRAME_RC_ENABLE` and writes
`V4L2_CID_MPEG_VIDEO_BITRATE`. When it is omitted, both controls are left at the
driver's default.

`--setup-usb-gadget` creates one configfs USB gadget with a combined HID
endpoint on `/dev/hidg0`, selectable UAC1 or UAC2 audio, and removable mass
storage. Its HID reports include keyboard, relative mouse, absolute mouse, and
touchscreen input. UAC1 is the default for PlayStation compatibility; select
UAC2 in the management UI or use `--usb-audio-class uac2`. Changing the class
disconnects and re-enumerates the USB gadget, restarts the service, and requires
signing in to the management UI again. If the gadget is already managed
elsewhere, omit that option and pass `--hid` explicitly.

The PiKVM virtual-media UI stores images under `--msd-dir` (default
`/var/lib/moonlink/msd`) and leaves the LUN disconnected at startup. Reliable
bulk transfers on CV1800/CV181x require gadget DMA in the DWC2 platform driver.
Linux currently forces both `g_dma` and `g_dma_desc` off for this SoC, while the
vendor kernel enables them. The userspace implementation is complete, but mass
storage should remain disconnected until that kernel limitation is fixed;
PIO-mode transfers can starve the single CPU and cause USB re-enumeration.

Audio defaults to USB capture and is encoded as the packet duration requested
by Moonlight. Use `--audio none` to advertise video only.

## Management UI

The management UI listens on `http://<device-ip>:8080` by default. On each USB
attachment, the SSD1306 display shows the device IP and a six-digit recovery
code for five minutes. Use that code to set the initial password or replace a
forgotten password. A recovery code expires after one use.

Stream and encoder settings are stored in
`/var/lib/moonlink/settings.json` with mode `0600`. Advertisement changes apply
immediately. Media changes disconnect the active client and are loaded when the
next RTSP session opens the V4L2 and ALSA devices. Changing the USB audio class
restarts the host service to rebuild the gadget descriptors. The UI
authenticates with the system root password, and both normal
password changes and OLED recovery-code changes update the root account through
`chpasswd`.

Stream encryption defaults to `opportunistic`: audio and video are encrypted
when requested by a compatible Moonlight client. Select `Required` in the UI or
use `--encryption-mode mandatory` to reject sessions that do not negotiate both
encrypted media channels. `--encryption-mode never` disables media encryption;
pairing and encrypted control traffic remain enabled.

To pair a Moonlight client, start pairing on the client and open **Moonlight
devices** in the web UI. Enter the four-digit PIN beside the pending device.
Each trusted client is stored by its certificate fingerprint, so any number of
phones, tablets, and desktop clients can remain paired. Devices can be revoked
individually from the same list.

Install `nanokvm-host.service` in `/etc/systemd/system` and enable it so the
host returns after a reboot:

```sh
sudo install -m 0644 examples/nanokvm-host/nanokvm-host.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now nanokvm-host.service
```

For the already-tested external stream path:

```sh
sudo ./target/release/nanokvm-host \
  --video-command 'your-capture-encoder --hevc --annex-b --aud --stdout' \
  --hid /dev/hidg0
```

Useful arguments include `--name`, `--pin`, `--bind-http`, `--bind-https`,
`--bind-rtsp`, `--bind-video`, `--bind-audio`, `--bind-control`,
`--buffer-count`, `--max-width`, `--max-height`, `--max-fps`,
`--bitrate-kbps`, `--gop`, `--audio`, `--usb-audio-class`, `--encryption-mode`,
`--no-repeat-headers`, `--no-aud`,
`--hid`, `--setup-usb-gadget`, `--msd-dir`, `--udc`, and
`--display-transform`.

For Linux V4L2 smoke testing with fake devices:

```sh
sudo ./examples/nanokvm-host/setup-v4l2-test-devices.sh
cargo run -p nanokvm-host --bin v4l2-smoke -- \
  --capture-device /dev/videoX \
  --codec-device /dev/videoY \
  --encoded-fourcc FWHT \
  --frames 30
```

`vicodec` is a virtual FWHT codec, not an H.264 or HEVC encoder, so it validates
the V4L2 queue and DMABUF path but cannot be streamed to Moonlight directly.
