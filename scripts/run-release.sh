#!/bin/sh
# Boot an igis-v2 release in QEMU.
#
# A release is four files that must stay together:
#   igis.iso   the kernel and GRUB (the ISO deliberately carries no applications)
#   apps.img   the read-only ext2 application volume (primary slave)
#   home.img   the writable ext2 /home volume (primary master; edits persist here)
#   run.sh     this script
#
# Usage:
#   ./run.sh                   4 cores, TCG, 256 MiB, a graphical window
#   ./run.sh --cores 1         single-core boot
#   ./run.sh --headless        no display; the serial log goes to stdout
#   ./run.sh --accel kvm       use KVM when the host allows it (also IGIS_ACCEL)
#   ./run.sh -m 512M           anything else is passed to qemu-system-x86_64
#
# The guest mounts apps.img read-only and QEMU opens it with snapshot=on, so the
# file on disk is never modified. home.img is attached writable: write it to a
# copy if you want to keep the seed pristine.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
self="$here/$(basename -- "$0")"
cd "$here"

usage() {
    sed -n '2,19p' "$self" | sed 's/^# \{0,1\}//'
}

cores=4
accel=${IGIS_ACCEL:-tcg}
headless=0
while [ $# -gt 0 ]; do
    case "$1" in
        --cores)
            if [ $# -lt 2 ]; then
                echo "run.sh: --cores needs a value" >&2
                exit 2
            fi
            cores=$2
            shift 2
            ;;
        --accel)
            if [ $# -lt 2 ]; then
                echo "run.sh: --accel needs a value" >&2
                exit 2
            fi
            accel=$2
            shift 2
            ;;
        --headless)
            headless=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            break
            ;;
        *)
            break
            ;;
    esac
done

for file in igis.iso apps.img home.img; do
    if [ ! -f "$file" ]; then
        echo "run.sh: missing $file (keep the release files next to this script)" >&2
        exit 1
    fi
done

if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
    echo "run.sh: qemu-system-x86_64 is not installed" >&2
    exit 1
fi

set -- \
    -cdrom igis.iso \
    -drive "file=home.img,format=raw,if=ide,index=0" \
    -drive "file=apps.img,format=raw,if=ide,index=1,snapshot=on" \
    -m 256M \
    -smp "$cores" \
    -accel "$accel" \
    -vga std \
    -rtc base=localtime \
    -usb \
    -device usb-tablet \
    -netdev user,id=n0 \
    -device virtio-net-pci,netdev=n0,disable-modern=on \
    -no-reboot \
    -no-shutdown \
    "$@"

if [ "$headless" = 1 ]; then
    set -- -display none -serial stdio "$@"
fi

exec qemu-system-x86_64 "$@"
