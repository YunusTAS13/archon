#!/bin/bash
# SDL2'yi kaynaktan derler (statik) ve build/sdl2 altina kurar.
#
# Neden SDL2: SDL3'un Linux "evdev" video surucusu yalnizca dokunmatik okur;
# klavye/fare icin X11 veya Wayland bekler. SDL2'nin "evdev" surucusu ise
# sahte video + gercek /dev/input/event* okuyucudur, framebuffer ile calisir.
#
# Kullanim: tools/dev/sdl2.sh
set -eu

cd "$(dirname "$0")/../.."

version="${SDL2_VERSION:-2.32.10}"
source_dir="build/src/SDL2-$version"
target="build/sdl2"

if [ -f "$target/lib/libSDL2.a" ]; then
    echo "sdl2: hazir ($target/lib/libSDL2.a)"
    exit 0
fi

mkdir -p build/src "$target/lib" "$target/include"

if [ ! -d "$source_dir" ]; then
    echo "sdl2: kaynak indiriliyor ($version)"
    curl -sL -o "build/src/SDL2-$version.tar.gz" \
        "https://libsdl.org/release/SDL2-$version.tar.gz"
    tar xzf "build/src/SDL2-$version.tar.gz" -C build/src
fi

echo "sdl2: yapilandiriliyor"
(
    cd "$source_dir"
    ./configure --disable-shared --enable-static \
        --disable-libudev --disable-dbus \
        --disable-audio --disable-joystick --disable-haptic --disable-sensor \
        --disable-power --disable-ime --disable-libsamplerate \
        --disable-video-x11 --disable-video-wayland --disable-video-kmsdrm \
        --disable-video-vivante --disable-video-directfb \
        --disable-video-opengl --disable-video-opengles --disable-video-opengles2 \
        --disable-video-vulkan >/dev/null
    make -j4 >/dev/null
)

cp "$source_dir/build/.libs/libSDL2.a" "$target/lib/"
cp "$source_dir/include/"*.h "$target/include/"
echo "sdl2: derlendi ($target/lib/libSDL2.a)"
