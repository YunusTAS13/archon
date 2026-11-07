# KDE Plasma planı

Hedef: Arch userland'ı üzerinde bir Plasma oturumu. Bu belge, hedefin neden
"KDE yazmak" değil "Linux'un yarısını yazmak" olduğunu ve sıranın ne olması
gerektiğini anlatır.

## Kısa gerçek

Plasma'nın kendisi işin küçük kısmı. Açılış zinciri şöyle:

```
sddm ──► startplasma-wayland ──► kwin_wayland ──► plasmashell
  │            │                      │                 │
  │            └─ dbus-daemon         ├─ libdrm (ioctl)
  │                                   ├─ mesa (llvmpipe)
  │                                   ├─ libinput (evdev)
  │                                   └─ gbm / dma-buf
  └─ PAM, logind (seat), fork/exec
```

Bu zincirdeki her kutu, çekirdekten farklı bir şey ister. Kabaca:

| Katman | Çekirdekten istediği |
|---|---|
| `dbus-daemon` | AF_UNIX socket, `SCM_RIGHTS`, `poll`, `memfd` |
| `kwin_wayland` | DRM/KMS ioctl, `epoll`, `eventfd`, `timerfd`, `signalfd`, `memfd`, `dma-buf`, `futex` |
| `mesa` (llvmpipe) | `mmap`, `mprotect`, `madvise`, `memfd`, çalışan TLS, `/proc/self/maps` |
| `libinput` | `evdev`, `udev`, netlink uevent, `/sys/class/input` |
| `plasmashell` | Qt6 + KF6, D-Bus, `inotify`, thread'ler, `cgroup` (kısmi) |
| `sddm` | PAM, `setuid`, `fork`/`exec`, `/etc/shadow`, VT |

Yani `docs/SYSCALLS.md`'deki "Grafik" tablosunun tamamı + süreç modeli +
sinyaller + `/proc` + `/sys` + `/dev` + `udev` + `D-Bus` + `logind`.

## Sıra (bu sıra atlanamaz)

### A. Süreç modeli

