# Build the NanoKVM Image

This recipe builds a bootable Debian Trixie image for the **Sipeed NanoKVM
PCIe**. It follows the rootfs-overlay approach used by
[`djdisodo/cv181x-debian`](https://github.com/djdisodo/cv181x-debian): a small
tree under `images/nanokvm/rootfs-overlay/` is copied into the target rootfs by
an `mmdebstrap` customize hook after the base Debian bootstrap.

The recipe installs `u-boot-sg2002-nanokvm-pcie-distroboot`. It intentionally
does not install either the LicheeRV Nano or Milk-V Duo U-Boot package.

> [!WARNING]
> The resulting firmware may be insecure. Video is transmitted without
> encryption, and the management UI uses plaintext HTTP. Build and deploy it
> only for trusted, isolated networks.

## Image layout

- Partition 1: FAT firmware partition, mounted at `/boot/firmware`.
- Partition 2: Btrfs root filesystem.
- Rootfs overlay: board configuration, Sodo repository key/source, module load
  order, and `nanokvm-host.service`.
- Root account: locked in the image; there is no default password.

The NanoKVM U-Boot package supplies `fip.bin` and distroboot configuration. The
CV181x kernel package supplies the NanoKVM PCIe device tree, including HDMI
capture, Wave420L, USB peripheral mode, and OLED hardware descriptions.

## Build host

Use an amd64 Debian or Ubuntu host. Building requires root privileges for loop
devices, mounts, `chroot`, and filesystem creation.

```sh
sudo apt-get update
sudo apt-get install -y \
  mmdebstrap qemu-user-static binfmt-support \
  parted dosfstools btrfs-progs rsync xz-utils \
  gcc-riscv64-linux-gnu curl ca-certificates
```

Install a current stable Rust toolchain for the normal build user:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add riscv64gc-unknown-linux-gnu
```

Verify foreign RISC-V execution before building the image:

```sh
test -e /proc/sys/fs/binfmt_misc/qemu-riscv64
qemu-riscv64-static --version
```

## Build

Build the host as the normal user first:

```sh
./scripts/build-nanokvm-riscv64.sh
```

Then create the disk image as root:

```sh
sudo env \
  HOST_BINARY="$PWD/target/riscv64gc-unknown-linux-gnu/release/nanokvm-host" \
  ./images/nanokvm/build-image.sh
```

Outputs are written to `dist/`:

```text
moonlink-nanokvm-debian.img
moonlink-nanokvm-debian.img.xz
moonlink-nanokvm-debian.img.sha256
```

The raw image is retained alongside the compressed image. Check available disk
space before starting.

## Build variables

The builder accepts environment overrides:

```sh
sudo env \
  HOST_BINARY=/absolute/path/to/nanokvm-host \
  OUTPUT_DIR=/var/tmp/moonlink-images \
  IMAGE_NAME=moonlink-test.img \
  ROOT_SIZE_MIB=3072 \
  ./images/nanokvm/build-image.sh
```

`HOST_BINARY` must be an executable RISC-V `nanokvm-host` binary. If omitted,
the script invokes `scripts/build-nanokvm-riscv64.sh` as root, which is usually
less convenient when Rust was installed only for a normal user.

## Overlay contents

```text
images/nanokvm/rootfs-overlay/
├── etc/apt/sources.list.d/moonlink-sodo.list
├── etc/fstab
├── etc/hostname
├── etc/hosts
├── etc/initramfs-tools/
├── etc/kernel/cmdline
├── etc/modules-load.d/moonlink-camera.conf
├── etc/systemd/system/nanokvm-host.service
└── usr/share/keyrings/sodo-repo.asc
```

To customize the image, add files under this tree using their final absolute
paths. Do not put credentials, pairing data, private keys, or a root password
in the overlay.

## Installed hardware packages

- `linux-image-cv181x-sodoport`
- `linux-headers-cv181x-sodoport`
- `u-boot-sg2002-nanokvm-pcie-distroboot`
- `firmware-cnm-wave420l`
- `lt6911uxe-nanokvm-dkms`
- `cv181x-camera-dkms`

The Sodo repository is external to this project. Review and pin package
versions if reproducible or supply-chain-controlled builds are required.

## Root password policy

The build script runs `passwd --lock root` and never calls `chpasswd`. Initial
password setup happens after boot through the OLED recovery code described in
the [user guide](USER_GUIDE.md).
