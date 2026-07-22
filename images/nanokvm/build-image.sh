#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd -- "$SCRIPT_DIR/../.." && pwd)
OVERLAY="$SCRIPT_DIR/rootfs-overlay"
OUTPUT_DIR=${OUTPUT_DIR:-"$REPO_ROOT/dist"}
IMAGE_NAME=${IMAGE_NAME:-moonlink-nanokvm-debian.img}
IMAGE="$OUTPUT_DIR/$IMAGE_NAME"
BOOT_START_MIB=${BOOT_START_MIB:-4}
BOOT_SIZE_MIB=${BOOT_SIZE_MIB:-32}
ROOT_SIZE_MIB=${ROOT_SIZE_MIB:-2048}
BOOT_END_MIB=$((BOOT_START_MIB + BOOT_SIZE_MIB))
ROOT_END_MIB=$((BOOT_END_MIB + ROOT_SIZE_MIB))
IMAGE_SIZE_MIB=$((ROOT_END_MIB + 1))
ROOT_MNT=
LOOP=

unmount_chroot_filesystems() {
    if [[ -n "$ROOT_MNT" ]]; then
        mountpoint -q "$ROOT_MNT/dev" && umount -R "$ROOT_MNT/dev" || true
        mountpoint -q "$ROOT_MNT/sys" && umount -R "$ROOT_MNT/sys" || true
        mountpoint -q "$ROOT_MNT/proc" && umount "$ROOT_MNT/proc" || true
    fi
}

cleanup() {
    unmount_chroot_filesystems
    if [[ -n "$ROOT_MNT" ]]; then
        mountpoint -q "$ROOT_MNT/boot/firmware" && umount "$ROOT_MNT/boot/firmware" || true
        mountpoint -q "$ROOT_MNT" && umount "$ROOT_MNT" || true
    fi
    if [[ -n "$LOOP" ]]; then
        losetup -d "$LOOP" 2>/dev/null || true
    fi
    if [[ "$ROOT_MNT" == /tmp/moonlink-image.* ]]; then
        rm -rf -- "$ROOT_MNT"
    fi
}
trap cleanup EXIT

if [[ $EUID -ne 0 ]]; then
    echo "run this script as root" >&2
    exit 1
fi

required=(
    btrfs
    losetup
    mmdebstrap
    mkfs.vfat
    parted
    qemu-riscv64-static
    rsync
    systemctl
    xz
)
for command in "${required[@]}"; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command not found: $command" >&2
        exit 1
    fi
done

BINARY=${HOST_BINARY:-"$REPO_ROOT/target/riscv64gc-unknown-linux-gnu/release/nanokvm-host"}
if [[ -z ${HOST_BINARY:-} ]]; then
    for command in cargo riscv64-linux-gnu-gcc; do
        if ! command -v "$command" >/dev/null 2>&1; then
            echo "required command not found: $command" >&2
            exit 1
        fi
    done
    "$REPO_ROOT/scripts/build-nanokvm-riscv64.sh"
fi
if [[ ! -x "$BINARY" ]]; then
    echo "cross-built host binary not found: $BINARY" >&2
    exit 1
fi

install -d "$OUTPUT_DIR"
ROOT_MNT=$(mktemp -d /tmp/moonlink-image.XXXXXX)

: >"$IMAGE"
truncate -s "$((IMAGE_SIZE_MIB * 1024 * 1024))" "$IMAGE"
parted -s "$IMAGE" mklabel msdos
parted -s "$IMAGE" mkpart primary fat32 "${BOOT_START_MIB}MiB" "${BOOT_END_MIB}MiB"
parted -s "$IMAGE" set 1 lba on
parted -s "$IMAGE" mkpart primary btrfs "${BOOT_END_MIB}MiB" "${ROOT_END_MIB}MiB"
parted -s "$IMAGE" set 2 boot on

LOOP=$(losetup --find --show --partscan "$IMAGE")
mkfs.vfat -n FIRMWARE "${LOOP}p1"
mkfs.btrfs -L rootfs -f "${LOOP}p2"
mount "${LOOP}p2" "$ROOT_MNT"
install -d "$ROOT_MNT/boot/firmware"
mount "${LOOP}p1" "$ROOT_MNT/boot/firmware"

