//! Süreçler ve preemptive round-robin zamanlayıcı.
//!
//! Zaman dilimi 5 tik (50 ms). Kesme ring3'ten geldiyse kesme çerçevesi
//! doğrudan yeniden yazılarak süreç değiştirilir: ek bağlam asm'i gerekmez,
//! çünkü `iretq` zaten tüm kullanıcı durumunu çerçeveden yükler.

use alloc::alloc::{alloc, Layout};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

const KERNEL_STACK_SIZE: usize = 16 * 1024;

use crate::arch::{cpu, gdt, idt};
use crate::fs::DescriptorTable;

pub mod signal;
use crate::sync::SpinLock;

pub const STATE_READY: u8 = 0;
pub const STATE_RUNNING: u8 = 1;
pub const STATE_BLOCKED: u8 = 2;
pub const STATE_ZOMBIE: u8 = 3;
pub const STATE_STOPPED: u8 = 4;

static NEXT_PID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);
static TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static CURRENT: SpinLock<Option<Arc<Process>>> = SpinLock::new(None);
static TABLE: SpinLock<Vec<Arc<Process>>> = SpinLock::new(Vec::new());

pub struct Process {
    pub pid: u64,
    pub ppid: u64,
    pub name: String,
    pub pml4: core::sync::atomic::AtomicU64,
    pub kernel_stack_top: u64,
    pub entry: core::sync::atomic::AtomicU64,
    pub user_stack: core::sync::atomic::AtomicU64,
    /// Program sonu. İş parçacıkları paylaşır.
    pub brk: Arc<core::sync::atomic::AtomicU64>,
    pub state: core::sync::atomic::AtomicU8,
    /// Dosya tanıtıcı tablosu. `CLONE_FILES` ile iş parçacıkları paylaşır.
    pub fd_table: Arc<DescriptorTable>,
    /// Preemption sırasında saklanan kullanıcı durumu.
    pub context: SpinLock<Option<UserContext>>,
    /// Sürecin çekirdek yığını (taban adresi).
    pub kernel_stack: u64,
    /// Çekirdek modunda kesilen sürecin devam noktası.
    pub kernel_context: SpinLock<Context>,
    pub exit_code: core::sync::atomic::AtomicI32,
    /// Sürecin FS tabanı (TLS). Bağlam değişiminde MSR'a yazılır.
    pub fs_base: core::sync::atomic::AtomicU64,
    /// Sinyal durumu. `CLONE_SIGHAND` ile iş parçacıkları paylaşır.
    pub signals: Arc<signal::SignalState>,
    /// İş parçacığı grubu kimliği (ana iş parçacığının pid'i).
    pub tgid: u64,
    /// Süreç grubu (iş kontrolü için).
    pub pgid: core::sync::atomic::AtomicU64,
    /// Oturum kimliği.
    pub sid: core::sync::atomic::AtomicU64,
    /// Durma durumu `wait4(WUNTRACED)` ile bildirildi mi?
    pub stopped_reported: core::sync::atomic::AtomicU8,
    /// Çalıştırılan programın yolu (`/proc/<pid>/exe`).
    pub exe_path: SpinLock<String>,
    /// `clone(CLONE_THREAD)` ile doğdu mu?
    pub thread: bool,
    /// `CLONE_CHILD_CLEARTID`: çıkarken sıfırlanacak kullanıcı adresi.
    pub clear_child_tid: core::sync::atomic::AtomicU64,
}

/// Kesme çerçevesinin kullanıcı kısmı: 15 GPR + RIP/RFLAGS/RSP.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UserContext {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub rip: u64,
    pub rflags: u64,
    pub rsp: u64,
}

impl UserContext {
    pub fn from_frame(frame: &crate::arch::idt::InterruptFrame) -> Self {
        Self {
            r15: frame.r15,
            r14: frame.r14,
            r13: frame.r13,
            r12: frame.r12,
            r11: frame.r11,
            r10: frame.r10,
            r9: frame.r9,
            r8: frame.r8,
            rbp: frame.rbp,
            rdi: frame.rdi,
            rsi: frame.rsi,
            rdx: frame.rdx,
            rcx: frame.rcx,
            rbx: frame.rbx,
            rax: frame.rax,
            rip: frame.rip,
            rflags: frame.rflags,
            rsp: frame.rsp,
        }
    }

    pub fn apply(&self, frame: &mut crate::arch::idt::InterruptFrame) {
        frame.r15 = self.r15;
        frame.r14 = self.r14;
        frame.r13 = self.r13;
        frame.r12 = self.r12;
        frame.r11 = self.r11;
        frame.r10 = self.r10;
        frame.r9 = self.r9;
        frame.r8 = self.r8;
        frame.rbp = self.rbp;
        frame.rdi = self.rdi;
        frame.rsi = self.rsi;
        frame.rdx = self.rdx;
        frame.rcx = self.rcx;
        frame.rbx = self.rbx;
        frame.rax = self.rax;
        frame.rip = self.rip;
        frame.rflags = self.rflags;
        frame.rsp = self.rsp;
    }
}

