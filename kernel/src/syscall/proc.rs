//! Süreç syscall'ları: kimlik, `exit`, `uname`, TLS, saat.

use core::sync::atomic::Ordering;

use super::{copy_to_user, EFAULT, EINVAL, ENOSYS};
use crate::arch::cpu;

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;
const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
const CLOCK_THREAD_CPUTIME_ID: i32 = 3;

const ARCH_SET_GS: i32 = 0x1001;
const ARCH_SET_FS: i32 = 0x1002;
const ARCH_GET_FS: i32 = 0x1003;
const ARCH_GET_GS: i32 = 0x1004;

static FS_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static GS_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Sinyal gönderimi: gerçek teslimat henüz yok, çağrı başarılı sayılır.
pub fn kill(_pid: i64, _signal: u32) -> i64 {
    0
}

pub fn getpid() -> i64 {
    crate::task::current_pid() as i64
}

pub fn getppid() -> i64 {
    crate::task::current()
        .map(|process| process.ppid as i64)
        .unwrap_or(0)
}

pub fn exit(code: i32) -> i64 {
    crate::task::exit(code);
}

const UTS_LENGTH: usize = 65;

pub fn uname(pointer: u64) -> i64 {
    let mut buffer = [0u8; UTS_LENGTH * 6];
    let fields = [
        "Linux",              // sysname: glibc ve Qt bunu bekler
        "archon",             // nodename
        "6.6.0-archon",       // release
        "#1 SMP Archon",      // version
        "x86_64",             // machine
        "(none)",             // domainname
    ];

    for (index, field) in fields.iter().enumerate() {
        let bytes = field.as_bytes();
        let start = index * UTS_LENGTH;
        let count = bytes.len().min(UTS_LENGTH - 1);
        buffer[start..start + count].copy_from_slice(&bytes[..count]);
    }

    match copy_to_user(pointer, &buffer) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

pub fn arch_prctl(code: i32, address: u64) -> i64 {
    match code {
        ARCH_SET_FS => {
            FS_BASE.store(address, Ordering::Relaxed);
            if let Some(process) = crate::task::current() {
                process.fs_base.store(address, Ordering::Relaxed);
            }
            // FS tabanı MSR'dan yüklenir; TLS bu olmadan çalışmaz.
            unsafe { cpu::write_msr(0xC000_0100, address) };
            0
        }
        ARCH_SET_GS => {
            GS_BASE.store(address, Ordering::Relaxed);
            unsafe { cpu::write_msr(cpu::MSR_KERNEL_GS_BASE, address) };
            0
        }
        ARCH_GET_FS => match copy_to_user(address, &FS_BASE.load(Ordering::Relaxed).to_le_bytes()) {
            Ok(()) => 0,
            Err(failure) => failure,
        },
        ARCH_GET_GS => match copy_to_user(address, &GS_BASE.load(Ordering::Relaxed).to_le_bytes()) {
            Ok(()) => 0,
            Err(failure) => failure,
        },
        _ => EINVAL,
    }
}

pub fn set_tid_address(pointer: u64) -> i64 {
    // glibc bunu her iş parçacığı başlangıcında çağırır; çıkışta sıfırlanır.
    if let Some(process) = crate::task::current() {
        process
            .clear_child_tid
            .store(pointer, core::sync::atomic::Ordering::Relaxed);
    }
    crate::task::current_pid() as i64
}

pub fn gettid() -> i64 {
    crate::task::current_pid() as i64
}

pub fn sched_yield() -> i64 {
    crate::task::schedule();
    0
}

/// `futex(uaddr, op, val, timeout, uaddr2, val3)`.
///
/// Bekleme, sözcüğü yoklayan ve her turda zamanlayıcıya bırakan bir döngüdür:
/// uyandırma yarışı olmadığı için `FUTEX_WAKE` ayrıca bir şey yapmak zorunda
/// değil, uyanan taraf sözcüğün değiştiğini kendisi görür.
pub fn futex(uaddr: u64, op: u32, value: u32, _uaddr2: u64, _val3: u32) -> i64 {
    use crate::syscall::{EAGAIN, EFAULT, EINTR};
    const FUTEX_WAIT: u32 = 0;
    const FUTEX_WAKE: u32 = 1;
    const FUTEX_REQUEUE: u32 = 3;
    const FUTEX_WAIT_BITSET: u32 = 9;
    const FUTEX_WAKE_BITSET: u32 = 10;

    match op & 0x7f {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            let mut first = true;
            loop {
                match crate::syscall::read_user_u32(uaddr) {
                    None => return EFAULT,
                    Some(current) => {
                        if current != value {
                            return if first { EAGAIN } else { 0 };
                        }
                    }
                }
                first = false;
                if crate::task::signal::interrupted() {
                    return EINTR;
                }
                crate::task::schedule();
            }
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => value as i64,
        FUTEX_REQUEUE => 0,
        _ => 0,
    }
}

pub fn clock_gettime(clock: i32, pointer: u64) -> i64 {
    let nanos = match clock {
        CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            crate::time::monotonic_nanos()
        }
        _ => return EINVAL,
    };

    let mut buffer = [0u8; 16];
    buffer[0..8].copy_from_slice(&((nanos / 1_000_000_000) as u64).to_le_bytes());
    buffer[8..16].copy_from_slice(&((nanos % 1_000_000_000) as u64).to_le_bytes());

    match copy_to_user(pointer, &buffer) {
        Ok(()) => 0,
        Err(failure) => failure,
    }
}

