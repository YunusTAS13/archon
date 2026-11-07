//! Sistem çağrıları: Linux x86_64 ABI'si.
//!
//! `SYSCALL` girişinde RCX kullanıcı RIP'ini, R11 kullanıcı RFLAGS'ini tutar.
//! Çekirdek yığınına geçiş için tek bir global scratch alanı kullanılıyor;
//! SMP geldiğinde bu alan per-CPU olacak (GS tabanlı).
//!
//! Numara tablosu: https://github.com/torvalds/linux/blob/master/arch/x86/entry/syscalls/syscall_64.tbl
//! Uygulananlar `docs/SYSCALLS.md` içinde işaretli.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::cpu;
use crate::arch::gdt;
use crate::fs::VfsError;

pub mod fs;
pub mod mem;
pub mod proc;
pub mod signal;

pub const ENOSYS: i64 = -38;
pub const EFAULT: i64 = -14;
pub const EINVAL: i64 = -22;
pub const EBADF: i64 = -9;
pub const ENOMEM: i64 = -12;
pub const ENOENT: i64 = -2;
pub const EACCES: i64 = -13;
pub const EPERM: i64 = -1;
pub const ESRCH: i64 = -3;
pub const EAGAIN: i64 = -11;
pub const EINTR: i64 = -4;

#[repr(C)]
pub struct SyscallArgs {
    pub number: u64,
    pub arg1: u64,
    pub arg2: u64,
    pub arg3: u64,
    pub arg4: u64,
    pub arg5: u64,
    pub arg6: u64,
}

static KERNEL_RSP: AtomicU64 = AtomicU64::new(0);

/// Son syscall'lar (numara, kullanıcı RIP) — hata ayıklama için halka tampon.
static HISTORY: [AtomicU64; 16] = [
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
];
static HISTORY_INDEX: AtomicU64 = AtomicU64::new(0);

fn record(number: u64, rip: u64) {
    let index = (HISTORY_INDEX.fetch_add(1, Ordering::Relaxed) as usize) % 8;
    HISTORY[index * 2].store(number, Ordering::Relaxed);
    HISTORY[index * 2 + 1].store(rip, Ordering::Relaxed);
}

/// Panik sırasında son syscall'ları seri porta döker.
pub fn dump_history() {
    crate::println!("syscall gecmisi (son 8):");
    for index in 0..8 {
        let number = HISTORY[index * 2].load(Ordering::Relaxed);
        let rip = HISTORY[index * 2 + 1].load(Ordering::Relaxed);
        if number != 0 || rip != 0 {
            crate::println!("  {} rip={:#x}", number, rip);
        }
    }
}
static SYSCALL_COUNT: AtomicU64 = AtomicU64::new(0);

/// `SYSCALL` sırasında kullanıcı yığını ve callee-saved register'lar burada
/// saklanır. SMP'de per-CPU olacak.
#[used]
#[no_mangle]
static archon_syscall_scratch: [AtomicU64; 9] = [
    AtomicU64::new(0), // 0: kullanıcı RSP
    AtomicU64::new(0), // 1: R12
    AtomicU64::new(0), // 2: R13
    AtomicU64::new(0), // 3: R14
    AtomicU64::new(0), // 4: RBX
    AtomicU64::new(0), // 5: RBP
    AtomicU64::new(0), // 6: R15
    AtomicU64::new(0), // 7: kullanıcı RIP (RCX)
    AtomicU64::new(0), // 8: kullanıcı RFLAGS (R11)
];

#[used]
#[no_mangle]
static archon_syscall_stack: AtomicU64 = AtomicU64::new(0);

extern "C" {
    fn archon_syscall_entry();
}


