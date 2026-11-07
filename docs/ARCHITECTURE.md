# Mimari

## Adres uzayı

| Aralık | Kullanım |
|---|---|
| `0x0000_0000_0000_1000` – `0x0000_4FFF_FFFF_FFFF` | kullanıcı kodu, heap, `mmap` bölgesi (`MMAP_BASE = 0x0000_6000_0000_0000`) |
| `0x0000_7FFF_FFFF_0000` | kullanıcı yığını (16 sayfa, aşağı büyür) |
| `0xFFFF_8000_0000_0000` – `0xFFFF_CFFF_FFFF_FFFF` | HHDM: tüm fiziksel bellek |
| `0xFFFF_FFFF_8000_0000` | çekirdek imajı (`.text`, `.rodata`, `.data`, `.bss`) |

Çekirdek yüksek yarıda, `-C code-model=kernel`. Kullanıcı süreçleri kendi
PML4'lerini alır; PML4 girdileri 256–511 arası (üst yarı) çekirdekle
paylaşılır, 0–255 arası boştur.

## Önyükleme sırası

1. Limine `kmain`'e atlar, tüm istekler `.limine_requests` bölümünde hazır.
2. Seri port açılır. Bundan sonraki her çıktı seriye gider.
3. `boot::init` memmap'i BSS'teki sabit tampona kopyalar (heap yok).
4. `mem::init`: HHDM kaydedilir, PMM bitmap'i kurulur, heap BSS arenası hazır.
5. GDT + TSS: `SYSCALL` için `STAR`, `LSTAR`, `SFMASK`.
6. IDT: 256 vektör, `ISR_ERR`/`ISR_NOERR` stub'ları, tek ortak giriş.
7. PIC remap, PIT 100 Hz, TSC kalibrasyonu.
8. ACPI (MADT), PCI taraması, framebuffer konsolu.
9. VFS (ramfs kök) + initramfs (newc cpio), syscall tablosu, zamanlayıcı.
10. VirtIO-blk sürücüsü, ext4 okuma, `/mnt` bağlaması.
11. `/bin/init` ring3'e iner; `fork`, `execve`, `wait4` zinciri çalışır.

## Durum özeti

| Katman | Durum |
|---|---|
| Önyükleme, bellek, kesmeler | ✅ |
| Preemptive round-robin (50 ms) | ✅ adres alanı değişimi dahil |
| `fork` + `wait4` + `execve` (statik ELF) | ✅ |
| Initramfs (cpio newc) ve VFS bağlama noktaları | ✅ |
| VirtIO-blk (modern PCI) + ext4 okuma | ✅ `/mnt` |
| Dinamik ELF (`PT_INTERP`), tam auxv | ✅ |
| Sinyaller (`rt_sigaction`, `SIGINT/SIGTSTP`, `EINTR`) | ✅ |
| İş parçacıkları (`clone` + `futex` + TLS) | ✅ |
| Süreç grupları, iş kontrolü (`^C`, `^Z`, `fg`, `jobs`) | ✅ |
| `/dev`, `/proc`, framebuffer GUI uçbirim | ✅ |
| epoll, eventfd, timerfd, memfd (`MAP_SHARED`), socketpair | ✅ |
| `evdev` (`/dev/input/event0`) + framebuffer sahipliği | ✅ |
| SDL2 (kaynaktan) tam ekran çizim + klavye | ✅ |
| SDL3 (gerçek Arch paketi) framebuffer'a çiziyor | 🟡 klavye X11/Wayland istiyor |
| ext4 yazma, ağ, DRM/KMS, Wayland | ⬜ sırada |

## Zamanlayıcı ve süreçler

Her sürecin 16 KiB çekirdek yığını vardır; `TSS.RSP0` ve syscall girişi bu
yığını kullanır. İki geçiş yolu var:

