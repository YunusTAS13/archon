#!/bin/bash
# QEMU'yu arka planda baslatir, istenen tuslari yollar ve ekran goruntusu alir.
# Kullanim: tools/dev/gui-shot.sh [cikti.png] [tus...]
set -u

cd "$(dirname "$0")/../.." || exit 1

output="${1:-/tmp/opencode/screen.png}"
shift || true
keys=("$@")

pkill -f '^qemu-system' 2>/dev/null
sleep 1
rm -f /tmp/opencode/mon.sock /tmp/opencode/screen.ppm build/serial.log

nohup qemu-system-x86_64 \
    -M q35 -m 2G -smp 1 -accel kvm -cpu host \
    -cdrom build/archon.iso -boot d \
    -serial file:build/serial.log \
    -display none \
    -drive file=build/rootfs.img,if=none,id=disk0,format=raw \
    -device virtio-blk-pci,drive=disk0,disable-legacy=on \
    -monitor unix:/tmp/opencode/mon.sock,server,nowait \
    -no-reboot >/tmp/opencode/qemu.err 2>&1 &

sleep "${ARCHON_BOOT_WAIT:-12}"

for key in "${keys[@]}"; do
    echo "sendkey $key" | socat - UNIX-CONNECT:/tmp/opencode/mon.sock >/dev/null 2>&1
    sleep 0.15
done

sleep 2
echo "screendump /tmp/opencode/screen.ppm" | socat - UNIX-CONNECT:/tmp/opencode/mon.sock >/dev/null 2>&1
sleep 1

python3 - "$output" <<'PY'
import sys
from PIL import Image
Image.open('/tmp/opencode/screen.ppm').save(sys.argv[1])
print(sys.argv[1])
PY
