//! Sinyaller: eylem tablosu, bekleyen küme, teslimat ve `rt_sigreturn`.
//!
//! Teslimat syscall dönüşünde yapılır: kullanıcı yığınına bir çerçeve
//! kurulur, RIP işleyiciye çevrilir, işleyici `ret` ile glibc'nin
//! `__restore_rt`'sine döner ve o da `rt_sigreturn` çağırır.

use core::sync::atomic::Ordering;

use super::Process;

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

pub const SA_NODEFER: u64 = 0x4000_0000;
pub const SA_SIGINFO: u64 = 4;

pub const SIGKILL: u32 = 9;
pub const SIGSTOP: u32 = 19;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGWINCH: u32 = 28;
pub const SIGURG: u32 = 23;
pub const SIGTSTP: u32 = 20;
pub const SIGTTIN: u32 = 21;
pub const SIGTTOU: u32 = 22;

const SIGFRAME_MAGIC: u64 = 0x5157_5f41_5243_484f; // "QW_ARCHO"
const SIGFRAME_WORDS: usize = 22;

#[derive(Clone, Copy)]
pub struct SignalAction {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: u64,
}

impl Default for SignalAction {
    fn default() -> Self {
        Self {
            handler: SIG_DFL,
            flags: 0,
            restorer: 0,
            mask: 0,
        }
    }
}

/// Süreç (ve iş parçacığı grubu) başına sinyal durumu.
pub struct SignalState {
    pub pending: core::sync::atomic::AtomicU64,
    pub blocked: core::sync::atomic::AtomicU64,
    pub actions: crate::sync::SpinLock<[SignalAction; 64]>,
}

impl SignalAction {
    pub const DEFAULT: Self = Self {
        handler: SIG_DFL,
        flags: 0,
        restorer: 0,
        mask: 0,
    };
}

impl Default for SignalState {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalState {
    pub const fn new() -> Self {
        Self {
            pending: core::sync::atomic::AtomicU64::new(0),
            blocked: core::sync::atomic::AtomicU64::new(0),
            actions: crate::sync::SpinLock::new([SignalAction::DEFAULT; 64]),
        }
    }