mmdebstrap \
    --mode=root \
    --skip=check/empty \
    --architectures=riscv64 \
    --variant=minbase \
    --include='systemd-sysv,systemd-timesyncd,fake-hwclock,openssh-server,network-manager,netbase,kmod,btrfs-progs,ca-certificates,libcrypt1' \
    --customize-hook="sync-in $OVERLAY /" \
    trixie \
    "$ROOT_MNT" \
    'deb https://deb.debian.org/debian trixie main non-free-firmware' \
    'deb https://deb.debian.org/debian trixie-updates main non-free-firmware' \
    'deb https://security.debian.org/debian-security trixie-security main non-free-firmware'

mount -t proc proc "$ROOT_MNT/proc"
mount --rbind /sys "$ROOT_MNT/sys"
mount --make-rslave "$ROOT_MNT/sys"
mount --rbind /dev "$ROOT_MNT/dev"
mount --make-rslave "$ROOT_MNT/dev"

chroot "$ROOT_MNT" apt-get update
chroot "$ROOT_MNT" /usr/bin/env DEBIAN_FRONTEND=noninteractive apt-get install -y \
    linux-image-cv181x-sodoport \
    linux-headers-cv181x-sodoport \
    u-boot-sg2002-nanokvm-pcie-distroboot \
    firmware-cnm-wave420l \
    lt6911uxe-nanokvm-dkms \
    cv181x-camera-dkms \
    v4l-utils \
    i2c-tools

install -Dm0755 "$BINARY" "$ROOT_MNT/usr/local/bin/nanokvm-host"
install -d -m0700 "$ROOT_MNT/var/lib/moonlink"
systemctl --root="$ROOT_MNT" enable nanokvm-host.service

# Keep the factory image credential-free. The OLED recovery flow sets root's
# first password through the local management UI after USB attachment.
chroot "$ROOT_MNT" passwd --lock root

KERNEL_IMAGE=$(find "$ROOT_MNT/boot" -maxdepth 1 -name 'vmlinuz-*' -print | sort -V | tail -n 1)
if [[ -z "$KERNEL_IMAGE" ]]; then
    echo "installed kernel image was not found" >&2
    exit 1
fi
KERNEL_VERSION=${KERNEL_IMAGE##*/vmlinuz-}
chroot "$ROOT_MNT" /etc/kernel/postinst.d/zz-u-boot-menu "$KERNEL_VERSION"
chroot "$ROOT_MNT" apt-get clean

if [[ $(chroot "$ROOT_MNT" dpkg-query -W -f='${db:Status-Status}' \
    u-boot-sg2002-nanokvm-pcie-distroboot) != installed ]]; then
    echo "NanoKVM PCIe U-Boot package is not installed" >&2
    exit 1
fi
if chroot "$ROOT_MNT" dpkg-query -W -f='${binary:Package}\n' 2>/dev/null \
    | grep -Eq '^u-boot-(sg2002-licheerv-nano|cv1800b-milkv-duo)'; then
    echo "an incompatible LicheeRV Nano or Milk-V Duo U-Boot package is installed" >&2
    exit 1
fi
if [[ ! -s "$ROOT_MNT/boot/firmware/fip.bin" ]]; then
    echo "NanoKVM PCIe FIP was not installed to /boot/firmware/fip.bin" >&2
    exit 1
fi
if [[ ! -s "$ROOT_MNT/boot/extlinux/extlinux.conf" ]]; then
    echo "U-Boot boot menu was not generated" >&2
    exit 1
fi
ROOT_PASSWORD_FIELD=$(awk -F: '$1 == "root" { print $2 }' "$ROOT_MNT/etc/shadow")
if [[ $ROOT_PASSWORD_FIELD != '!'* && $ROOT_PASSWORD_FIELD != '*'* ]]; then
    echo "root account is not locked" >&2
    exit 1
fi

sync
unmount_chroot_filesystems
umount "$ROOT_MNT/boot/firmware"
umount "$ROOT_MNT"
losetup -d "$LOOP"
LOOP=

xz -T0 -f -k "$IMAGE"
(
    cd "$OUTPUT_DIR"
    sha256sum "$IMAGE_NAME" "$IMAGE_NAME.xz" >"$IMAGE_NAME.sha256"
)

echo "built $IMAGE"
echo "built $IMAGE.xz"
echo "checksums: $IMAGE.sha256"
