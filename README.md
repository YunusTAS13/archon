# Archon

A bare-metal x86_64 kernel for running Arch Linux userland without modification.

---

## Overview / Genel Bakış

Archon is a minimalist, Rust-based kernel designed to run Arch Linux userland on bare metal. It implements the essential components needed to run a modern Linux desktop environment while maintaining a minimal footprint. As little AI as possible was used (around 5%).

Archon, Arch Linux userland'ını bare-metal üzerinde çalıştırmak için tasarlanmış minimalist, Rust tabanlı bir kerneledir. Modern bir Linux masaüstü ortamını çalıştırmak için gerekli temel bileşenleri uygularken minimal bir ayak izi korur. En az AI kullanımı (yaklaşık %5 civarı).

---

## Key Features / Temel Özellikler

- ✅ Limine boot protocol support
- ✅ Rust, no_std, and no libc
- ✅ Arch Linux userland compatibility (no recompilation needed)
- ✅ Preemptive round-robin scheduling
- ✅ Basic memory management with bitmap allocator
- ✅ Virtual memory and page tables
- ✅ Process and thread management
- ✅ System call implementation
- ✅ VirtIO-blk storage support
- ✅ ext4 read support
- ✅ SDL2 graphics and input (evdev)
- ✅ Framebuffer GUI capabilities
- ✅ Basic networking stack
- ✅ ELF64 loader with PT_INTERP and full auxv
- ✅ Signal delivery with EINTR semantics
- ✅ Process groups, job control (Ctrl-C, Ctrl-Z, fg, bg)
- ✅ epoll, eventfd, timerfd, memfd, socketpair, O_NONBLOCK
- ✅ Fake terminal device (/dev/ptmx, /dev/pts/0)
- ✅ Real PS/2 keyboard support via evdev
- ✅ Serial console (COM1, 115200 8N1)
- ✅ ACPI and PCI enumeration

- ✅ Limine önyükleme protokolü desteği
- ✅ Rust, no_std, libc yok
- ✅ Arch Linux userland uyumluluğu (yeniden derleme gerekmez)
- ✅ Preemptive round-robin zamanlama
- ✅ Bitmap ayırıcı ile temel bellek yönetimi
- ✅ Sanal bellek ve sayfa tabloları
- ✅ Süreç ve iş parçacığı yönetimi
- ✅ Sistem çağrısı implementasyonu
- ✅ VirtIO-blk depolama desteği
- ✅ ext4 okuma desteği
- ✅ SDL2 grafik ve giriş (evdev)
- ✅ Framebuffer GUI yetenekleri
- ✅ Temel ağ yığını
- ✅ PT_INTERP ve tam auxv ile ELF64 yükleyici
- ✅ EINTR semantiği ile sinyal teslimi
- ✅ Süreç grupları, iş kontrolü (Ctrl-C, Ctrl-Z, fg, bg)
- ✅ epoll, eventfd, timerfd, memfd, socketpair, O_NONBLOCK
- ✅ Sahte terminal cihazı (/dev/ptmx, /dev/pts/0)
- ✅ evdev üzerinden gerçek PS/2 klavye desteği
- ✅ Seri konsol (COM1, 115200 8N1)
- ✅ ACPI ve PCI numaralandırması

---

## Current Status / Mevcut Durum

| Component / Bileşen | Status / Durum |
|---------------------|----------------|
| Boot & Kernel / Önyükleme ve Kernel | ✅ |
| Memory Management / Bellek Yönetimi | ✅ |
| Processes & Threads / Süreçler ve İş Parçacıkları | ✅ |
| System Calls / Sistem Çağrıları | ✅ |
| Storage (ext4 read) / Depolama (ext4 okuma) | ✅ |
| Graphics (SDL2) / Grafik (SDL2) | ✅ |
| Input (evdev) / Giriş (evdev) | ✅ |
| GUI Framebuffer / GUI Framebuffer | ✅ |
| Networking / Ağ | ⬜ |
| DRM/KMS | ⬜ |
| Wayland | ⬜ |
| ext4 write / ext4 yazma | ⬜ |
| Real network stack / Gerçek ağ yığı | ⬜ |

---

## Build Instructions / Derleme Talimatları

### 1. Install dependencies / Bağımlılıkları kurun

```bash
rustup toolchain install nightly --component rust-src llvm-tools
make deps      # Downloads Limine and generates limine_ids.rs / Limine indirir, limine_ids.rs üretir
make           # Builds kernel and ISO / Kernel ve ISO derler
```

### 2. Run in QEMU (nographic) / QEMU'da çalıştır (sadece seri konsol)

```bash
make run
```

### 3. Run with GUI / GUI ile çalıştır

```bash
make run-gui
```

### 4. Build test programs / Test programlarını derle

```bash
make user
```

### 5. Build rootfs test image / Rootfs test imajı oluştur

```bash
make disk
```

---

## Documentation / Dokümantasyon

- [ROADMAP.md](ROADMAP.md) - Milestone-based development plan / Aşama bazlı gelişim planı
- [ARCHITECTURE.md](docs/ARCHITECTURE.md) - Technical architecture details / Teknik mimari detayları
- [SYSCALLS.md](docs/SYSCALLS.md) - System call reference / Sistem çağrısı referansı
- [DEMOS.md](docs/DEMOS.md) - Graphics demos / Grafik demoları

---

## Hardware Support / Donanım Desteği

- QEMU (QEMU System emulator for x86_64) / QEMU (x86_64 için QEMU sistem emulate edici)
- KVM acceleration when available (`/dev/kvm`) / KVM hızlandırması mevcutsa (`/dev/kvm`)
- VirtIO-blk virtual disk / VirtIO-blk sanal disk
- virtio-gpu (future) / virtio-gpu (gelecek)
- PS/2 keyboard and mouse / PS/2 klavye ve fare

---

## Build Configuration / Derleme Yapılandırması

Default build settings / Varsayılan derleme ayarları:
- Target / Hedef: `x86_64-unknown-none`
- Code model / Kod modeli: kernel (high half / yüksek yarım)
- Relocation model / Taşıma modeli: static
- Kernel linker script / Kernel bağlayıcı betiği: `kernel/linker.ld`
- QEMU memory / QEMU belleği: 2 GB
- SMP: 1 processor / 1 işlemci
- CPU: host

The kernel targets x86-64-v3 (AVX2), so host CPU must support AVX2 features.

Çekirdek x86-64-v3 (AVX2) hedefler, bu yüzden ana CPU'nun AVX2 özelliklerini desteklemesi gerekir.

---

## License / Lisans

MIT License / MIT Lisansı
