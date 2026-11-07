#!/bin/bash
# Konağın ikili dosyalarını ve paylaşılan kütüphanelerini kök görüntüye kopyalar.
# Kullanım: tools/dev/addbin.sh /usr/bin/dbus-daemon ...
set -eu

cd "$(dirname "$0")/../.."

for binary in "$@"; do
    install -D -m755 "$binary" "build/rootdir/usr/bin/$(basename "$binary")"
    ldd "$binary" 2>/dev/null | while read -r line; do
        for field in $line; do
            case "$field" in
                /*)
                    if [ -e "$field" ]; then
                        install -D -m755 "$field" "build/rootdir$field"
                    fi
                    ;;
            esac
        done
    done
done
