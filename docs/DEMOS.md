# Grafik demoları

Üç ayrı program var; hepsi aynı çekirdek üzerinde çalışır.

| Program | Ne yapar | Giriş |
|---|---|---|
| `tools/gui/terminal.c` | `/dev/fb0` üzerinde kendi GUI uçbirimimiz, `/dev/ptmx` + `/bin/sh -i` | `/dev/kbd` |
| `tools/gui/sdl-demo.c` | SDL3 (Arch paketi), `SDL_VIDEODRIVER=evdev` | yok (SDL3 evdev sürücüsü yalnızca dokunmatik okur) |
| `tools/gui/sdl2-demo.c` | SDL2 (kaynaktan, statik), tam ekran çizim + klavye | `/dev/input/event0` |

## SDL2 neden kaynaktan

Arch'in `sdl2` paketi artık `sdl2-compat`, yani SDL3'ün üzerinde bir kabuk.
SDL2'nin **`evdev` video sürücüsü** ("dummy video driver with evdev") sahte bir
video arka ucu ile gerçek `/dev/input/event*` aygıtlarını birleştirir: ekran
çizimi bize kalır, klavye/fare SDL olay kuyruğuna düşer.

```sh
tools/dev/sdl2.sh          # SDL2 2.32.10'u build/sdl2 altına derler
tools/dev/rootfs.sh gui    # sdl2-demo + font + sdl2run betiğini köke koyar
make iso
```

`SDL_EVDEV_DEVICES` yalnızca libudev **olmadan** derlenmiş SDL2'de kullanılır;
`tools/dev/sdl2.sh` bu yüzden `--disable-libudev` ile yapılandırır. Sınıf
numarası `2` klavyedir:

```sh
export SDL_VIDEODRIVER=evdev
export SDL_EVDEV_DEVICES=2:/dev/input/event0
exec /bin/sdl2-demo
```

## Framebuffer sahipliği

Bir süreç `/dev/fb0`'ı eşlediğinde çekirdek onu ekranın sahibi ilan eder
(`mmap_notify`). GUI uçbirimi `FB_OWNER_IOCTL` (`0xF100`) ile sahibi sorar ve
başka bir süreç ekrandayken **çizim yapmayı bırakır**; süreç bitince ekranı
tamamen yeniden çizer. Böylece tam ekran bir SDL uygulaması arkasındaki
uçbirimle çakışmaz.

## Ekran görüntüleri

`docs/screenshots/` altındakiler QEMU monitor soketinden alınır:

```sh
echo "screendump /tmp/opencode/screen.ppm" | \
    socat - UNIX-CONNECT:/tmp/opencode/mon.sock
```
