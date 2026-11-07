#!/bin/bash
# Kök dosya sistemini yeniden üretir.
# Kullanim: tools/dev/rootfs.sh [gui|console]
set -eu

cd "$(dirname "$0")/../.."

mode="${1:-console}"

gcc -static -no-pie -O2 -o build/rootdir/bin/gui-terminal tools/gui/terminal.c -I tools/gui
gcc -static -no-pie -O2 -o build/rootdir/bin/init tools/gui/init.c
gcc -static -no-pie -O2 -o build/rootdir/bin/unixtest tools/tests/unixtest.c
gcc -static -no-pie -O2 -o build/rootdir/bin/fswrite tools/tests/fswrite.c
if [ ! -f build/sdl2/lib/libSDL2.a ]; then
    tools/dev/sdl2.sh
fi
gcc -static -no-pie -O2 -o build/rootdir/bin/sdl2-demo tools/gui/sdl2-demo.c \
    -I build/sdl2/include build/sdl2/lib/libSDL2.a -lpthread -ldl -lm
if [ ! -f build/rootdir/usr/share/font8x16.bmp ]; then
    python3 tools/dev/mkfont.py build/rootdir/usr/share/font8x16.bmp
fi
printf '#!/bin/sh\nexport SDL_VIDEODRIVER=evdev\nexport SDL_EVDEV_DEVICES=2:/dev/input/event0\nexec /bin/sdl2-demo\n' \
    > build/rootdir/bin/sdl2run
chmod +x build/rootdir/bin/sdl2run

if [ -x /usr/bin/dbus-daemon ]; then
    tools/dev/addbin.sh /usr/bin/dbus-daemon /usr/bin/dbus-send
    install -D -m644 tools/dev/dbus-session.conf build/rootdir/etc/dbus-session.conf
fi

if [ "$mode" = "gui" ]; then
    touch build/rootdir/etc/gui-enabled
else
    rm -f build/rootdir/etc/gui-enabled
fi

truncate -s 64M build/rootfs.img
mke2fs -q -t ext4 -b 1024 -I 256 -O ^metadata_csum,^metadata_csum_seed,^64bit,^has_journal,^orphan_file -d build/rootdir -F build/rootfs.img
echo "rootfs: $mode kipinde hazir"
