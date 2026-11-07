//! Sinyal syscall'ları: `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`,
//! `kill`, `tkill`, `tgkill`.

use crate::syscall::{copy_from_user, copy_to_user, EFAULT, EINVAL, ESRCH};
use crate::task::signal::{self, SignalAction};

const SIGACTION_SIZE: usize = 32;

fn current() -> Option<alloc::sync::Arc<crate::task::Process>> {
    crate::task::current()
}

/// 13: `rt_sigaction(sig, act, oldact, sigsetsize)`.
///
/// Çekirdek ABI düzeni: `{ handler, flags, restorer, mask }`.
pub fn sigaction(signal_number: u64, act: u64, oldact: u64, _sigsetsize: u64) -> i64 {
    let number = signal_number as u32;
    if number == 0 || number > 64 || number == signal::SIGKILL || number == signal::SIGSTOP {
        return EINVAL;
    }
    let Some(process) = current() else {
        return ESRCH;
    };

    if oldact != 0 {
        let existing = signal::action(&process, number);
        let mut buffer = [0u8; SIGACTION_SIZE];
        buffer[0..8].copy_from_slice(&existing.handler.to_le_bytes());
        buffer[8..16].copy_from_slice(&existing.flags.to_le_bytes());
        buffer[16..24].copy_from_slice(&existing.restorer.to_le_bytes());
        buffer[24..32].copy_from_slice(&existing.mask.to_le_bytes());
        if let Err(error) = copy_to_user(oldact, &buffer) {
            return error;
        }
    }

    if act != 0 {
        let Ok(data) = copy_from_user(act, SIGACTION_SIZE) else {
            return EFAULT;
        };
        let word = |index: usize| {
            u64::from_le_bytes(data[index * 8..index * 8 + 8].try_into().unwrap())
        };
        signal::set_action(
            &process,
            number,
            SignalAction {
                handler: word(0),
                flags: word(1),
                restorer: word(2),
                mask: word(3) & !signal::bit(number),
            },
        );
    }

    0
}

/// 14: `rt_sigprocmask(how, set, oldset, sigsetsize)`.
pub fn sigprocmask(how: u64, set: u64, oldset: u64, _sigsetsize: u64) -> i64 {
    let Some(process) = current() else {
        return ESRCH;
    };

    if oldset != 0 {
        let mask = signal::mask(&process);
        if let Err(error) = copy_to_user(oldset, &mask.to_le_bytes()) {
            return error;
        }
    }

    if set != 0 {
        let Ok(data) = copy_from_user(set, 8) else {
            return EFAULT;
        };
        let mask = u64::from_le_bytes(data[..8].try_into().unwrap());
        signal::set_mask(&process, how as u32, mask);
    }

    0
}

/// 62: `kill(pid, sig)` — pid <= 0 süreç grubu anlamına gelir.
pub fn kill(pid: i64, signal_number: u32) -> i64 {
    if signal_number > 64 {
        return EINVAL;
    }
    if pid > 0 {
        return signal::send(pid as u64, signal_number);
    }
    // pid == 0: kendi süreç grubumuz. pid < 0: -pid numaralı grup.
    let group = if pid == 0 {
        crate::task::getpgrp() as u64
    } else {
        (-pid) as u64
    };
    let exists = crate::task::processes().iter().any(|process| {
        process
            .pgid
            .load(core::sync::atomic::Ordering::Relaxed)
            == group
            || process.pid == group
    });
    if !exists {
        return ESRCH;
    }
    if signal_number != 0 {
        if pid == 0 {
            let Some(process) = current() else {
                return ESRCH;
            };
            process
                .signals
                .pending
                .fetch_or(signal::bit(signal_number), core::sync::atomic::Ordering::Relaxed);
        }
        signal::broadcast(group, signal_number);
    }
    0
}

/// 200: `tkill(tid, sig)`.
pub fn tkill(tid: i64, signal_number: u32) -> i64 {
    kill(tid, signal_number)
}

/// 234: `tgkill(tgid, tid, sig)`.
pub fn tgkill(_tgid: i64, tid: i64, signal_number: u32) -> i64 {
    kill(tid, signal_number)
}