/// Kaydedilmiş bağlam: callee-saved register'lar + yığın işaretçisi.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Context {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub rip: u64,
}

core::arch::global_asm!(
    r#"
    .att_syntax prefix
    .global archon_switch_context
archon_switch_context:
    movq %r15, 0(%rdi)
    movq %r14, 8(%rdi)
    movq %r13, 16(%rdi)
    movq %r12, 24(%rdi)
    movq %rbx, 32(%rdi)
    movq %rbp, 40(%rdi)
    movq %rsp, 48(%rdi)
    leaq 1f(%rip), %rax
    movq %rax, 56(%rdi)

    movq 0(%rsi), %r15
    movq 8(%rsi), %r14
    movq 16(%rsi), %r13
    movq 24(%rsi), %r12
    movq 32(%rsi), %rbx
    movq 40(%rsi), %rbp
    movq 48(%rsi), %rsp
    movq 56(%rsi), %rax
    jmp *%rax
1:
    ret
"#
);

extern "C" {
    fn archon_switch_context(save: *mut Context, load: *const Context);
}

pub fn init() {
    gdt::set_kernel_stack(crate::syscall_stack_top());
    println!("task: zamanlayici hazir (preemption M3 sonu)");
}

fn allocate_kernel_stack() -> Result<(u64, u64), &'static str> {
    let layout = Layout::from_size_align(KERNEL_STACK_SIZE, 16).map_err(|_| "yigin duzeni")?;
    let base = unsafe { alloc(layout) } as u64;
    if base == 0 {
        return Err("cekirdek yigini ayrilamadi");
    }
    Ok((base, base + KERNEL_STACK_SIZE as u64))
}

/// `Context` alanına kilit tutmadan işaretçi verir. Tek CPU ve kesmeler kapalı
/// olduğu için güvenli; SMP'de per-CPU kilitlenecek.
fn context_pointer(process: &Process) -> *mut Context {
    let guard = process.kernel_context.lock();
    let pointer = &*guard as *const Context as *mut Context;
    drop(guard);
    pointer
}

/// Çekirdek modunda süreç değiştirir. Döndüğünde eski süreç yeniden
/// zamanlanmıştır ve kendi çekirdek yığınından devam eder.
fn switch_to_process(from: &Arc<Process>, next: &Arc<Process>) {
    if from.state.load(core::sync::atomic::Ordering::Relaxed) == STATE_RUNNING {
        from.state
            .store(STATE_READY, core::sync::atomic::Ordering::Relaxed);
    }

    next.state
        .store(STATE_RUNNING, core::sync::atomic::Ordering::Relaxed);
    *CURRENT.lock() = Some(next.clone());

    // TLS tabanı çekirdek MSR'ında; süreç başına saklanıp geri yüklenmeli.
    unsafe {
        from.fs_base.store(
            cpu::read_msr(0xC000_0100),
            core::sync::atomic::Ordering::Relaxed,
        );
        cpu::write_msr(
            0xC000_0100,
            next.fs_base.load(core::sync::atomic::Ordering::Relaxed),
        );
    }

    let pml4 = next.pml4.load(core::sync::atomic::Ordering::Relaxed);
    if pml4 != crate::mem::vmm::current_pml4() {
        crate::mem::vmm::switch_to(pml4);
    }
    gdt::set_kernel_stack(next.kernel_stack_top);
    crate::syscall::set_kernel_stack(next.kernel_stack_top);

    unsafe {
        archon_switch_context(context_pointer(from), context_pointer(next));
    }
}

/// Çalışan süreci bırakır ve sıradaki hazır sürece geçer. Hazır süreç yoksa
/// hemen döner; çağıran yeniden denemeli.
pub fn schedule() {
    let Some(from) = current() else {
        return;
    };
    let Some(next) = pick_next(from.pid) else {
        return;
    };

    switch_to_process(&from, &next);
}

/// İlk süreci başlatır: önyükleme yığınından sürecin çekirdek yığınına geçer.
pub fn start() -> ! {
    let Some(first) = pick_next(0) else {
        idle_loop()
    };

    first
        .state
        .store(STATE_RUNNING, core::sync::atomic::Ordering::Relaxed);
    *CURRENT.lock() = Some(first.clone());

    let pml4 = first.pml4.load(core::sync::atomic::Ordering::Relaxed);
    crate::mem::vmm::switch_to(pml4);
    gdt::set_kernel_stack(first.kernel_stack_top);
    crate::syscall::set_kernel_stack(first.kernel_stack_top);

    let mut bootstrap = Context::default();
    unsafe {
        archon_switch_context(&mut bootstrap, context_pointer(&first));
    }
    unreachable!("ilk surec dondu")
}