- **Kullanıcıdan kesme (preemption):** PIT kesmesi ring3'ten geldiyse kesme
  çerçevesi sıradaki sürecin `UserContext`'iyle yeniden yazılır, CR3 ve
  çekirdek yığını güncellenir, `iretq` doğrudan yeni sürece döner.
- **Çekirdekten engelleme (`wait4`):** `archon_switch_context` ile çekirdek
  bağlamı (callee-saved + RSP + RIP) saklanır. Yeni süreç ilk kez
  çalışıyorsa `user_trampoline` üzerinden `iretq` ile ring3'e iner.

`fork` kullanıcı yarısını derin kopyalar (her 4 KiB sayfa yeni çerçeve).
Zombi çocuklar `wait4` ile toplanır; adres alanı ve çekirdek yığını serbest
bırakma işi henüz yok.

## İş parçacıkları ve sinyaller

`clone(2)` `CLONE_VM` ile çağrılırsa adres alanı kopyalanmaz: yeni iş
parçacığı aynı `pml4`'ü, dosya tablosunu (`CLONE_FILES`), sinyal durumunu
(`CLONE_SIGHAND`) ve `brk` işaretçisini paylaşır. Yalnızca çekirdek yığını,
TLS (`CLONE_SETTLS` → `fs_base`) ve kullanıcı yığını ayrıdır. `CLONE_VM`
yoksa çağrı sıradan `fork`'a düşer.

Sinyal durumu (`SignalState`) bekleyen küme, maskelenen küme ve 64 girişlik
eylem tablosundan oluşur. Teslimat syscall dönüşünde yapılır: kullanıcı
yığınının altına 312 baytlık çerçeve (geri dönüş adresi + kaydedilmiş
register'lar + `siginfo`) kurulur, `RIP` işleyiciye çevrilir. İşleyici
döndüğünde glibc'nin `__restore_rt`'si `rt_sigreturn` çağırır ve bağlam
geri yüklenir. Engellenen bir sinyal varsa bloklayan syscall'lar (`read`,
`poll`, `select`, `wait4`, `futex`) `EINTR` döner.

İş kontrolü: `setpgid`/`setsid` gerçek süreç gruplarını yönetir, pty
`TIOCSPGRP` ile ön plan grubunu saklar, `^C`/`^Z` o gruba `SIGINT`/`SIGTSTP`
gönderir. Duran çocuk `wait4(WUNTRACED)` ile `0x7f | (SIGTSTP << 8)` olarak
bildirilir; `bash` bunu `[1]+ Stopped` diye raporlar ve `fg`/`bg` çalışır.

## Depolama

`drivers/virtio_blk.rs` modern virtio 1.0 PCI aygıtını sürer: capability
yürüyüşü, common/notify/device bölgeleri, tek virtqueue (64 giriş), üç
tanımlayıcılı istek (başlık + veri + durum). MMIO bölgeleri
`vmm::mmio_map` ile üst yarıda `0xFFFF_C000_0000_0000` sonrasına eşlenir;
süreçler başlamadan önce eşlendiği için tüm adres alanlarında görünür.

`BlockDevice` 512 baytlık sektörlerle çalışır. ext4 katmanı kendi blok
numaralarını sektöre çevirir (`blok * blok_boyu / 512`). ext4 şu an yalnızca
okunur; yazma + JBD2 M4'te.

## Bellek

**PMM** 1 MiB bitmap ile 32 GiB'a kadar 4 KiB çerçeve yönetir. Bitmap
`0xFF` (dolu) başlar ve yalnızca `MEMMAP_USABLE` aralıklar boşaltılır; böylece
çekirdek, modüller, framebuffer ve ACPI tabloları kendiliğinden korunur.

**VMM** mevcut CR3'ü yürür, 4 KiB eşleme ekler/çıkarır ve yeni adres alanları
için üst yarıyı kopyalar. Kernel eşlemeleri HHDM üzerinden yapıldığı için
`map` çağrısı yalnızca kullanıcı sayfalarında gerekli.