core::arch::global_asm!(
    r#"
    .att_syntax prefix
    .global archon_syscall_entry
archon_syscall_entry:
    # `fork` için kullanıcı bağlamı scratch'e yazılır (fork bloklamaz).
    movq %rsp, archon_syscall_scratch+0(%rip)
    movq %r12, archon_syscall_scratch+8(%rip)
    movq %r13, archon_syscall_scratch+16(%rip)
    movq %r14, archon_syscall_scratch+24(%rip)
    movq %rbx, archon_syscall_scratch+32(%rip)
    movq %rbp, archon_syscall_scratch+40(%rip)
    movq %r15, archon_syscall_scratch+48(%rip)
    movq %rcx, archon_syscall_scratch+56(%rip)
    movq %r11, archon_syscall_scratch+64(%rip)

    # Dönüş bağlamı sürecin çekirdek yığınında tutulur: bloklayan bir
    # syscall sırasında başka süreç scratch'i ezse bile bu kopya bozulmaz.
    movq archon_syscall_stack(%rip), %rsp
    andq $-16, %rsp
    subq $192, %rsp

    movq %rax, 0(%rsp)
    movq %rbx, 8(%rsp)
    movq %rcx, 16(%rsp)         # kullanıcı RIP
    movq %rdx, 24(%rsp)
    movq %rsi, 32(%rsp)
    movq %rdi, 40(%rsp)
    movq %rbp, 48(%rsp)
    movq %r8, 56(%rsp)
    movq %r9, 64(%rsp)
    movq %r10, 72(%rsp)
    movq %r11, 80(%rsp)         # kullanıcı RFLAGS
    movq %r12, 88(%rsp)
    movq %r13, 96(%rsp)
    movq %r14, 104(%rsp)
    movq %r15, 112(%rsp)
    movq archon_syscall_scratch+0(%rip), %rax
    movq %rax, 120(%rsp)        # kullanıcı RSP

    # SyscallArgs (dispatch'e tek işaretçi).
    movq 0(%rsp), %rax
    movq %rax, 128(%rsp)
    movq 40(%rsp), %rax
    movq %rax, 136(%rsp)
    movq 32(%rsp), %rax
    movq %rax, 144(%rsp)
    movq 24(%rsp), %rax
    movq %rax, 152(%rsp)
    movq 72(%rsp), %rax
    movq %rax, 160(%rsp)
    movq 56(%rsp), %rax
    movq %rax, 168(%rsp)
    movq 64(%rsp), %rax
    movq %rax, 176(%rsp)
    leaq 128(%rsp), %rdi
    call archon_syscall_dispatch

    # Linux ABI: yalnızca RAX (dönüş değeri), RCX ve R11 değişir; diğer tüm
    # register'lar korunur. glibc buna güveniyor.
    movq %rax, 0(%rsp)


    movq 8(%rsp), %rbx
    movq 24(%rsp), %rdx
    movq 32(%rsp), %rsi
    movq 40(%rsp), %rdi
    movq 48(%rsp), %rbp
    movq 56(%rsp), %r8
    movq 64(%rsp), %r9
    movq 72(%rsp), %r10
    movq 88(%rsp), %r12
    movq 96(%rsp), %r13
    movq 104(%rsp), %r14
    movq 112(%rsp), %r15
    movq 16(%rsp), %rcx
    movq 80(%rsp), %r11
    movq 0(%rsp), %rax
    movq 120(%rsp), %rsp
    sysretq
"#
);

core::arch::global_asm!(
    r#"
    .text
"#
);

pub fn init(kernel_stack_top: u64) {
    KERNEL_RSP.store(kernel_stack_top, Ordering::Relaxed);
    archon_syscall_stack.store(kernel_stack_top, Ordering::Relaxed);

    unsafe {
        cpu::enable_syscall();
        cpu::write_msr(cpu::MSR_LSTAR, archon_syscall_entry as *const () as u64);
        cpu::write_msr(cpu::MSR_SFMASK, (1 << 9) | (1 << 10) | (1 << 8) | (1 << 18));

        let _ = gdt::USER_CODE_RPL3;
    }

    println!("syscall: LSTAR kuruldu (Linux x86_64 ABI)");
}

/// Süreç değişiminde çağrılır: syscall girişinin kullanacağı çekirdek yığını.
pub fn set_kernel_stack(top: u64) {
    archon_syscall_stack.store(top, Ordering::Relaxed);
}