/// Hiç çalışmamış sürecin çekirdek bağlamından kullanıcı moduna giriş.
extern "C" fn user_trampoline() -> ! {
    let Some(process) = current() else {
        idle_loop()
    };

    let pml4 = process.pml4.load(core::sync::atomic::Ordering::Relaxed);
    let context = match *process.context.lock() {
        Some(context) => context,
        None => UserContext {
            rip: process.entry.load(core::sync::atomic::Ordering::Relaxed),
            rsp: process.user_stack.load(core::sync::atomic::Ordering::Relaxed),
            rflags: 0x202,
            ..UserContext::default()
        },
    };

    unsafe { enter_user_context(pml4, &context) }
}

/// `UserContext` içeriğiyle ring3'e döner: iretq çerçevesi kurulur, tüm GPR'lar
/// yüklenir. Kesme çerçevesi yeniden yazılarak yapılan preemption'ın aksine
/// burada sıfırdan giriş yapılır.
///
/// # Safety
/// `context` içindeki RIP/RSP kullanıcı adres uzayında eşlenmiş olmalı.
unsafe fn enter_user_context(pml4: u64, context: &UserContext) -> ! {
    // Seçiciler GDT'deki sıraya sabit; aşağıdaki const doğrulamasına bakın.
    core::arch::asm!(
        "mov cr3, {pml4}",
        "push 0x1b",
        "push qword ptr [rdi + 136]",
        "push qword ptr [rdi + 128]",
        "push 0x23",
        "push qword ptr [rdi + 120]",
        "mov r15, qword ptr [rdi + 0]",
        "mov r14, qword ptr [rdi + 8]",
        "mov r13, qword ptr [rdi + 16]",
        "mov r12, qword ptr [rdi + 24]",
        "mov r11, qword ptr [rdi + 32]",
        "mov r10, qword ptr [rdi + 40]",
        "mov r9, qword ptr [rdi + 48]",
        "mov r8, qword ptr [rdi + 56]",
        "mov rbp, qword ptr [rdi + 64]",
        "mov rsi, qword ptr [rdi + 80]",
        "mov rdx, qword ptr [rdi + 88]",
        "mov rcx, qword ptr [rdi + 96]",
        "mov rbx, qword ptr [rdi + 104]",
        "mov rax, qword ptr [rdi + 112]",
        "mov rdi, qword ptr [rdi + 72]",
        "iretq",
        pml4 = in(reg) pml4,
        in("rdi") context as *const UserContext,
        options(noreturn)
    );
}

// GDT seçicileri asm içinde sabit yazıldı; kayarsa derleme durur.
const _: () = assert!(gdt::USER_DATA_RPL3 == 0x1b);
const _: () = assert!(gdt::USER_CODE_RPL3 == 0x23);

/// `fork`: adres alanını kopyalar, çocuğu hazır kuyruğa ekler. Çocuk ilk
/// zamanlandığında `rax = 0` ile kullanıcı moduna döner.
pub fn fork() -> i64 {
    let Some(parent) = current() else {
        return -3; // ESRCH
    };

    let registers = crate::syscall::saved_user_registers();
    let parent_pml4 = parent.pml4.load(core::sync::atomic::Ordering::Relaxed);

    let Some(pml4) = crate::mem::vmm::copy_address_space(parent_pml4) else {
        println!("fork: adres alani kopyalanamadi");
        return -12; // ENOMEM
    };
    let Ok((kernel_stack, kernel_stack_top)) = allocate_kernel_stack() else {
        println!("fork: cekirdek yigini yok");
        return -12;
    };

    let context = UserContext {
        rip: registers.rip,
        rsp: registers.rsp,
        rflags: registers.rflags,
        rbx: registers.rbx,
        rbp: registers.rbp,
        r12: registers.r12,
        r13: registers.r13,
        r14: registers.r14,
        r15: registers.r15,
        rax: 0,
        ..UserContext::default()
    };

    let pid = NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let child = Arc::new(Process {
        pid,
        ppid: parent.pid,
        name: parent.name.clone(),
        pml4: core::sync::atomic::AtomicU64::new(pml4),
        kernel_stack_top,
        entry: core::sync::atomic::AtomicU64::new(
            parent.entry.load(core::sync::atomic::Ordering::Relaxed),
        ),
        user_stack: core::sync::atomic::AtomicU64::new(
            parent
                .user_stack
                .load(core::sync::atomic::Ordering::Relaxed),
        ),
        brk: Arc::new(core::sync::atomic::AtomicU64::new(
            parent.brk.load(core::sync::atomic::Ordering::Relaxed),
        )),
        state: core::sync::atomic::AtomicU8::new(STATE_READY),
        fd_table: Arc::new(parent.fd_table.duplicate()),
        context: SpinLock::new(Some(context)),
        kernel_stack,
        kernel_context: SpinLock::new(Context {
            rip: user_trampoline as usize as u64,
            rsp: kernel_stack_top - 8,
            ..Context::default()
        }),
        exit_code: core::sync::atomic::AtomicI32::new(0),
        fs_base: core::sync::atomic::AtomicU64::new(parent.fs_base.load(
            core::sync::atomic::Ordering::Relaxed,
        )),
        signals: Arc::new(parent.signals.snapshot()),
        tgid: pid,
        thread: false,
        clear_child_tid: core::sync::atomic::AtomicU64::new(0),
        pgid: core::sync::atomic::AtomicU64::new(
            parent.pgid.load(core::sync::atomic::Ordering::Relaxed),
        ),
        sid: core::sync::atomic::AtomicU64::new(
            parent.sid.load(core::sync::atomic::Ordering::Relaxed),
        ),
        stopped_reported: core::sync::atomic::AtomicU8::new(0),
        exe_path: SpinLock::new(parent.exe_path.lock().clone()),
    });

    TABLE.lock().push(child);
    println!("task: fork -> pid {} (ebeveyn {})", pid, parent.pid);

    pid as i64
}

