# Syscall matrisi

Numaralar Linux x86_64 ABI'si. Durum: ✅ çalışıyor · 🟡 kısmi · ⬜ yok.

## Süreç ve bellek

| # | Ad | Durum | Not |
|---|---|---|---|
| 9 | mmap | ✅ | anonim, dosya, `MAP_FIXED`, aygıt belleği (`/dev/fb0`) |
| 10 | mprotect | ✅ | |
| 11 | munmap | ✅ | |
| 12 | brk | ✅ | |
| 56/57 | clone | ✅ | `CLONE_VM` yoksa `fork`; varsa iş parçacığı (`VM`, `FILES`, `SIGHAND`, `THREAD`, `SETTLS`, `PARENT_SETTID`, `CHILD_CLEARTID`) |
| 57 | fork | ✅ | derin adres alanı kopyası, çocukta `RAX=0` |
| 59 | execve | ✅ | dinamik ELF + `PT_INTERP`, argv/envp/auxv |
| 60 | exit | ✅ | |
| 61 | wait4 | ✅ | zombi toplama, `WNOHANG`, `WUNTRACED` (duran çocuk), engelleme |
| 62 | kill | ✅ | pid, 0 (grup), negatif (grup); `SIGKILL`/`SIGSTOP` yakalanamaz |
| 13 | rt_sigaction | ✅ | çekirdek ABI düzeni, `SA_SIGINFO`, `SA_NODEFER`, `sa_mask` |
| 14 | rt_sigprocmask | ✅ | `SIG_BLOCK`/`UNBLOCK`/`SETMASK` |
| 15 | rt_sigreturn | ✅ | kullanıcı yığınındaki çerçeveyi geri yükler |
| 131 | sigaltstack | 🟡 | 0 döner (alternatif yığın yok) |
| 200/234 | tkill/tgkill | ✅ | iş parçacığı hedefli sinyal |
| 24 | sched_yield | ✅ | zamanlayıcıya bırakır |
| 186 | gettid | ✅ | |
| 158 | arch_prctl | ✅ | `ARCH_SET_FS`/`GET_FS`, süreç başına TLS |
| 202 | futex | 🟡 | `WAIT`/`WAKE`/`WAIT_BITSET`; bekleme, sözcüğü yoklayan devirli döngü |
| 218 | set_tid_address | ✅ | |
| 228 | clock_gettime | ✅ | |
| 231 | exit_group | ✅ | |
| 273 | set_robust_list | ✅ | stub |
| 302 | prlimit64 | ✅ | sabit limitler |
| 318 | getrandom | 🟡 | TSC karışımı, CSPRNG değil |
| 39/110 | getpid/getppid | ✅ | |
| 109/111/112/121/124 | setpgid/getpgrp/setsid/getpgid/getsid | ✅ | gerçek süreç grupları; iş kontrolü (`^Z`, `fg`, `jobs`) |
| 102/104/107/108 | getuid/gid/euid/egid | ✅ | 0 |

## Dosya sistemi

| # | Ad | Durum | Not |
|---|---|---|---|
| 0 | read | ✅ | dosya, boru, konsol, pty, klavye |
| 1 | write | ✅ | dosya, boru, konsol, pty |
| 2 | open | ✅ | ramfs, ext4 (okuma), devfs |
| 3 | close | ✅ | |
| 5 | fstat | ✅ | Linux `struct stat` yerleşimi |
| 8 | lseek | ✅ | |
| 16 | ioctl | ✅ | tty (termios, TIOCGWINSZ, pgrp), fbdev (`FBIOGET_*`, sahiplik), evdev (`EVIOCG*`) |
| 17 | pread64 | ✅ | |
| 19/20 | readv/writev | ✅ | 1 MiB sınırı |
| 21/269/439 | access/faccessat/faccessat2 | ✅ | |
| 22/293 | pipe/pipe2 | ✅ | 64 KiB halka tampon, `O_NONBLOCK` |
| 32/33 | dup/dup2 | ✅ | |
| 72 | fcntl | ✅ | `F_GETFD`/`F_SETFD`/`F_GETFL`/`F_SETFL` (`O_NONBLOCK`) |
| 79/80 | getcwd/chdir | ✅ | |
| 89 | readlink | ✅ | |
| 217 | getdents64 | ✅ | `linux_dirent64`, NUL sonlandırıcı dahil |
| 257 | openat | ✅ | |
| 262 | newfstatat | ✅ | |
| 332 | statx | ✅ | |
| 7 | poll | ✅ | hazırlık: boru, konsol, pty, klavye, dosya |
| 23 | select | ✅ | |
| 270 | pselect6 | ✅ | |
| 271 | ppoll | ✅ | |

## Grafik ve sonraki adımlar

| # | Ad | Durum | Neden |
|---|---|---|---|
| 232/233/281/291 | epoll_create1/ctl/wait | ✅ | seviye tetiklemeli, `ready()` üzerinden |
| 290/284 | eventfd2/eventfd | ✅ | `EFD_SEMAPHORE`, `EFD_NONBLOCK` |
| 283/286/287 | timerfd_create/settime/gettime | ✅ | tek atım ve aralık |
| 319 | memfd_create | ✅ | `MAP_SHARED` gerçek sayfa paylaşımı |
| 53 | socketpair | ✅ | `AF_UNIX`, `SOCK_STREAM`/`SOCK_DGRAM` |
| 41/42/49/50 | socket/connect/bind/listen | ✅ | yol adlı + soyut (`@name`) ad alanı |
| 43/288 | accept/accept4 | ✅ | `SOCK_NONBLOCK`, `SOCK_CLOEXEC` |
| 46/47 | sendmsg/recvmsg | ✅ | `SCM_RIGHTS` ile fd geçişi |
| 44/45 | sendto/recvfrom | ✅ | bağlı soketlerde |
| 48 | shutdown | ✅ | `SHUT_RD`/`SHUT_WR`/`SHUT_RDWR` |
| 51/52 | getsockname/getpeername | ✅ | `sockaddr_un` |
| 54/55 | setsockopt/getsockopt | ✅ | `SO_TYPE`, `SO_ERROR`, `SO_PEERCRED`, `SO_ACCEPTCONN` |
| 83/84/87/263 | mkdir/rmdir/unlink/unlinkat | ✅ | ramfs + ext4 okuma kökü |
| 90/91/268 | chmod/fchmod/fchmodat | ✅ | izinler saklanır, zorlanmaz |
| 280 | utimensat | ✅ | yok sayılır |
| 77 | ftruncate | ✅ | |
| 35/230 | nanosleep/clock_nanosleep | ✅ | `TIMER_ABSTIME` |
| 98 | getrusage | ✅ | sıfır sayaçlar |
| 96/99/204 | gettimeofday/sysinfo/sched_getaffinity | ✅ | SDL açılışı için |
| 28 | madvise | ✅ | yok sayılır |
| 289 | signalfd4 | ⬜ | sinyal tabanlı olay döngüleri |
| 253/254 | inotify | ⬜ | dosya izleme |
| 137/165/166 | statfs/mount/umount2 | ⬜ | gerçek kök, paket yöneticisi |
| 155 | pivot_root | ⬜ | initramfs'ten gerçek köke geçiş |
| 157 | prctl | 🟡 | `0` döner (ad/seccomp yok) |

## Bilinçli olarak yapılmayacaklar

- `io_uring`: hiçbir Arch masaüstü bileşeni zorunlu tutmuyor.
- KVM, eBPF, nftables: kapsam dışı.
- `clone3`: glibc `ENOSYS` görürse `clone`'a düşer; önce o yol.