**Heap** BSS'te 64 MiB arena üzerinde first-fit bağlı liste. Blok başlığı
16 bayt olduğu için `align <= 16` istekleri bedava hizalanır. Arena PMM
çerçevelerine taşındığında `heap::grow` eklenecek.

## Kesmeler

`idt.rs` içindeki `global_asm!` 256 stub üretir:

- Hata kodu itmeyen vektörler sahte `0` iter, sonra vektör numarası.
- Hata kodu iten vektörler (8, 10–14, 17, 21, 29, 30) yalnızca vektör ekler.
- Ortak giriş 15 GPR'yi yığar, yığını 16'ya hizalar, `isr_dispatch` çağırır.

`InterruptFrame` yerleşimi bu sırayla birebir eşleşir. `isr_dispatch`
istisnaları raporlar, IRQ'ları PIC'e yönlendirir, `0x80` yazılım kesmesini
syscall olarak kabul eder.

## Syscall ABI

`SYSCALL`/`SYSRET` (MSR `STAR`, `LSTAR`, `SFMASK`). Girişte kullanıcı RSP'si
ve callee-saved register'lar global scratch alanına yazılır, sürecin kendi
çekirdek yığınına geçilir. `fork` için RSP/RIP/RFLAGS/RBX/RBP/R12–R15
saklanır; çocuk ilk çalıştığında `RAX = 0` ile döner. Argümanlar `SyscallArgs` yapısına paketlenip tek işaretçi olarak
`archon_syscall_dispatch`'e verilir. Bu, 6 argümanlı Linux çağrılarını
register sırası derdi olmadan taşır.

Tek CPU varsayımı: scratch alanı global. SMP geldiğinde `swapgs` + per-CPU
alanına geçilecek; `ROADMAP.md` M2'nin sonunda.

## Dosya sistemi

`Inode` trait'i içerik + meta veri, `Dentry` yerine `lookup`, `FileSystem`
bağlama noktası. `ramfs` tam yazılabilir (initramfs ve `/tmp`). `ext4` okuma
yolu yazıldı: süper blok, grup tanımlayıcıları, inode, extent ağacı (iç
düğümler dahil), seyrek dosya delikleri, `readdir` kayıtları.

## Neden kendi ABI'miz değil

Arch binary'leri yeniden derlenmeden çalışmalı. Bu, `docs/KDE-PLAN.md`'deki
tüm işin ön koşulu.

## Aygıtlar (`/dev`)

`devfs` sanal bir dosya sistemi olarak `/dev`'e bağlanır; gerçek düğüm
dosyaları yerine `Inode` nesneleri sunar:

| Düğüm | Davranış |
|---|---|
| `/dev/null` | okuma EOF, yazma yutulur |
| `/dev/zero` | sonsuz sıfır |
| `/dev/full` | yazma `ENOSPC` |
| `/dev/random`, `/dev/urandom` | xorshift64 (CSPRNG değil) |
| `/dev/console`, `/dev/tty` | seri port + PS/2; çıktı framebuffer'a da gider |
| `/dev/kbd` | PS/2'den ham ASCII, engelleyici |
| `/dev/fb0` | framebuffer; `mmap` fiziksel belleği doğrudan eşler |
| `/dev/ptmx`, `/dev/pts/0` | sözde uçbirim çifti |

`Inode` trait'i `ready()` (select/poll), `is_terminal()`, `physical_region()`
ve `ioctl()` kancalarını taşır; böylece aygıtlar VFS'in genel yollarını
değiştirmeden çalışır.

## Sözde uçbirim

`/dev/ptmx` ana taraf, `/dev/pts/0` köle taraftır. Köleden yazılanlar ana
tarafın okuma kuyruğuna, anadan yazılanlar kölenin girdi kuyruğuna gider.
Satır disiplini `ICANON`, `ECHO`, `ICRNL` ve `VERASE` davranışını uygular;
`termios` çağrı başına saklanır. Böylece bash gerçek bir uçbirim görür:
`isatty(0)` doğrudur, readline ham kipe geçip satır düzenlemeyi kendisi yapar.