pub const CLONE_VM: u64 = 0x0000_0100;
pub const CLONE_FS: u64 = 0x0000_0200;
pub const CLONE_FILES: u64 = 0x0000_0400;
pub const CLONE_SIGHAND: u64 = 0x0000_0800;
pub const CLONE_THREAD: u64 = 0x0001_0000;
pub const CLONE_SETTLS: u64 = 0x0008_0000;
pub const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
pub const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
pub const CLONE_CHILD_SETTID: u64 = 0x0100_0000;

/// `clone(flags, stack, parent_tid, child_tid, tls)`.
///
/// `CLONE_VM` yoksa tam `fork`. Varsa yeni iş parçacığı: adres alanı, dosya
/// tablosu, sinyal durumu ve `brk` paylaşılır; yalnızca yığın ve TLS ayrıdır.
pub fn clone(flags: u64, stack: u64, parent_tid: u64, child_tid: u64, tls: u64) -> i64 {
    if flags & CLONE_VM == 0 {
        return fork();
    }

    let Some(parent) = current() else {
        return -3; // ESRCH
    };
    let registers = crate::syscall::saved_user_registers();
    let Ok((kernel_stack, kernel_stack_top)) = allocate_kernel_stack() else {
        return -12; // ENOMEM
    };

    let pid = NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let context = UserContext {
        rip: registers.rip,
        rsp: if stack != 0 { stack } else { registers.rsp },
        rflags: registers.rflags,
        rbx: registers.rbx,
        rbp: registers.rbp,
        r12: registers.r12,
        r13: registers.r13,
        r14: registers.r14,
        r15: registers.r15,
        rax: 0,
        ..UserContext::default()
    };

    let fd_table = if flags & CLONE_FILES != 0 {
        parent.fd_table.clone()
    } else {
        Arc::new(parent.fd_table.duplicate())
    };
    let signals = if flags & CLONE_SIGHAND != 0 {
        parent.signals.clone()
    } else {
        Arc::new(parent.signals.snapshot())
    };
    let fs_base = if flags & CLONE_SETTLS != 0 {
        tls
    } else {
        parent.fs_base.load(core::sync::atomic::Ordering::Relaxed)
    };

    let child = Arc::new(Process {
        pid,
        ppid: parent.ppid,
        name: parent.name.clone(),
        pml4: core::sync::atomic::AtomicU64::new(
            parent.pml4.load(core::sync::atomic::Ordering::Relaxed),
        ),
        kernel_stack_top,
        entry: core::sync::atomic::AtomicU64::new(
            parent.entry.load(core::sync::atomic::Ordering::Relaxed),
        ),
        user_stack: core::sync::atomic::AtomicU64::new(registers.rsp),
        brk: parent.brk.clone(),
        state: core::sync::atomic::AtomicU8::new(STATE_READY),
        fd_table,
        context: SpinLock::new(Some(context)),
        kernel_stack,
        kernel_context: SpinLock::new(Context {
            rip: user_trampoline as usize as u64,
            rsp: kernel_stack_top - 8,
            ..Context::default()
        }),
        exit_code: core::sync::atomic::AtomicI32::new(0),
        fs_base: core::sync::atomic::AtomicU64::new(fs_base),
        signals,
        tgid: parent.tgid,
        thread: flags & CLONE_THREAD != 0,
        clear_child_tid: core::sync::atomic::AtomicU64::new(
            if flags & CLONE_CHILD_CLEARTID != 0 {
                child_tid
            } else {
                0
            },
        ),
        pgid: core::sync::atomic::AtomicU64::new(
            parent.pgid.load(core::sync::atomic::Ordering::Relaxed),
        ),
        sid: core::sync::atomic::AtomicU64::new(
            parent.sid.load(core::sync::atomic::Ordering::Relaxed),
        ),
        stopped_reported: core::sync::atomic::AtomicU8::new(0),
        exe_path: SpinLock::new(parent.exe_path.lock().clone()),
    });

    TABLE.lock().push(child);

    let tid = (pid as i32).to_le_bytes();
    if flags & CLONE_PARENT_SETTID != 0 && parent_tid != 0 {
        let _ = crate::syscall::copy_to_user(parent_tid, &tid);
    }
    if flags & CLONE_CHILD_SETTID != 0 && child_tid != 0 {
        let _ = crate::syscall::copy_to_user(child_tid, &tid);
    }

    println!(
        "task: clone -> tid {} (tgid {}, iplik {})",
        pid,
        parent.tgid,
        flags & CLONE_THREAD != 0
    );

    pid as i64
}