pub fn getrandom(pointer: u64, length: usize) -> i64 {
    if length > 4096 {
        return EINVAL;
    }

    // TSC + adres karışımı yeterli bir erken entropi kaynağı; gerçek CSPRNG
    // M3'te ChaCha20 ile gelecek (getrandom bloklamamalı).
    let mut buffer = alloc::vec![0u8; length];
    let mut state = cpu::rdtsc() ^ (pointer.rotate_left(17)) ^ crate::time::monotonic_nanos();
    for byte in buffer.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state & 0xFF) as u8;
    }

    match copy_to_user(pointer, &buffer) {
        Ok(()) => length as i64,
        Err(failure) => failure,
    }
}

const _: i64 = EFAULT;

/// `fork`: çocuğun pid'i ebeveyne, 0 çocuğa döner.
pub fn fork() -> i64 {
    crate::task::fork()
}

/// `wait4`: zombi çocuğu toplar; durum `*status` içine yazılır.
pub fn wait4(pid: i64, status_pointer: u64, options: u32) -> i64 {
    crate::task::wait4(pid, status_pointer, options)
}

/// `gettimeofday(struct timeval *tv, struct timezone *tz)`.
pub fn gettimeofday(timeval: u64, timezone: u64) -> i64 {
    let nanos = crate::time::monotonic_nanos();
    if timeval != 0 {
        let seconds = (nanos / 1_000_000_000) as i64;
        let micros = ((nanos % 1_000_000_000) / 1000) as i64;
        let mut buffer = [0u8; 16];
        buffer[..8].copy_from_slice(&seconds.to_le_bytes());
        buffer[8..].copy_from_slice(&micros.to_le_bytes());
        if let Err(code) = crate::syscall::copy_to_user(timeval, &buffer) {
            return code;
        }
    }
    if timezone != 0 {
        let zeros = [0u8; 8];
        let _ = crate::syscall::copy_to_user(timezone, &zeros);
    }
    0
}

