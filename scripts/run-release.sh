#!/bin/sh
# Boot an igis-v2 release in QEMU.
#
# A release is four files that must stay together:
#   igis.iso       the kernel and GRUB (the ISO carries no applications)
#   apps.img.gz    the read-only ext2 application volume (primary slave)
#   home.img.gz    the writable ext2 /home seed (primary master)
#   run.sh         this script
#
# Usage:
#   ./run.sh                   4 cores, TCG, 256 MiB, a graphical window
#   ./run.sh --cores 1         single-core boot
#   ./run.sh --headless        no display; the serial log goes to stdout
#   ./run.sh --accel kvm       use KVM when the host allows it (also IGIS_ACCEL)
#   ./run.sh -m 512M           anything else is passed to qemu-system-x86_64
#   IGIS_AUDIO=none ./run.sh   sound backend: pa (default), pipewire, alsa, none
#
# apps.img.gz and home.img.gz are expanded once on first run; apps.img is
# opened with snapshot=on (never modified) and home.img is attached writable,
# so edits persist. gunzip is required only for that first run.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
self="$here/$(basename -- "$0")"
cd "$here"

usage() {
    sed -n '2,20p' "$self" | sed 's/^# \{0,1\}//'
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

# Both large images ship gzipped: the 256 MiB home is mostly holes and the PPM
# corpus roughly halves. Expand each once; never overwrite an existing image.
expand_image() {
    gz=$1
    img=${gz%.gz}
    [ -f "$img" ] && return 0
    [ -f "$gz" ] || return 0
    if ! command -v gunzip >/dev/null 2>&1; then
        echo "run.sh: gunzip is required to expand $gz" >&2
        exit 1
    fi
    echo "run.sh: expanding $gz (one time)"
    if ! gunzip -c "$gz" > "$img.tmp"; then
        rm -f "$img.tmp"
        echo "run.sh: cannot expand $gz" >&2
        exit 1
    fi
    mv "$img.tmp" "$img"
}
expand_image apps.img.gz
expand_image home.img.gz

for file in igis.iso apps.img home.img; do
    if [ ! -f "$file" ]; then
        echo "run.sh: missing $file (or its .gz; keep the release files next to this script)" >&2
        exit 1
    fi
done

if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
    echo "run.sh: qemu-system-x86_64 is not installed" >&2
    exit 1
fi

# Sound: use the PulseAudio/PipeWire server when one is reachable, otherwise a
# silent HDA device so the player still opens. IGIS_AUDIO overrides.
audio=${IGIS_AUDIO:-}
if [ -z "$audio" ]; then
    if command -v pactl >/dev/null 2>&1 && pactl info >/dev/null 2>&1; then
        audio=pa
    elif command -v pw-play >/dev/null 2>&1; then
        audio=pipewire
    else
        audio=none
    fi
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
    -audiodev "$audio,id=snd0" \
    -device ich9-intel-hda \
    -device hda-output,audiodev=snd0 \
    -no-reboot \
    -no-shutdown \
    "$@"

if [ "$headless" = 1 ]; then
    set -- -display none -serial stdio "$@"
fi

exec qemu-system-x86_64 "$@"