/// `wait4`: zombi çocuğu toplar. Çocuk yoksa ECHILD, `WNOHANG` ile hiç
/// beklemeden 0 döner. Aksi halde çocuk çıkana kadar zamanlayıcıya bırakır.
pub fn wait4(expected_pid: i64, status_pointer: u64, options: u32) -> i64 {
    const WNOHANG: u32 = 1;
    const WUNTRACED: u32 = 2;
    const WCONTINUED: u32 = 8;
    let parent_pid = current_pid();

    loop {
        let mut found_child = false;
        let mut collected: Option<(u64, i32)> = None;

        {
            let table = TABLE.lock();
            for process in table.iter() {
                if process.ppid != parent_pid || process.thread {
                    continue;
                }
                if expected_pid > 0 && process.pid != expected_pid as u64 {
                    continue;
                }
                found_child = true;
                let state = process.state.load(core::sync::atomic::Ordering::Relaxed);
                if state == STATE_ZOMBIE {
                    // wait(2) durumu: çıkış kodunun alt baytı << 8.
                    let code = process
                        .exit_code
                        .load(core::sync::atomic::Ordering::Relaxed);
                    collected = Some((process.pid, (code & 0xFF) << 8));
                    break;
                }
                if state == STATE_STOPPED
                    && options & WUNTRACED != 0
                    && process.stopped_reported.load(core::sync::atomic::Ordering::Relaxed) == 0
                {
                    process
                        .stopped_reported
                        .store(1, core::sync::atomic::Ordering::Relaxed);
                    collected = Some((process.pid, (signal::SIGTSTP << 8 | 0x7f) as i32));
                    break;
                }
            }
        }

        if let Some((pid, code)) = collected {
            let zombie = TABLE
                .lock()
                .iter()
                .find(|process| process.pid == pid)
                .map(|process| {
                    process.state.load(core::sync::atomic::Ordering::Relaxed) == STATE_ZOMBIE
                })
                .unwrap_or(false);
            if zombie {
                TABLE.lock().retain(|process| process.pid != pid);
            }
            if status_pointer != 0 {
                let status = code as i32;
                let _ = crate::syscall::copy_to_user(status_pointer, &status.to_le_bytes());
            }
            return pid as i64;
        }

        if !found_child {
            return -10; // ECHILD
        }
        if options & WNOHANG != 0 {
            return 0;
        }
        if signal::interrupted() {
            return -4; // EINTR
        }

        schedule();
    }
}

/// `setpgid(pid, pgid)`: 0 değerleri "kendim" anlamına gelir.
pub fn setpgid(pid: i64, pgid: i64) -> i64 {
    let Some(current) = current() else {
        return -3;
    };
    let target_pid = if pid == 0 { current.pid } else { pid as u64 };
    let target = if target_pid == current.pid {
        current.clone()
    } else {
        match TABLE.lock().iter().find(|process| process.pid == target_pid) {
            Some(process) => process.clone(),
            None => return -3, // ESRCH
        }
    };
    let group = if pgid == 0 { target.pid } else { pgid as u64 };
    if group != target.pid {
        // Grup lideri olmayan bir gruba katılmak: grup var olmalı.
        let exists = TABLE
            .lock()
            .iter()
            .any(|process| process.pgid.load(core::sync::atomic::Ordering::Relaxed) == group);
        if !exists {
            return -3;
        }
    }
    target
        .pgid
        .store(group, core::sync::atomic::Ordering::Relaxed);
    0
}

pub fn pgid_of(pid: i64, fallback: u64) -> u64 {
    let target = if pid <= 0 {
        fallback
    } else {
        pid as u64
    };
    TABLE
        .lock()
        .iter()
        .find(|process| process.pid == target)
        .map(|process| process.pgid.load(core::sync::atomic::Ordering::Relaxed))
        .unwrap_or(fallback)
}

pub fn getpgrp() -> i64 {
    current()
        .map(|process| process.pgid.load(core::sync::atomic::Ordering::Relaxed) as i64)
        .unwrap_or(0)
}

pub fn setsid() -> i64 {
    let Some(process) = current() else {
        return -3;
    };
    process.pgid.store(process.pid, core::sync::atomic::Ordering::Relaxed);
    process.sid.store(process.pid, core::sync::atomic::Ordering::Relaxed);
    process.pid as i64
}

/// Süreci durdurur; `SIGCONT` gelene kadar zamanlayıcı onu seçmez.
pub fn stop_process(process: &Process) {
    process
        .state
        .store(STATE_STOPPED, core::sync::atomic::Ordering::Relaxed);
    signal::notify_parent(process);
    while process.state.load(core::sync::atomic::Ordering::Relaxed) == STATE_STOPPED {
        schedule();
    }
}

