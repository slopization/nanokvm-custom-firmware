# NanoKVM Host

> [!WARNING]
> This host is experimental. Stream encryption is configurable, but the
> management UI uses plaintext HTTP. See [`SECURITY.md`](../../SECURITY.md).

This workspace crate provides the production NanoKVM PCIe daemon. It exposes a
Moonlight-compatible host and a browser KVM while leaving capture, DMA, and HEVC
encoding selectable at the edge. The native path configures V4L2 capture and
encoder devices directly; the command path accepts Annex-B HEVC from stdout for
testing.

```sh
cargo build --release -p nanokvm-host

sudo ./target/release/nanokvm-host \
  --setup-usb-gadget \
  --capture-device /dev/video0 \
  --encoder-device /dev/video1 \
  --raw-fourcc UYVY \
  --codec hevc
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

Video bitrate defaults to 1000 kbps. When `--bitrate-kbps` is set, the host
enables `V4L2_CID_MPEG_VIDEO_FRAME_RC_ENABLE` and writes
`V4L2_CID_MPEG_VIDEO_BITRATE`. Clearing the bitrate in the management UI leaves
both controls at the encoder driver's default.

`--setup-usb-gadget` creates a configfs USB gadget with separate standard HID
interfaces for keyboard, relative mouse, and absolute mouse on `/dev/hidg0`,
`/dev/hidg1`, and `/dev/hidg2`, plus selectable UAC1 or UAC2 audio and removable
mass storage. Keeping relative and absolute mouse endpoints separate prevents
PiKVM absolute-pointer reports from changing how the target interprets
Moonlight relative-mouse reports. UAC1 is the default for PlayStation
compatibility; select UAC2 in the management UI or use `--usb-audio-class
uac2`. Changing the class disconnects and re-enumerates the USB gadget, restarts
the service, and requires signing in to the management UI again. If the gadget
is already managed elsewhere, omit that option and pass `--keyboard-hid`,
`--mouse-hid`, and `--absolute-mouse-hid` explicitly.

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

The PiKVM UI listens on `http://<device-ip>:8080` by default. Open **System**
and then **NanoKVM** for host, stream, audio, Moonlight pairing, password, and
reboot controls. On each USB attachment, the SSD1306 display shows the device
IP and a six-digit recovery code for five minutes. On the PiKVM login page,
enter that code and the desired new password to reset the root password and
sign in. Leave the recovery-code field empty for normal login. A recovery code
expires after one use.

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

To pair a Moonlight client, start pairing on the client and open **System**,
**NanoKVM**, then **Moonlight devices**. Enter the four-digit PIN beside the pending device.
Each trusted client is stored by its certificate fingerprint, so any number of
phones, tablets, and desktop clients can remain paired. Devices can be revoked
individually from the same list.

Install `nanokvm-host.service` in `/etc/systemd/system` and enable it so the
host returns after a reboot:

```sh
sudo install -m 0644 deploy/systemd/nanokvm-host.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now nanokvm-host.service
```

For the already-tested external stream path:

```sh
sudo ./target/release/nanokvm-host \
  --video-command 'your-capture-encoder --hevc --annex-b --aud --stdout' \
  --keyboard-hid /dev/hidg0 \
  --mouse-hid /dev/hidg1 \
  --absolute-mouse-hid /dev/hidg2
```

Useful arguments include `--name`, `--pin`, `--bind-http`, `--bind-https`,
`--bind-rtsp`, `--bind-video`, `--bind-audio`, `--bind-control`,
`--buffer-count`, `--max-width`, `--max-height`, `--max-fps`,
`--bitrate-kbps`, `--gop`, `--audio`, `--usb-audio-class`, `--encryption-mode`,
`--no-repeat-headers`, `--no-aud`,
`--keyboard-hid`, `--mouse-hid`, `--absolute-mouse-hid`, `--setup-usb-gadget`,
`--msd-dir`, `--udc`, and
`--display-transform`.

For Linux V4L2 smoke testing with fake devices:

```sh
sudo ./crates/nanokvm-host/setup-v4l2-test-devices.sh
cargo run -p nanokvm-host --bin v4l2-smoke -- \
  --capture-device /dev/videoX \
  --codec-device /dev/videoY \
  --encoded-fourcc FWHT \
  --frames 30
```

`vicodec` is a virtual FWHT codec, not an H.264 or HEVC encoder, so it validates
the V4L2 queue and DMABUF path but cannot be streamed to Moonlight directly.