    pub fn snapshot(&self) -> Self {
        Self {
            pending: core::sync::atomic::AtomicU64::new(0),
            blocked: core::sync::atomic::AtomicU64::new(self.blocked.load(Ordering::Relaxed)),
            actions: crate::sync::SpinLock::new(*self.actions.lock()),
        }
    }
}

pub fn bit(signal: u32) -> u64 {
    if (1..=64).contains(&signal) {
        1u64 << (signal - 1)
    } else {
        0
    }
}

/// İşleyicisi olmayan, varsayılanı "yok say" olan sinyaller.
fn default_ignored(signal: u32) -> bool {
    matches!(
        signal,
        SIGCHLD | SIGCONT | SIGWINCH | SIGURG | SIGSTOP
    )
}

pub fn action(process: &Process, signal: u32) -> SignalAction {
    let actions = process.signals.actions.lock();
    match actions.get((signal as usize).saturating_sub(1)) {
        Some(action) => *action,
        None => SignalAction::default(),
    }
}

pub fn set_action(process: &Process, signal: u32, value: SignalAction) {
    if signal == 0 || signal > 64 {
        return;
    }
    let mut actions = process.signals.actions.lock();
    if let Some(slot) = actions.get_mut(signal as usize - 1) {
        *slot = value;
    }
}

pub fn reset_actions(process: &Process) {
    let mut actions = process.signals.actions.lock();
    for slot in actions.iter_mut() {
        *slot = SignalAction::default();
    }
}

pub fn set_mask(process: &Process, how: u32, mask: u64) {
    // SIGKILL ve SIGSTOP maskelenemez.
    let mask = mask & !bit(SIGKILL) & !bit(SIGSTOP);
    let current = process.signals.blocked.load(Ordering::Relaxed);
    let updated = match how {
        0 => current | mask,            // SIG_BLOCK
        1 => current & !mask,           // SIG_UNBLOCK
        2 => mask,                      // SIG_SETMASK
        _ => current,
    };
    process.signals.blocked.store(updated, Ordering::Relaxed);
}

pub fn mask(process: &Process) -> u64 {
    process.signals.blocked.load(Ordering::Relaxed)
}

/// Hedef sürece sinyal bırakır. `0` döner ya da negatif hata kodu.
pub fn send(pid: u64, signal: u32) -> i64 {
    if signal > 64 {
        return -22; // EINVAL
    }
    if pid == 0 {
        if let Some(process) = super::current() {
            if signal != 0 {
                process.signals.pending.fetch_or(bit(signal), Ordering::Relaxed);
            }
            return 0;
        }
        return -3;
    }

    let table = super::TABLE.lock();
    let Some(target) = table.iter().find(|process| process.pid == pid) else {
        return -3; // ESRCH
    };
    if signal != 0 {
        target.signals.pending.fetch_or(bit(signal), Ordering::Relaxed);
    }
    0
}

/// İş parçacığı çıkışında `CLONE_CHILD_CLEARTID` adresini sıfırlar. Bekleyen
/// `pthread_join` futex'i bu adresi yokladığı için ek uyandırma gerekmez.
pub fn thread_exit(process: &Process) {
    let address = process
        .clear_child_tid
        .load(Ordering::Relaxed);
    if address != 0 {
        let _ = crate::syscall::write_user_u32(address, 0);
    }
}

/// Çocuk durduğunda/çıktığında ebeveyne `SIGCHLD` bırakır.
pub fn notify_parent(process: &Process) {
    if process.thread || process.ppid == 0 {
        return;
    }
    for parent in super::processes() {
        if parent.pid == process.ppid {
            parent
                .signals
                .pending
                .fetch_or(bit(SIGCHLD), Ordering::Relaxed);
            return;
        }
    }
}

/// Bir süreç grubuna sinyal gönderir; grup 0 ise tüm süreçlere.
pub fn broadcast(group: u64, signal: u32) {
    if signal == 0 || signal > 64 {
        return;
    }
    for process in super::processes() {
        if group != 0
            && process.pgid.load(Ordering::Relaxed) != group
            && process.pid != group
        {
            continue;
        }
        process
            .signals
            .pending
            .fetch_or(bit(signal), Ordering::Relaxed);
    }
}

/// Süreçte teslim edilmeyi bekleyen ilk sinyal.
pub fn pending_unblocked(process: &Process) -> Option<u32> {
    let pending = process.signals.pending.load(Ordering::Relaxed);
    let blocked = process.signals.blocked.load(Ordering::Relaxed);
    let ready = pending & !blocked & !bit(0);
    if ready == 0 {
        return None;
    }
    Some(ready.trailing_zeros() + 1)
}

/// Bloklayan bir syscall kesilmeli mi?
pub fn interrupted() -> bool {
    match super::current() {
        Some(process) => pending_unblocked(&process).is_some(),
        None => false,
    }
}

fn clear_pending(process: &Process, signal: u32) {
    process.signals.pending.fetch_and(!bit(signal), Ordering::Relaxed);
}

/// Varsayılan eylem: çoğu sinyal süreci sonlandırır.
fn default_action(process: &Process, signal: u32) {
    if default_ignored(signal) {
        clear_pending(process, signal);
        return;
    }
    super::exit_process(process, 128 + signal as i32);
}

/// Syscall dönüş çerçevesine sinyal yerleştirir. `true` ise RAX sıfırlanmalı.
///
/// `frame` yerleşimi `archon_syscall_entry` ile aynı: 0 RAX, 16 RIP,
/// 24 RDX, 32 RSI, 40 RDI, 80 RFLAGS, 120 RSP.
pub fn deliver(frame: *mut u64, result: i64) -> bool {
    let Some(process) = super::current() else {
        return false;
    };
    let Some(signal) = pending_unblocked(&process) else {
        return false;
    };

    // İş kontrolü: durdurma/devam sinyalleri yakalanamaz.
    if signal == SIGCONT {
        super::resume(&process);
        clear_pending(&process, signal);
        return false;
    }
    if signal == SIGSTOP || signal == SIGTSTP || signal == SIGTTIN || signal == SIGTTOU {
        let action = action(&process, signal);
        let stops = signal == SIGSTOP
            || (action.handler == SIG_DFL && !default_ignored(signal));
        if stops {
            clear_pending(&process, signal);
            super::stop_process(&process);
            return false;
        }
    }

    let action = action(&process, signal);

    if signal == SIGKILL {
        default_action(&process, signal);
        return false;
    }
    if action.handler == SIG_IGN {
        clear_pending(&process, signal);
        return false;
    }
    if action.handler == SIG_DFL {
        default_action(&process, signal);
        return false;
    }
    if action.restorer == 0 {
        // Geri dönüş adresi yok: işleyici çalıştırılamaz, sinyali yok say.
        clear_pending(&process, signal);
        return false;
    }

    // Çerçeve dizinleri: 0 RAX, 1 RBX, 2 RCX(RIP), 3 RDX, 4 RSI, 5 RDI,
    // 6 RBP, 7 R8, 8 R9, 9 R10, 10 R11(RFLAGS), 11 R12, 12 R13, 13 R14,
    // 14 R15, 15 RSP.
    let mut saved = [0u64; 16];
    unsafe {
        for (index, slot) in saved.iter_mut().enumerate() {
            *slot = *frame.add(index);
        }
    }

    let user_rsp = saved[15];
    let user_rip = saved[2];
    let user_flags = saved[10];

    // İşleyici girişinde RSP ≡ 8 (mod 16). Çerçeve (184) + siginfo (128)
    // toplam 312 bayt; hepsi özgün RSP'nin altına yerleşmeli, yoksa
    // çağıranın canlı yığın çerçevesi ezilir.
    let entry_rsp = (user_rsp & !0xF).wrapping_sub(312);
    // 184 bayt çerçeve öneki + 128 bayt siginfo (SA_SIGINFO işleyicileri için).
    let mut buffer = [0u8; 312];
    buffer[0..8].copy_from_slice(&action.restorer.to_le_bytes());

    let words: [u64; SIGFRAME_WORDS] = [
        SIGFRAME_MAGIC,
        signal as u64,
        user_flags | 0x202,
        user_rip,
        user_rsp,
        result as u64,
        saved[1],  // rbx
        saved[2],  // rcx
        saved[3],  // rdx
        saved[4],  // rsi
        saved[5],  // rdi
        saved[6],  // rbp
        saved[7],  // r8
        saved[8],  // r9
        saved[9],  // r10
        saved[10], // r11
        saved[11], // r12
        saved[12], // r13
        saved[13], // r14
        saved[14], // r15
        process.signals.blocked.load(Ordering::Relaxed),
        action.mask,
    ];
    for (index, word) in words.iter().enumerate() {
        let start = 8 + index * 8;
        buffer[start..start + 8].copy_from_slice(&word.to_le_bytes());
    }

    // siginfo_t: si_signo, si_errno, si_code, ardından si_pid/si_uid.
    let siginfo = 184usize;
    buffer[siginfo..siginfo + 4].copy_from_slice(&(signal as i32).to_le_bytes());
    buffer[siginfo + 8..siginfo + 12].copy_from_slice(&0i32.to_le_bytes()); // SI_USER
    buffer[siginfo + 16..siginfo + 20].copy_from_slice(&0i32.to_le_bytes()); // si_pid

    if crate::syscall::copy_to_user(entry_rsp, &buffer).is_err() {
        clear_pending(&process, signal);
        return false;
    }

    unsafe {
        *frame.add(2) = action.handler;                  // RIP
        *frame.add(15) = entry_rsp;                      // RSP
        *frame.add(5) = signal as u64;                   // RDI = signum
        *frame.add(4) = if action.flags & SA_SIGINFO != 0 {
            entry_rsp + siginfo as u64                      // RSI = siginfo
        } else {
            0
        };
        *frame.add(3) = 0;                               // RDX = ucontext
        *frame.add(10) = (user_flags & !0x400) | 0x202;  // RFLAGS
    }

    clear_pending(&process, signal);
    let mut blocked = process.signals.blocked.load(Ordering::Relaxed);
    if action.flags & SA_NODEFER == 0 {
        blocked |= bit(signal);
    }
    blocked |= action.mask;
    process.signals.blocked.store(blocked & !bit(SIGKILL) & !bit(SIGSTOP), Ordering::Relaxed);

    true
}

/// `rt_sigreturn`: kullanıcı yığınındaki çerçeveyi geri yükler.
pub fn sigreturn(frame: *mut u64) -> i64 {
    let Some(process) = super::current() else {
        return -3;
    };
    let user_rsp = unsafe { *frame.add(15) };
    let Ok(data) = crate::syscall::copy_from_user(user_rsp, SIGFRAME_WORDS * 8) else {
        return -14; // EFAULT
    };

    let mut words = [0u64; SIGFRAME_WORDS];
    for (index, word) in words.iter_mut().enumerate() {
        *word = u64::from_le_bytes(data[index * 8..index * 8 + 8].try_into().unwrap());
    }

    if words[0] != SIGFRAME_MAGIC {
        return -22; // EINVAL
    }

    unsafe {
        *frame.add(1) = words[6];   // rbx
        *frame.add(2) = words[3];   // rip (RCX)
        *frame.add(3) = words[8];   // rdx
        *frame.add(4) = words[9];   // rsi
        *frame.add(5) = words[10];  // rdi
        *frame.add(6) = words[11];  // rbp
        *frame.add(7) = words[12];  // r8
        *frame.add(8) = words[13];  // r9
        *frame.add(9) = words[14];  // r10
        *frame.add(10) = words[2];  // r11 = rflags
        *frame.add(11) = words[16]; // r12
        *frame.add(12) = words[17]; // r13
        *frame.add(13) = words[18]; // r14
        *frame.add(14) = words[19]; // r15
        *frame.add(15) = words[4];  // rsp
    }

    process
        .signals
        .blocked
        .store(words[20] & !bit(SIGKILL) & !bit(SIGSTOP), Ordering::Relaxed);

    words[5] as i64 // rax
}