pub fn resume(process: &Process) {
    if process.state.load(core::sync::atomic::Ordering::Relaxed) == STATE_STOPPED {
        process
            .state
            .store(STATE_READY, core::sync::atomic::Ordering::Relaxed);
        process
            .stopped_reported
            .store(0, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Tüm süreçlerin kopyası (sinyal yayını için).
pub fn processes() -> Vec<Arc<Process>> {
    TABLE.lock().clone()
}

pub fn current() -> Option<Arc<Process>> {
    CURRENT.lock().clone()
}

pub fn set_current(process: Arc<Process>) {
    process
        .state
        .store(STATE_RUNNING, core::sync::atomic::Ordering::Relaxed);
    *CURRENT.lock() = Some(process);
}

/// `/proc` için süreç anlık görüntüsü.
pub struct ProcessSnapshot {
    pub pid: u64,
    pub ppid: u64,
    pub name: alloc::string::String,
    pub path: alloc::string::String,
    pub state: u8,
    pub user_stack: u64,
    pub exit_code: i32,
}

pub fn snapshot() -> alloc::vec::Vec<ProcessSnapshot> {
    let table = TABLE.lock();
    table
        .iter()
        .map(|process| ProcessSnapshot {
            pid: process.pid,
            ppid: process.ppid,
            name: process.name.clone(),
            path: process.exe_path.lock().clone(),
            state: process.state.load(core::sync::atomic::Ordering::Relaxed),
            user_stack: process
                .user_stack
                .load(core::sync::atomic::Ordering::Relaxed),
            exit_code: process.exit_code.load(core::sync::atomic::Ordering::Relaxed),
        })
        .collect()
}

pub fn snapshot_of(pid: u64) -> Option<ProcessSnapshot> {
    let table = TABLE.lock();
    table.iter().find(|process| process.pid == pid).map(|process| {
        ProcessSnapshot {
            pid: process.pid,
            ppid: process.ppid,
            name: process.name.clone(),
            path: process.exe_path.lock().clone(),
            state: process.state.load(core::sync::atomic::Ordering::Relaxed),
            user_stack: process
                .user_stack
                .load(core::sync::atomic::Ordering::Relaxed),
            exit_code: process.exit_code.load(core::sync::atomic::Ordering::Relaxed),
        }
    })
}

/// Süreç hâlâ çalışıyor mu? (zombie sayılmaz)
pub fn alive(pid: u64) -> bool {
    let table = TABLE.lock();
    table.iter().any(|process| {
        process.pid == pid
            && process.state.load(core::sync::atomic::Ordering::Relaxed) != STATE_ZOMBIE
    })
}

pub fn current_pid() -> u64 {
    current().map(|process| process.pid).unwrap_or(0)
}

pub fn process_count() -> usize {
    TABLE.lock().len()
}

pub fn ticks() -> u64 {
    TICKS.load(core::sync::atomic::Ordering::Relaxed)
}

/// Zaman dilimi (tik cinsinden). 100 Hz'de 50 ms.
const QUANTUM: u64 = 5;

/// PIT kesmesinden çağrılır. Zaman dilimi dolduysa sıradaki sürece geçer.
///
/// Geçiş, kesme çerçevesini yeniden yazmak yerine *çekirdek bağlamı*
/// üzerinden yapılır: engelleyen bir syscall içinde bekleyen süreçlerin
/// yığını ve dönüş adresi kendi çekirdek yığınında durur. Çerçeve yazımı
/// yalnızca kullanıcı modundan kesilen süreçler için doğru olurdu.
pub fn on_timer_tick(frame: &mut crate::arch::idt::InterruptFrame) {
    let tick = TICKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;

    // Çekirdek modundan gelen kesmede SS:RSP yok; geçiş yapma.
    if frame.cs & 3 != 3 {
        return;
    }
    if tick % QUANTUM != 0 {
        return;
    }

    let Some(current) = current() else {
        return;
    };
    let Some(next) = pick_next(current.pid) else {
        return;
    };

    // Kullanıcı bağlamı kesme çerçevesinde; bu süreç yeniden seçildiğinde
    // buradan devam edip `iretq` ile kullanıcıya dönecek.
    *current.context.lock() = Some(UserContext::from_frame(frame));
    switch_to_process(&current, &next);
}

/// Sıradaki çalıştırılabilir süreç. `current_pid` atlanır; zombiler seçilmez.
fn pick_next(current_pid: u64) -> Option<Arc<Process>> {
    let table = TABLE.lock();
    let count = table.len();
    if count == 0 {
        return None;
    }

    // Sıra tablonun başından değil, çalışan süreçten sonraki süreçten başlar;
    // aksi hâlde `wait` döngüsündeki eski süreçler yenileri aç bırakır.
    let start = table
        .iter()
        .position(|process| process.pid == current_pid)
        .map(|index| index + 1)
        .unwrap_or(0);

    let mut fallback = None;
    for offset in 0..count {
        let process = &table[(start + offset) % count];
        if process.pid == current_pid {
            continue;
        }
        let state = process.state.load(core::sync::atomic::Ordering::Relaxed);
        if state == STATE_READY {
            return Some(process.clone());
        }
        if state == STATE_RUNNING && fallback.is_none() {
            fallback = Some(process.clone());
        }
    }

    fallback
}

/// Yeni bir adres alanı ve süreç oluşturur, ELF'i yükler.
pub fn spawn(name: &str, image: &[u8], argv: &[&str], envp: &[&str]) -> Result<u64, &'static str> {
    spawn_loaded(name, image, None, argv, envp)
}

/// Yorumlayıcısı (PT_INTERP) önceden okunmuş bir ELF'i yükleyip süreç açar.
pub fn spawn_loaded(
    name: &str,
    image: &[u8],
    interpreter: Option<&[u8]>,
    argv: &[&str],
    envp: &[&str],
) -> Result<u64, &'static str> {
    let pml4 = crate::mem::vmm::new_address_space().ok_or("adres alani ayrilamadi")?;
    let loaded = crate::elf::load(pml4, image, argv, envp, interpreter).map_err(|_| "ELF yuklenemedi")?;

    let (kernel_stack, kernel_stack_top) = allocate_kernel_stack()?;

    let pid = NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let process = Arc::new(Process {
        pid,
        ppid: current_pid(),
        name: name.to_string(),
        pml4: core::sync::atomic::AtomicU64::new(pml4),
        kernel_stack_top,
        entry: core::sync::atomic::AtomicU64::new(loaded.entry),
        user_stack: core::sync::atomic::AtomicU64::new(loaded.stack_pointer),
        brk: Arc::new(core::sync::atomic::AtomicU64::new(loaded.brk_start)),
        state: core::sync::atomic::AtomicU8::new(STATE_READY),
        fd_table: Arc::new(DescriptorTable::new()),
        context: SpinLock::new(None),
        kernel_stack,
        kernel_context: SpinLock::new(Context {
            rip: user_trampoline as usize as u64,
            rsp: kernel_stack_top - 8,
            ..Context::default()
        }),
        exit_code: core::sync::atomic::AtomicI32::new(0),
        fs_base: core::sync::atomic::AtomicU64::new(0),
        signals: Arc::new(signal::SignalState::new()),
        tgid: pid,
        thread: false,
        clear_child_tid: core::sync::atomic::AtomicU64::new(0),
        pgid: core::sync::atomic::AtomicU64::new(pid),
        sid: core::sync::atomic::AtomicU64::new(pid),
        stopped_reported: core::sync::atomic::AtomicU8::new(0),
        exe_path: SpinLock::new(String::from("/bin/init")),
    });

    TABLE.lock().push(process.clone());
    println!(
        "task: {} (pid {}) hazir, entry {:#x}, stack {:#x}",
        name, pid, loaded.entry, loaded.stack_pointer
    );

    Ok(pid)
}

/// Dosya sisteminden ELF okuyup yeni süreç olarak yükler.
pub fn spawn_path(name: &str, path: &str, argv: &[&str], envp: &[&str]) -> Result<u64, &'static str> {
    let (image, interpreter) = read_program(path)?;
    spawn_loaded(name, &image, interpreter.as_deref(), argv, envp)
}

