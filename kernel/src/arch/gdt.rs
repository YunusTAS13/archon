//! GDT + TSS.
//!
//! Seçici yerleşimi SYSCALL/SYSRET ile uyumlu olmak zorunda:
//!
//! ```text
//! 0x00 null
//! 0x08 kernel code   (STAR[47:32])
//! 0x10 kernel data
//! 0x18 user data     (SYSRET SS = STAR[63:48] + 8  = 0x18)
//! 0x20 user code     (SYSRET CS = STAR[63:48] + 16 = 0x20)
//! 0x28 TSS alt yarı
//! 0x30 TSS üst yarı
//! ```
//!
//! `STAR[63:48] = 0x10` seçildiğinde SYSRET sırasıyla 0x1B ve 0x23 üretir.

use core::arch::global_asm;
use core::mem::size_of;

use super::cpu::{self, MSR_STAR, MSR_SFMASK};

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const USER_DATA: u16 = 0x18;
pub const USER_CODE: u16 = 0x20;
pub const TSS_SELECTOR: u16 = 0x28;

pub const USER_DATA_RPL3: u16 = USER_DATA | 3;
pub const USER_CODE_RPL3: u16 = USER_CODE | 3;

#[repr(C, packed)]
pub struct TaskStateSegment {
    _reserved0: u32,
    pub rsp0: u64,
    pub rsp1: u64,
    pub rsp2: u64,
    _reserved1: u64,
    pub ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    pub iomap_base: u16,
}

impl TaskStateSegment {
    const fn new() -> Self {
        Self {
            _reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            _reserved1: 0,
            ist: [0; 7],
            _reserved2: 0,
            _reserved3: 0,
            // IO bitmap yok: ring3 `in`/`out` her zaman #GP alır.
            iomap_base: size_of::<Self>() as u16,
        }
    }
}

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

static mut TSS: TaskStateSegment = TaskStateSegment::new();
static mut GDT: [u64; 7] = [0; 7];
static mut GDT_POINTER: DescriptorPointer = DescriptorPointer { limit: 0, base: 0 };

global_asm!(
    r#"
    .att_syntax prefix
    .global archon_gdt_flush
archon_gdt_flush:
    lgdt (%rdi)
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %fs
    movw %ax, %gs
    movw %ax, %ss
    pushq $0x08
    leaq 1f(%rip), %rax
    pushq %rax
    lretq
1:
    ret
"#
);

extern "C" {
    fn archon_gdt_flush(pointer: *const DescriptorPointer);
}

/// Klasik 64-bit segment tanımlayıcısı. `flags` düşük 4 bit: G, D/B, L, AVL.
const fn segment(base: u32, limit: u32, access: u8, flags: u8) -> u64 {
    let limit_low = (limit & 0xFFFF) as u64;
    let base_low = (base & 0xFFFF) as u64;
    let base_mid = ((base >> 16) & 0xFF) as u64;
    let base_high = ((base >> 24) & 0xFF) as u64;

    limit_low
        | (base_low << 16)
        | (base_mid << 32)
        | ((access as u64) << 40)
        | ((((limit >> 16) & 0x0F) as u64) << 48)
        | (((flags & 0x0F) as u64) << 52)
        | (base_high << 56)
}

const ACCESS_KERNEL_CODE: u8 = 0x9A; // P=1, DPL=0, S=1, exec/read
const ACCESS_KERNEL_DATA: u8 = 0x92; // P=1, DPL=0, S=1, read/write
const ACCESS_USER_DATA: u8 = 0xF2; // P=1, DPL=3, S=1, read/write
const ACCESS_USER_CODE: u8 = 0xFA; // P=1, DPL=3, S=1, exec/read
const ACCESS_TSS: u8 = 0x89; // P=1, tip=9 (64-bit available TSS)

const FLAGS_LONG_MODE: u8 = 0x0A; // G=1, L=1

pub fn init() {
    unsafe {
        let gdt = &mut *core::ptr::addr_of_mut!(GDT);
        gdt[0] = 0;
        gdt[1] = segment(0, 0xFFFFF, ACCESS_KERNEL_CODE, FLAGS_LONG_MODE);
        gdt[2] = segment(0, 0xFFFFF, ACCESS_KERNEL_DATA, FLAGS_LONG_MODE);
        gdt[3] = segment(0, 0xFFFFF, ACCESS_USER_DATA, FLAGS_LONG_MODE);
        gdt[4] = segment(0, 0xFFFFF, ACCESS_USER_CODE, FLAGS_LONG_MODE);

        let tss_base = core::ptr::addr_of!(TSS) as u64;
        let tss_limit = (size_of::<TaskStateSegment>() - 1) as u32;
        gdt[5] = segment(tss_base as u32, tss_limit, ACCESS_TSS, 0x00);
        gdt[6] = tss_base >> 32;

        let pointer = &mut *core::ptr::addr_of_mut!(GDT_POINTER);
        pointer.limit = (size_of::<[u64; 7]>() - 1) as u16;
        pointer.base = gdt.as_ptr() as u64;

        archon_gdt_flush(pointer as *const DescriptorPointer);

        // TSS'i yükle.
        core::arch::asm!(
            "ltr ax",
            in("ax") TSS_SELECTOR,
            options(nostack, preserves_flags)
        );

        configure_syscall_msrs(core::ptr::addr_of!(TSS) as u64);
    }
}

/// Kernel yığınının tepesi. `SYSCALL` geldiğinde RSP buradan yüklenir.
pub fn set_kernel_stack(top: u64) {
    unsafe {
        let tss = &mut *core::ptr::addr_of_mut!(TSS);
        tss.rsp0 = top;
    }
}

pub fn kernel_stack() -> u64 {
    unsafe {
        let tss = &*core::ptr::addr_of!(TSS);
        tss.rsp0
    }
}

/// Kesme sırasında kullanılacak IST yığını (double fault için IST1).
pub fn set_ist(index: usize, top: u64) {
    debug_assert!(index < 7);
    unsafe {
        let tss = &mut *core::ptr::addr_of_mut!(TSS);
        tss.ist[index] = top;
    }
}

/// STAR / LSTAR / SFMASK. `lstar` çekirdeğin syscall giriş noktası; TSS
/// adresi de `SYSCALL` sırasında RSP'yi bulmak için saklanır.
fn configure_syscall_msrs(_tss_base: u64) {
    unsafe {
        // STAR: [47:32] kernel CS, [63:48] SYSRET tabanı (user data - 8).
        let star = ((KERNEL_DATA as u64) << 48) | ((KERNEL_CODE as u64) << 32);
        cpu::write_msr(MSR_STAR, star);

        // SYSCALL'da temizlenmesi gereken bayraklar: IF, DF, TF, AC.
        let flags = (1 << 9) | (1 << 10) | (1 << 8) | (1 << 18);
        cpu::write_msr(MSR_SFMASK, flags);
    }
}

pub fn selectors() -> (u16, u16) {
    (USER_CODE_RPL3, USER_DATA_RPL3)
}
