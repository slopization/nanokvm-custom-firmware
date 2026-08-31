# Moonlink

Moonlink is an experimental Rust implementation of a Moonlight-compatible
streaming host. The workspace provides reusable protocol/server crates and a
NanoKVM PCIe firmware image that captures HDMI through V4L2, encodes HEVC with
the Wave420L codec, forwards Moonlight input through a USB HID gadget, and
captures target audio through UAC2.

> [!WARNING]
> This firmware is experimental, has not received a security audit, and may be
> insecure. Moonlight media encryption is configurable and opportunistic by
> default; select mandatory encryption when plaintext fallback is unacceptable.
> The management UI currently uses plaintext HTTP and handles the system root
> password. Do not expose it to an untrusted network.

## Documentation

- [Build the NanoKVM image](docs/BUILD_IMAGE.md)
- [Install and use the image](docs/USER_GUIDE.md)
- [Security limitations](SECURITY.md)
- [NanoKVM host development notes](examples/nanokvm-host/README.md)

## Workspace

- `moonlink-protocol`: GameStream packet, media, and input protocol types.
- `moonlink-core`: runtime-neutral server, media, pairing, and input traits.
- `moonlink-tokio`: asynchronous discovery, pairing, HTTPS, RTSP, control,
  video, and audio server implementation.
- `moonlink-v4l2`: asynchronous V4L2 queue and DMABUF support.
- `basic-server`: synthetic external-stream example.
- `nanokvm-host`: V4L2, Wave420L, UAC2, combined HID, OLED, and management UI
  integration.

## Development

Install a current stable Rust toolchain, then run:

```sh
cargo test --workspace
cargo run -p basic-server
```

Cross-build the NanoKVM host with:

```sh
rustup target add riscv64gc-unknown-linux-gnu
sudo apt-get install gcc-riscv64-linux-gnu
./scripts/build-nanokvm-riscv64.sh
```

The resulting binary is
`target/riscv64gc-unknown-linux-gnu/release/nanokvm-host`.

## Status

Moonlink is not a drop-in Sunshine replacement. The tested NanoKVM profile is
currently fixed to one active streaming session and a V4L2 capture/encoder
pipeline. Compatibility outside the tested Moonlight clients and NanoKVM PCIe
hardware is not guaranteed.

## License

GPL-3.0-or-later.