/// Dosyayı okur; dinamik ikiliyse `PT_INTERP` yorumlayıcısını da yükler.
fn read_program(path: &str) -> Result<(alloc::vec::Vec<u8>, Option<alloc::vec::Vec<u8>>), &'static str> {
    let inode = crate::fs::lookup_path(path).map_err(|_| "dosya yok")?;
    let size = inode.metadata().size as usize;
    if size == 0 || size > 128 * 1024 * 1024 {
        return Err("gecersiz boyut");
    }

    let mut image = alloc::vec![0u8; size];
    inode.read_at(0, &mut image).map_err(|_| "okunamadi")?;

    let interpreter = match crate::elf::interpreter_path(&image) {
        Some(path_bytes) => {
            let interpreter_path =
                core::str::from_utf8(&path_bytes).map_err(|_| "gecersiz PT_INTERP")?;
            let inode = crate::fs::lookup_path(interpreter_path).map_err(|_| "yorumlayici yok")?;
            let size = inode.metadata().size as usize;
            if size == 0 || size > 128 * 1024 * 1024 {
                return Err("yorumlayici boyutu");
            }
            let mut bytes = alloc::vec![0u8; size];
            inode
                .read_at(0, &mut bytes)
                .map_err(|_| "yorumlayici okunamadi")?;
            Some(bytes)
        }
        None => None,
    };

    Ok((image, interpreter))
}

