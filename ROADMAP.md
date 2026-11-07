# Yol haritası

Her kilometre taşının **çıkış kriteri** var. Kriter sağlanmadan sonrakine
geçilmez; aksi halde KDE'ye kadar hata borcu birikir.

## M0 — Boot ve seri konsol

Yazıldı, doğrulanmadı.

- Limine ile BIOS ve UEFI boot, ELF64, yüksek yarı
- Seri konsol (`uart16550`, COM1, 115200 8N1)
- Limine memmap / HHDM / framebuffer / RSDP / modül istekleri
- Panic handler: vektör, hata kodu, RIP, CR2

**Çıkış:** `make run` ile QEMU'da banner + bellek haritası özeti; kasıtlı
`panic!` çalıştığında anlamlı çıktı.

## M1 — Bellek

- Bitmap fiziksel çerçeve ayırıcı (PMM), HHDM üzerinden erişim
- 4 seviyeli sayfa tablosu: `map`, `unmap`, `translate`, `Prot` bayrakları
- Kernel heap: bağlı liste ayırıcı, `alloc` crate (`Box`, `Vec`, `String`)
- `alloc_error_handler`

**Çıkış:** 1 milyon `Box<u64>` ayır/bırak döngüsü, bellek sızıntısı testi;
`/proc/meminfo` benzeri sayıların tutarlı olması.

## M2 — Kesmeler ve zaman

- GDT + TSS (ring0 yığını, IST)
- IDT, 256 vektör, hata kodlu istisnalar
- 8259 PIC remap, PIT 100 Hz, TSC tabanlı `monotonic_nanos`
- PS/2 klavye IRQ1 → kuyruk, seri girdi

**Çıkış:** `#BP`/`#PF`/`#GP` doğru raporlanıyor, zaman 1 saatte < 1 ms sapıyor.

## M3 — Süreçler ve ilk userland

- `struct Process`: adres alanı (kendi PML4'ü), kernel yığını, fd tablosu
- `switch_to` (callee-saved), round-robin, `yield`, `exit`
- `SYSCALL`/`SYSRET` (MSR_STAR/LSTAR/FMASK), per-CPU yığın
- ELF64 yükleyici: `PT_LOAD`, `PT_INTERP`, `AT_PHDR`/`AT_ENTRY`/`AT_RANDOM` auxv
- VFS: `Inode`/`Dentry` trait'leri, ramfs, `/dev/console`, `/proc/self`
- Syscall: `read write openat close lseek brk mmap munmap exit exit_group
  getpid gettid getuid geteuid uname arch_prctl set_tid_address
  set_robust_list futex clock_gettime getrandom`

**Çıkış:** `busybox sh` statik binary'si ring3'te açılıyor, `ls`, `cat`,
`echo > /tmp/x` çalışıyor.

## M4 — Gerçek dosya sistemi

- VirtIO-blk ve AHCI
- ext4 okuma: superblock, grup tanımlayıcıları, inode, extent ağacı,
  `dir_index` (htree) hash'leri, xattr
- ext4 yazma: bitmap tahsisi, extent ekleme, JBD2
- `mount`, `statfs`, `sync`, page cache (yazma geri akışıyla)

**Çıkış:** Alpine ext4 imajı `e2fsck` temiz bağlanıyor, dosya oluşturulup
silindiğinde `e2fsck -fn` hatasız.

## M5 — Ağ

- virtio-net/e1000, ARP, IPv4, ICMP, UDP, TCP (sınırlı), DHCP, DNS
- `socket`/`bind`/`listen`/`accept`/`connect`/`sendto`/`recvfrom`
- ~~Unix domain socket (`AF_UNIX`) + `SCM_RIGHTS`~~ ✅ (D-Bus doğrulandı)

**Çıkış:** `pacman -Sy` çalışıyor, `apk`/`pacman` imzası doğrulanıyor.

## M6 — Grafik

- ~~Framebuffer metin konsolu~~ ✅
- ~~`/dev/fb0` (fbdev ioctl'leri) + kendi GUI uçbirimimiz~~ ✅
- ~~`evdev`: `/dev/input/event0`, `EVIOCG*`, `input_event`~~ ✅
- ~~SDL2 (`fbdev` yerine `evdev` video sürücüsü) tam ekran çizim + klavye~~ ✅
- DRM/KMS: connector/encoder/CRTC, `virtio-gpu` veya basit framebuffer
- `evdev` (klavye/fare), `udev` için `NETLINK_KOBJECT_UEVENT` + sysfs
- Wayland: `wl_shm`, `wl_seat`, `xdg_shell`, `linux-dmabuf` (pixman ile)
- Mesa yazılım yolu (`llvmpipe`) — GPU sürücüsü olmadan tek gerçekçi yol

**Çıkış:** Weston açılıyor, `weston-terminal` çalışıyor, `WAYLAND_DEBUG=1`
hata vermiyor.

## M7 — KDE Plasma

- ~~D-Bus daemon (unix socket + `SCM_RIGHTS` + `poll`/`epoll` + `memfd`)~~ ✅
  (`dbus-daemon --session` + `dbus-send ListNames` çalışıyor)
- `systemd` yerine **seatd + elogind** (Plasma logind bekler, systemd şart değil)
- Qt6, KF6, KWin Wayland backend, PipeWire, polkit, udisks2
- `cgroup2` (kısmi), `epoll`, `eventfd`, `timerfd`, `signalfd`, `inotify`,
  `memfd_create`, `futex` (sağlam), `clone3` geri düşüşü, `prctl`, `seccomp` stub
- `/proc` ve `/sys`'in Plasma'nın okuduğu kısımları

**Çıkış:** SDDM veya doğrudan `startplasma-wayland` ile Plasma masaüstü
açılıyor; panel, Kickoff, bir Qt uygulaması çalışıyor.

## Gerçekçi süre

M0–M3: aylar. M4–M6: bir yıl mertebesi. M7: yıllar. SerenityOS 6+ yılda
Wayland'a yeni geçti, KDE yok. Bu bir yarış değil; her milestone kendi başına
kullanılabilir bir sistem.