## Anonim dosya tanımlayıcıları

`epoll`, `eventfd`, `timerfd`, `memfd` ve `socketpair` ayrı bir
"özel fd" mekanizması kullanmaz: hepsi `Inode` trait'ini uygular
(`kernel/src/fs/anon.rs`, `kernel/src/fs/epoll.rs`). Böylece `read`,
`write`, `dup`, `poll`, `close` ve `mmap` yolları değişmeden çalışır.

- Hazırlık tek kaynaktan gelir: `Inode::ready(write)`. `epoll` kayıtlarında
  tampon kopyası tutulmaz, her `epoll_wait` çağrısı hedef düğüme yeniden sorar.
- Engelleme syscall katmanında `schedule()` döngüsüyle yapılır; sinyal
  geldiğinde döngü `EINTR` ile çıkar.
- `O_NONBLOCK` `OpenFile` üzerinde tutulur; hazır olmayan fd `EAGAIN` döner.
- `memfd` sayfaları inode'a aittir. `MAP_SHARED` eşlemesi bu fiziksel
  çerçeveleri doğrudan kullanır, `munmap` onları serbest bırakmaz; iki süreç
  aynı belleği görür (`wl_shm` için gerekli).

## Grafik uçbirim

`tools/gui/terminal.c` kullanıcı alanında çalışır: `/dev/fb0`'ı `mmap` eder,
`FBIOGET_VSCREENINFO`/`FBIOGET_FSCREENINFO` ile geometriyi öğrenir, `/dev/ptmx`
üzerinden `/bin/sh -i` çalıştırır ve `/dev/kbd`'den tuşları köleye aktarır.
Çekirdek metin konsolu ilk framebuffer eşlemesinde donar (`fbtext::freeze`),
ekranın sahibi artık uygulamadır.

## Giriş aygıtları (`evdev`)

PS/2 klavye sürücüsü, ASCII karakter üretmenin yanında `/dev/input/event0`
için Linux uyumlu olaylar da üretir (`kernel/src/drivers/evdev.rs`):

- `struct input_event` 24 bayt: `timeval`, `type`, `code`, `value`.
- Set 1 tarama kodları Linux anahtar kodlarıyla aynıdır (`KEY_A = 30` gibi);
  `0xE0` önekli oklar, Home/End, Delete ayrıca eşlenir.
- Her tuş olayından sonra `EV_SYN`/`SYN_REPORT` gelir.
- Kuyruk tek üretici (IRQ) / tek tüketici (syscall) halka tampondur. IRQ
  bağlamı hiçbir spinlock almaz; tüketicileri ayrı bir kilit sıraya sokar.
- `EVIOCGNAME`, `EVIOCGVERSION`, `EVIOCGID`, `EVIOCGBIT`, `EVIOCGKEY`,
  `EVIOCGPHYS`, `EVIOCGPROP` desteklenir.

SDL2'nin `evdev` video sürücüsü tam olarak bunu bekler: sahte video arka ucu +
`/dev/input/event*`. Bir uygulama ekranı eşlediğinde çekirdek framebuffer
metnini dondurur ve `/dev/fb0` üzerinde `FB_OWNER_IOCTL` (0xF100) ile son
eşleyen süreç bildirilir; GUI uçbirimi bu sırada çizim yapmayı bırakır ve
süreç bitince ekranı tamamen yeniden çizer.

## Zamanlayıcı

Hazır süreç seçimi tablonun başından değil, çalışan süreçten sonraki
girdiden başlar. Aksi hâlde `wait4` döngüsündeki eski süreçler yeni
fork edilen çocukları sürekli atlar (açlık).