/// Syscall girişinde kaydedilen kullanıcı register'ları (`fork` için).
pub struct SavedUserRegisters {
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

pub fn saved_user_registers() -> SavedUserRegisters {
    let slot = |index: usize| archon_syscall_scratch[index].load(Ordering::Relaxed);
    SavedUserRegisters {
        rip: slot(7),
        rsp: slot(0),
        rflags: slot(8),
        rbx: slot(4),
        rbp: slot(5),
        r12: slot(1),
        r13: slot(2),
        r14: slot(3),
        r15: slot(6),
    }
}

pub fn count() -> u64 {
    SYSCALL_COUNT.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn archon_syscall_dispatch(args: *const SyscallArgs) -> i64 {
    let args = unsafe { &*args };
    SYSCALL_COUNT.fetch_add(1, Ordering::Relaxed);

    record(args.number, saved_user_registers().rip);

    // Çerçeve yerleşimi: 0 RAX, 1 RBX, 2 RCX(RIP), 3 RDX, 4 RSI, 5 RDI,
    // 6 RBP, 7 R8, 8 R9, 9 R10, 10 R11(RFLAGS), 11 R12, 12 R13, 13 R14,
    // 14 R15, 15 RSP, 16.. SyscallArgs. `rt_sigreturn` ve sinyal teslimatı
    // kullanıcı bağlamını bu çerçeve üzerinden değiştirir.
    let frame = args as *const SyscallArgs as *mut u64;
    let frame = unsafe { frame.sub(16) };


    let result = match args.number {
        0 => fs::read(args.arg1 as i32, args.arg2 as u64, args.arg3 as usize),
        1 => fs::write(args.arg1 as i32, args.arg2 as u64, args.arg3 as usize),
        2 => fs::open(args.arg1 as u64, args.arg2 as u64, args.arg3 as u32),
        3 => fs::close(args.arg1 as i32),
        5 => fs::fstat(args.arg1 as i32, args.arg2),
        7 => fs::poll(args.arg1, args.arg2, args.arg3 as i64),
        23 => fs::select(args.arg1 as i32, args.arg2, args.arg3, args.arg4, args.arg5),
        270 => fs::pselect6(args.arg1 as i32, args.arg2, args.arg3, args.arg4, args.arg5),
        271 => fs::ppoll(args.arg1, args.arg2, args.arg3),
        17 => fs::pread64(args.arg1 as i32, args.arg2, args.arg3 as usize, args.arg4),
        19 => fs::readv(args.arg1 as i32, args.arg2, args.arg3 as usize),
        20 => fs::writev(args.arg1 as i32, args.arg2, args.arg3 as usize),
        21 => fs::access(args.arg1, args.arg2 as u32),
        269 => fs::access(args.arg2, args.arg3 as u32), // faccessat
        439 => fs::access(args.arg2, args.arg3 as u32), // faccessat2
        22 => fs::pipe2(args.arg1, args.arg2 as u32),
        32 => fs::dup(args.arg1 as i32),
        33 => fs::dup2(args.arg1 as i32, args.arg2 as i32),
        72 => fs::fcntl(args.arg1 as i32, args.arg2 as u32, args.arg3),
        293 => fs::pipe2(args.arg1, args.arg2 as u32),
        8 => fs::lseek(args.arg1 as i32, args.arg2 as i64, args.arg3 as u32),
        9 => mem::mmap(
            args.arg1,
            args.arg2 as usize,
            args.arg3,
            args.arg4,
            args.arg5 as i32,
            args.arg6 as i64,
        ),
        10 => mem::mprotect(args.arg1, args.arg2 as usize, args.arg3),
        11 => mem::munmap(args.arg1, args.arg2 as usize),
        12 => mem::brk(args.arg1),
        13 => signal::sigaction(args.arg1, args.arg2, args.arg3, args.arg4),
        14 => signal::sigprocmask(args.arg1, args.arg2, args.arg3, args.arg4),
        15 => crate::task::signal::sigreturn(frame),
        62 => signal::kill(args.arg1 as i64, args.arg2 as u32),
        131 => 0, // sigaltstack
        200 => signal::tkill(args.arg1 as i64, args.arg2 as u32),
        234 => signal::tgkill(args.arg1 as i64, args.arg2 as i64, args.arg3 as u32),
        16 => fs::ioctl(args.arg1 as i32, args.arg2, args.arg3),
        39 => proc::getpid(),
        56 | 57 => crate::task::clone(args.arg1, args.arg2, args.arg3, args.arg4, args.arg5),
        61 => proc::wait4(args.arg1 as i64, args.arg2, args.arg3 as u32),
        59 => fs::execve(args.arg1, args.arg2, args.arg3),
        60 | 231 => proc::exit(args.arg1 as i32),
        63 => proc::uname(args.arg1),
        102 => 0, // getuid
        104 => 0, // getgid
        107 => 0, // geteuid
        108 => 0, // getegid
        109 => crate::task::setpgid(args.arg1 as i64, args.arg2 as i64),
        110 => proc::getppid(),
        111 => crate::task::getpgrp(),
        112 => crate::task::setsid(),
        121 => {
            let fallback = crate::task::getpgrp() as u64;
            crate::task::pgid_of(args.arg1 as i64, fallback) as i64
        }
        124 => proc::getpid(), // getsid
        158 => proc::arch_prctl(args.arg1 as i32, args.arg2),
        218 => proc::set_tid_address(args.arg1),
        186 => proc::gettid(),
        24 => proc::sched_yield(),
        273 => 0, // set_robust_list
        232 => fs::epoll_wait(args.arg1 as i32, args.arg2, args.arg3 as i32, args.arg4 as i32),
        233 => fs::epoll_ctl(args.arg1 as i32, args.arg2 as u32, args.arg3 as i32, args.arg4),
        281 => fs::epoll_wait(args.arg1 as i32, args.arg2, args.arg3 as i32, args.arg4 as i32),
        291 => fs::epoll_create1(args.arg1 as u32),
        284 => fs::eventfd2(args.arg1 as u32, 0),
        290 => fs::eventfd2(args.arg1 as u32, args.arg2 as u32),
        283 => fs::timerfd_create(args.arg1 as i32, args.arg2 as u32),
        286 => fs::timerfd_settime(args.arg1 as i32, args.arg2 as u32, args.arg3, args.arg4),
        287 => fs::timerfd_gettime(args.arg1 as i32, args.arg2),
        319 => fs::memfd_create(args.arg1, args.arg2 as u32),
        41 => fs::socket(args.arg1 as u32, args.arg2 as u32, args.arg3 as u32),
        42 => fs::connect(args.arg1 as i32, args.arg2, args.arg3 as usize),
        43 => fs::accept4(args.arg1 as i32, args.arg2, args.arg3, 0),
        288 => fs::accept4(args.arg1 as i32, args.arg2, args.arg3, args.arg4 as u32),
        44 => fs::sendto(
            args.arg1 as i32,
            args.arg2,
            args.arg3 as usize,
            args.arg4 as u32,
            args.arg5,
            args.arg6 as usize,
        ),
        45 => fs::recvfrom(
            args.arg1 as i32,
            args.arg2,
            args.arg3 as usize,
            args.arg4 as u32,
            args.arg5,
            args.arg6,
        ),
        46 => fs::sendmsg(args.arg1 as i32, args.arg2, args.arg3 as u32),
        47 => fs::recvmsg(args.arg1 as i32, args.arg2, args.arg3 as u32),
        48 => fs::shutdown(args.arg1 as i32, args.arg2 as u32),
        49 => fs::bind(args.arg1 as i32, args.arg2, args.arg3 as usize),
        50 => fs::listen(args.arg1 as i32, args.arg2 as i32),
        51 => fs::getsockname(args.arg1 as i32, args.arg2, args.arg3),
        52 => fs::getpeername(args.arg1 as i32, args.arg2, args.arg3),
        54 => fs::setsockopt(args.arg1 as i32, args.arg2 as i32, args.arg3 as i32, args.arg4, args.arg5 as usize),
        55 => fs::getsockopt(args.arg1 as i32, args.arg2 as i32, args.arg3 as i32, args.arg4, args.arg5),
        74 => fs::fsync(args.arg1 as i32, false),
        75 => fs::fsync(args.arg1 as i32, true),
        162 => 0, // sync: her yazma dogrudan diske gidiyor
        306 => 0, // syncfs
        83 => fs::mkdir(args.arg1, args.arg2 as u32),
        84 => fs::rmdir(args.arg1),
        87 => fs::unlink(args.arg1),
        263 => fs::unlinkat(args.arg1 as i32, args.arg2, args.arg3 as u32),
        90 => fs::chmod(args.arg1, args.arg2 as u32),
        91 => fs::fchmod(args.arg1 as i32, args.arg2 as u32),
        268 => fs::fchmodat(args.arg1 as i32, args.arg2, args.arg3 as u32, args.arg4 as u32),
        280 => fs::utimensat(args.arg1 as i32, args.arg2, args.arg3, args.arg4 as u32),
        92 => 0, // chown: tek kullanici, sahiplik degismez
        93 => 0, // fchown
        94 => 0, // lchown
        53 => fs::socketpair(args.arg1 as u32, args.arg2 as u32, args.arg3 as u32, args.arg4),
        77 => fs::ftruncate(args.arg1 as i32, args.arg2 as i64),
        202 => proc::futex(args.arg1, args.arg2 as u32, args.arg3 as u32, args.arg5, args.arg6 as u32),
        228 => proc::clock_gettime(args.arg1 as i32, args.arg2),
        35 => proc::nanosleep(args.arg1, args.arg2),
        230 => proc::clock_nanosleep(args.arg1 as i32, args.arg2 as u32, args.arg3, args.arg4),
        98 => proc::getrusage(args.arg1 as i32, args.arg2),
        28 => 0, // madvise
        157 => 0, // prctl: ad, seccomp ve benzeri istekler yok sayilir
        96 => proc::gettimeofday(args.arg1, args.arg2),
        99 => proc::sysinfo(args.arg1),
        204 => proc::sched_getaffinity(args.arg1 as i64, args.arg2 as usize, args.arg3),
        257 => fs::openat(args.arg1 as i32, args.arg2 as u64, args.arg3 as u32, args.arg4 as u32),
        262 => fs::stat(args.arg1 as i32, args.arg2 as u64, args.arg3 as u64),
        302 => fs::prlimit64(args.arg1 as i32, args.arg2 as u32, args.arg3, args.arg4),
        332 => fs::statx(args.arg1 as i32, args.arg2, args.arg3 as u32, args.arg4 as u32, args.arg5),
        217 => fs::getdents64(args.arg1 as i32, args.arg2 as u64, args.arg3 as usize),
        79 => fs::getcwd(args.arg1 as u64, args.arg2 as usize),
        80 => fs::chdir(args.arg1),
        89 => fs::readlink(args.arg1 as u64, args.arg2 as u64, args.arg3 as usize),
        318 => proc::getrandom(args.arg1, args.arg2 as usize),
        _ => ENOSYS,
    };

    if crate::task::signal::deliver(frame, result) {
        0
    } else {
        result
    }
}

pub fn error(error: VfsError) -> i64 {
    match error {
        VfsError::NotFound => ENOENT,
        VfsError::NotADirectory => -20,
        VfsError::IsADirectory => -21,
        VfsError::PermissionDenied => EACCES,
        VfsError::InvalidArgument => EINVAL,
        VfsError::NoSpace => -28,
        VfsError::Io => -5,
        VfsError::ReadOnly => -30,
        VfsError::Unsupported => ENOSYS,
        VfsError::AlreadyExists => -17,
        VfsError::TooManyOpenFiles => -24,
        VfsError::NotEmpty => -39,
        VfsError::Busy => -16,
        VfsError::Interrupted => -4,
        VfsError::BadDescriptor => EBADF,
        VfsError::AddressInUse => -98,
        VfsError::AlreadyConnected => -106,
        VfsError::ConnectionRefused => -111,
        VfsError::ConnectionAborted => -103,
        VfsError::NotConnected => -107,
        VfsError::BrokenPipe => -32,
    }
}

/// Kullanıcı belleğinden güvenli kopya. Adres aralığının gerçekten eşlenmiş
/// olduğunu doğrular; eşlenmemiş adres EFAULT döner.
pub fn copy_from_user(pointer: u64, length: usize) -> core::result::Result<alloc::vec::Vec<u8>, i64> {
    if pointer == 0 || length > 64 * 1024 * 1024 {
        return Err(EFAULT);
    }
    let pml4 = crate::mem::vmm::current_pml4();
    let start = pointer & !0xFFF;
    let end = (pointer + length as u64 + 0xFFF) & !0xFFF;

    let mut address = start;
    while address < end {
        if crate::mem::vmm::translate(pml4, address).is_none() {
            return Err(EFAULT);
        }
        address += 4096;
    }

    let mut buffer = alloc::vec![0u8; length];
    unsafe {
        core::ptr::copy_nonoverlapping(pointer as *const u8, buffer.as_mut_ptr(), length);
    }
    Ok(buffer)
}

/// Kullanıcı adresinden tek bir `u32` okur (futex sözcüğü gibi).
pub fn read_user_u32(pointer: u64) -> Option<u32> {
    if pointer == 0 || pointer & 3 != 0 {
        return None;
    }
    let pml4 = crate::mem::vmm::current_pml4();
    let physical = crate::mem::vmm::translate(pml4, pointer)?;
    Some(unsafe {
        core::ptr::read_volatile(crate::mem::hhdm::phys_to_ptr::<u32>(physical))
    })
}

/// Kullanıcı adresine tek bir `u32` yazar.
pub fn write_user_u32(pointer: u64, value: u32) -> core::result::Result<(), i64> {
    if pointer == 0 || pointer & 3 != 0 {
        return Err(EFAULT);
    }
    let pml4 = crate::mem::vmm::current_pml4();
    let Some(physical) = crate::mem::vmm::translate(pml4, pointer) else {
        return Err(EFAULT);
    };
    unsafe {
        core::ptr::write_volatile(crate::mem::hhdm::phys_to_ptr::<u32>(physical), value);
    }
    Ok(())
}

pub fn copy_to_user(pointer: u64, data: &[u8]) -> core::result::Result<(), i64> {
    if pointer == 0 {
        return Err(EFAULT);
    }
    let pml4 = crate::mem::vmm::current_pml4();
    let start = pointer & !0xFFF;
    let end = (pointer + data.len() as u64 + 0xFFF) & !0xFFF;

    let mut address = start;
    while address < end {
        if crate::mem::vmm::translate(pml4, address).is_none() {
            return Err(EFAULT);
        }
        address += 4096;
    }

    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), pointer as *mut u8, data.len());
    }
    Ok(())
}