- `clone`/`fork`/`execve`/`wait4`, `exit` + zombi toplama
- Sinyaller: `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, `kill`
- Thread'ler: `CLONE_VM` + ayrı yığın + TLS (`arch_prctl(ARCH_SET_FS)`)
- Preemptive round-robin, SMP (en az 2 çekirdek; Qt thread pool ister)

**Kabul:** `busybox sh` içinden `sh -c 'a | b &'` çalışıyor, `wait` dönüyor.

### B. Bootstrap: Arch kök dosya sistemi

Arch'ı kendi içinde kurmaya çalışmak (pacman ile) tavşan deliğidir. Doğru yol:
kök dosya sistemini **host'ta** hazırlayıp imaja yazmak.

```sh
# host'ta
mkdir archroot && pacstrap -K -c archroot base base-devel
# ya da: podman export archlinux:latest
```

Sonra:

- ext4 imajını mount edip kopyalamak kernel tarafında `ext4` yazma ister
  (M4). O gelene kadar imajı `mkfs.ext4` + `e2cp`/loop mount ile host'ta doldur.
- Kernel yalnızca okuma yapar; `mount -o ro` ile açılır.

**Kabul:** `/usr/bin/bash` çalışıyor, `pacman -Q` listeyi basıyor.

### C. Ağ (pacman için)

`virtio-net` + DHCP + DNS + TCP. `pacman` HTTP/TLS ister; TLS için `gnutls`
çalışmalı, bu da `/dev/urandom`, `getrandom`, `clock_gettime`, thread'ler
demek.

**Kabul:** `pacman -Sy` aynaları indiriyor.

### D. G/Ç ve cihaz modeli

- `devtmpfs` benzeri `/dev` (statik düğümlerle başlanabilir: `null`, `zero`,
  `console`, `tty`, `random`, `urandom`, `dri/card0`, `input/event*`)
- `sysfs`: `kwin` ve `libinput` `udev` üzerinden cihaz arar; `udev` netlink
  uevent dinler. En kısa yol: `udev`'i atlayıp cihazları statik `evdev`
  düğümleriyle vermek, `libinput`'a `LIBINPUT_DEVICE` ile işaret etmek.
- `procfs`: `/proc/self/{maps,status,stat,cmdline}`, `/proc/cpuinfo`,
  `/proc/meminfo` — Mesa ve Qt bunları okuyor.

**Kabul:** `weston` açılıyor (Plasma'dan çok önce).

### E. Grafik

1. Framebuffer + `evdev` + `wl_shm` ile **Weston** (pixman backend).
2. DRM/KMS: `virtio-gpu` (QEMU) veya gerçek GPU için basit `bochs`/`cirrus`.
   `atomic modesetting` ioctl'leri: `DRM_IOCTL_MODE_CREATE_DUMB`,
   `MAP_DUMB`, `ADDFB2`, `ATOMIC`, `PAGE_FLIP`.
3. Mesa llvmpipe: `memfd` + `dma-buf` (aslında `wl_drm`/`linux-dmabuf`
   yerine `wl_shm` ile de çalışır, yavaş ama doğru).
4. `epoll` + `eventfd` + `timerfd` + `signalfd`: Wayland ve Qt'nin olmazsa
   olmazı.

**Kabul:** `weston-terminal` çalışıyor, fare/klavye çalışıyor.

### F. D-Bus ve oturum

- `AF_UNIX` + `SCM_RIGHTS` (fd geçişi) + `poll`
- `dbus-daemon --session` + `dbus-broker`den biri
- `elogind` (systemd'siz logind): seat yönetimi, `/run/systemd/seats`,
  cgroup2 (kısmi), `inotify`
- `polkit`, `udisks2` (Plasma'nın disk paneli için)

**Kabul:** `busctl list` çalışıyor, `elogind` seat açıyor.

### G. Plasma

- Qt6 + KF6 paketleri (Arch'ta hazır)
- `kwin_wayland --drm` veya `--virtual`
- `startplasma-wayland`
- Sonunda `sddm` (PAM, VT, `setuid`)

**Kabul:** masaüstü açılıyor, panel ve Kickoff çalışıyor.

## Kaçış yolları (dürüst olalım)

- **Plasma X11**: Xorg sunucusunu yazmak Wayland'den daha büyük. Yapmayın.
- **KWin'i nested çalıştırmak**: KWin bir Wayland *istemcisi* olarak
  çalışamaz; host'ta Weston açıp içine KWin koymak Plasma'yı Archon'da
  çalıştırmak sayılmaz.
- **Sadece "Plasma görüntüsü"**: Ekran görüntüsü üretmek kolay, hedef değil.
- **`systemd`'yi taklit etmek**: gerek yok; `elogind` + `seatd` yeterli.

## Gerçekçi takvim

| Aşama | Süre (tek geliştirici, yarı zamanlı) |
|---|---|
| A süreç modeli | 2–4 ay |
| B bootstrap | 1 ay (+ M4 ext4 yazma 3–6 ay) |
| C ağ | 2–3 ay |
| D cihaz modeli | 2–4 ay |
| E Weston | 4–8 ay |
| F D-Bus/elogind | 3–6 ay |
| G Plasma | 3–6 ay |

Toplam: **1,5–3 yıl**. Bu iyimser; SerenityOS 6 yılda Wayland'a yeni geçti ve
KDE çalıştırmıyor. İyi haber: her aşama kendi başına gösterilebilir bir sistem.

## İlk yapılacak

`ROADMAP.md` M3. Bugünkü kodda eksik olan sıralı liste:

1. `clone`/`fork`/`wait4` + gerçek zamanlayıcı (bağlam değiştirme asm'i hazır)
2. `execve` + `PT_INTERP` (dinamik `ld.so` — Arch binary'lerinin çoğu dinamik)
3. Sinyaller
4. `PT_INTERP`'li ilk Arch binary'si: `/usr/bin/true`