/// `nanosleep(req, rem)`: işbirlikli bekleme.
pub fn nanosleep(request: u64, remaining: u64) -> i64 {
    let Ok(data) = crate::syscall::copy_from_user(request, 16) else {
        return crate::syscall::EFAULT;
    };
    let seconds = i64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]);
    let nanos = i64::from_le_bytes([
        data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
    ]);
    if seconds < 0 || nanos < 0 {
        return crate::syscall::EINVAL;
    }
    let total = (seconds as u64) * 1_000_000_000 + nanos as u64;
    let deadline = crate::time::monotonic_nanos() + total;
    while crate::time::monotonic_nanos() < deadline {
        if crate::task::signal::interrupted() {
            if remaining != 0 {
                let left = deadline - crate::time::monotonic_nanos();
                let mut buffer = [0u8; 16];
                buffer[..8].copy_from_slice(&((left / 1_000_000_000) as i64).to_le_bytes());
                buffer[8..].copy_from_slice(&((left % 1_000_000_000) as i64).to_le_bytes());
                let _ = crate::syscall::copy_to_user(remaining, &buffer);
            }
            return crate::syscall::EINTR;
        }
        crate::task::schedule();
    }
    0
}

/// `sysinfo`: bellek istatistikleri.
pub fn sysinfo(pointer: u64) -> i64 {
    let mut buffer = [0u8; 112];
    let total = crate::mem::pmm::total_frames() as u64 * 4096;
    buffer[32..40].copy_from_slice(&total.to_le_bytes()); // totalram
    buffer[40..48].copy_from_slice(&(total / 2).to_le_bytes()); // freeram
    buffer[48..56].copy_from_slice(&(total / 2).to_le_bytes()); // sharedram
    buffer[104..108].copy_from_slice(&1u32.to_le_bytes()); // procs
    buffer[108..112].copy_from_slice(&4096u32.to_le_bytes()); // mem_unit
    match crate::syscall::copy_to_user(pointer, &buffer) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

/// `sched_getaffinity`: tek CPU.
pub fn sched_getaffinity(_pid: i64, size: usize, mask: u64) -> i64 {
    if size < 8 {
        return crate::syscall::EINVAL;
    }
    let bits = 1u64.to_le_bytes();
    match crate::syscall::copy_to_user(mask, &bits) {
        Ok(()) => 8,
        Err(code) => code,
    }
}

/// `clock_nanosleep(clockid, flags, request, remaining)`; `TIMER_ABSTIME` destekli.
pub fn clock_nanosleep(clock: i32, flags: u32, request: u64, remaining: u64) -> i64 {
    const TIMER_ABSTIME: u32 = 1;
    let _ = clock;
    let Ok(data) = crate::syscall::copy_from_user(request, 16) else {
        return crate::syscall::EFAULT;
    };
    let seconds = i64::from_le_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]);
    let nanos = i64::from_le_bytes([
        data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
    ]);
    if seconds < 0 || nanos < 0 {
        return crate::syscall::EINVAL;
    }
    let requested = (seconds as u64) * 1_000_000_000 + nanos as u64;
    let deadline = if flags & TIMER_ABSTIME != 0 {
        requested
    } else {
        crate::time::monotonic_nanos() + requested
    };

    loop {
        let now = crate::time::monotonic_nanos();
        if now >= deadline {
            return 0;
        }
        if crate::task::signal::interrupted() {
            if remaining != 0 && flags & TIMER_ABSTIME == 0 {
                let left = deadline - now;
                let mut buffer = [0u8; 16];
                buffer[..8].copy_from_slice(&((left / 1_000_000_000) as i64).to_le_bytes());
                buffer[8..].copy_from_slice(&((left % 1_000_000_000) as i64).to_le_bytes());
                let _ = crate::syscall::copy_to_user(remaining, &buffer);
            }
            return crate::syscall::EINTR;
        }
        crate::task::schedule();
    }
}

/// `getrusage`: sıfır dolu sayaçlar (zaman ölçümü yok).
pub fn getrusage(_who: i32, pointer: u64) -> i64 {
    let zeros = [0u8; 144];
    match crate::syscall::copy_to_user(pointer, &zeros) {
        Ok(()) => 0,
        Err(code) => code,
    }
}