/// Çalışan sürecin imajını `path`'teki ELF ile değiştirir. Başarılıysa geri
/// dönmez. Eski adres alanı serbest bırakılmıyor (TODO: M3 sonu).
pub fn exec_path(path: &str, argv: &[&str], envp: &[&str]) -> Result<(), &'static str> {
    // `#!` yorumlayıcısı: betik dosyaları doğrudan çalıştırılabilir.
    let mut header = [0u8; 128];
    let header_length = match crate::fs::lookup_path(path) {
        Ok(inode) => inode.read_at(0, &mut header).unwrap_or(0),
        Err(_) => 0,
    };
    if header_length >= 3 && &header[..2] == b"#!" {
        let line_end = header[2..header_length]
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(header_length - 2);
        let line = core::str::from_utf8(&header[2..2 + line_end]).map_err(|_| "shebang")?;
        let mut parts = line.split_whitespace();
        let interpreter = parts.next().ok_or("bos shebang")?;
        let mut new_argv: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
        new_argv.push(interpreter);
        if let Some(argument) = parts.next() {
            new_argv.push(argument);
        }
        new_argv.push(path);
        for argument in argv.iter().skip(1) {
            new_argv.push(argument);
        }
        return exec_path(interpreter, &new_argv, envp);
    }

    let (image, interpreter) = read_program(path)?;

    let process = current().ok_or("surec yok")?;
    let pml4 = crate::mem::vmm::new_address_space().ok_or("adres alani yok")?;
    let loaded = crate::elf::load(pml4, &image, argv, envp, interpreter.as_deref())
        .map_err(|_| "ELF yuklenemedi")?;

    process
        .pml4
        .store(pml4, core::sync::atomic::Ordering::Relaxed);
    process
        .entry
        .store(loaded.entry, core::sync::atomic::Ordering::Relaxed);
    process
        .user_stack
        .store(loaded.stack_pointer, core::sync::atomic::Ordering::Relaxed);
    process
        .brk
        .store(loaded.brk_start, core::sync::atomic::Ordering::Relaxed);
    process.context.lock().take();
    *process.exe_path.lock() = String::from(path);
    process
        .fs_base
        .store(0, core::sync::atomic::Ordering::Relaxed);

    println!("exec: {} -> entry {:#x}", path, loaded.entry);
    unsafe { enter_user(pml4, loaded.entry, loaded.stack_pointer) }
}

/// Ring3'e geçer. Dönüş yok: süreç `exit` syscall'ı ile sonlanmalı.
///
/// # Safety
/// `pml4` bu sürece ait olmalı ve `entry`/`stack` eşlenmiş olmalı.
pub unsafe fn enter_user(pml4: u64, entry: u64, stack: u64) -> ! {
    // System V ABI: yeni program baslarken %rdx = rtld_fini (statik ikili icin 0),
    // diger GPR'ler sifir. glibc bu degeri atexit isleyicisi olarak kaydeder.
    let context = UserContext {
        rip: entry,
        rsp: stack,
        rflags: 0x202,
        ..UserContext::default()
    };
    enter_user_context(pml4, &context)
}

/// Sinyalin varsayılan eylemi için: süreci zombi yapıp başkasına geçer.
pub fn exit_process(process: &Process, code: i32) -> ! {
    println!("task: pid {} sinyalle sonlandi (kod {})", process.pid, code);
    signal::thread_exit(process);
    signal::notify_parent(process);
    process
        .exit_code
        .store(code, core::sync::atomic::Ordering::Relaxed);
    process
        .state
        .store(STATE_ZOMBIE, core::sync::atomic::Ordering::Relaxed);
    schedule();
    *CURRENT.lock() = None;
    idle_loop()
}

pub fn exit(code: i32) -> ! {
    let finished = current();

    if let Some(process) = &finished {
        println!("task: pid {} cikti (kod {})", process.pid, code);
        signal::thread_exit(process);
        signal::notify_parent(process);
        process.exit_code.store(code, core::sync::atomic::Ordering::Relaxed);
        process
            .state
            .store(STATE_ZOMBIE, core::sync::atomic::Ordering::Relaxed);
    }

    schedule();

    // Sırada başka süreç yok: çekirdek boşta bekler.
    *CURRENT.lock() = None;
    idle_loop()
}

pub fn idle_loop() -> ! {
    idt::enable();
    loop {
        cpu::halt();
    }
}

/// Boşta (çekirdek bağlamı) çalışırken hazır bir süreç varsa ona geçer.
/// Süreç bitince `exit` yeniden `idle_loop`a döner; bu çağrı geri dönmez.
pub fn start_ready_from_idle() -> bool {
    let Some(next) = pick_next(0) else {
        return false;
    };

    next.state
        .store(STATE_RUNNING, core::sync::atomic::Ordering::Relaxed);
    *CURRENT.lock() = Some(next.clone());

    let pml4 = next.pml4.load(core::sync::atomic::Ordering::Relaxed);
    crate::mem::vmm::switch_to(pml4);
    gdt::set_kernel_stack(next.kernel_stack_top);
    crate::syscall::set_kernel_stack(next.kernel_stack_top);

    let mut bootstrap = Context::default();
    unsafe {
        archon_switch_context(&mut bootstrap, context_pointer(&next));
    }
    unreachable!("bos surec baglami dondu")
}

/// Hazır süreçler arasında ilkini seçer (round-robin M3'te).
pub fn next_ready() -> Option<Arc<Process>> {
    let table = TABLE.lock();
    table
        .iter()
        .find(|process| {
            process.state.load(core::sync::atomic::Ordering::Relaxed) == STATE_READY
        })
        .cloned()
}
