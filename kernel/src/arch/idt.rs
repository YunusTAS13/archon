//! IDT ve kesme girişi.
//!
//! 256 vektörün tamamı derleme zamanında üretilir: her vektör için küçük bir
//! stub, ortak girişe atlar. Hata kodu iten vektörler (8, 10–14, 17, 21, 29,
//! 30) CPU'nun ittiği kodu olduğu gibi bırakır, diğerleri sahte 0 iter. Böylece
//! Rust tarafında tek bir `InterruptFrame` yerleşimi yeter.

use core::arch::global_asm;

use super::cpu;
use super::gdt;
use crate::sync::SpinLock;

const IDT_ENTRIES: usize = 256;

pub const VECTOR_DIVIDE_ERROR: u64 = 0;
pub const VECTOR_DEBUG: u64 = 1;
pub const VECTOR_BREAKPOINT: u64 = 3;
pub const VECTOR_INVALID_OPCODE: u64 = 6;
pub const VECTOR_DOUBLE_FAULT: u64 = 8;
pub const VECTOR_GENERAL_PROTECTION: u64 = 13;
pub const VECTOR_PAGE_FAULT: u64 = 14;
pub const VECTOR_IRQ_BASE: u64 = 32;

#[repr(C)]
pub struct InterruptFrame {
    // Yığın sırası: en son `rax` itildiği için en düşük adreste o dur.
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            flags: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn new(handler: u64, selector: u16, ist: u8, flags: u8) -> Self {
        Self {
            offset_low: handler as u16,
            selector,
            ist,
            flags,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

const FLAG_PRESENT: u8 = 0x80;
const FLAG_INTERRUPT_GATE: u8 = 0x0E;

static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry::missing(); IDT_ENTRIES];
static INTERRUPTS_ENABLED: SpinLock<bool> = SpinLock::new(false);

global_asm!(
    r#"
    .att_syntax prefix
    .altmacro
    .macro ISR_NOERR n
isr_\n:
    pushq $0
    pushq $\n
    jmp isr_common
    .endm

    .macro ISR_ERR n
isr_\n:
    pushq $\n
    jmp isr_common
    .endm

    .macro ISR_TABLE_ENTRY n
    .quad isr_\n
    .endm

isr_common:
    pushq %r15
    pushq %r14
    pushq %r13
    pushq %r12
    pushq %r11
    pushq %r10
    pushq %r9
    pushq %r8
    pushq %rbp
    pushq %rdi
    pushq %rsi
    pushq %rdx
    pushq %rcx
    pushq %rbx
    pushq %rax

    movq %rsp, %rbx
    andq $-16, %rsp
    movq %rbx, %rdi
    call isr_dispatch
    movq %rbx, %rsp

    popq %rax
    popq %rbx
    popq %rcx
    popq %rdx
    popq %rsi
    popq %rdi
    popq %rbp
    popq %r8
    popq %r9
    popq %r10
    popq %r11
    popq %r12
    popq %r13
    popq %r14
    popq %r15
    addq $16, %rsp
    iretq

    .set vec, 0
    .rept 256
        .set is_err, 0
        .if vec == 8
            .set is_err, 1
        .endif
        .if vec == 10
            .set is_err, 1
        .endif
        .if vec == 11
            .set is_err, 1
        .endif
        .if vec == 12
            .set is_err, 1
        .endif
        .if vec == 13
            .set is_err, 1
        .endif
        .if vec == 14
            .set is_err, 1
        .endif
        .if vec == 17
            .set is_err, 1
        .endif
        .if vec == 21
            .set is_err, 1
        .endif
        .if vec == 29
            .set is_err, 1
        .endif
        .if vec == 30
            .set is_err, 1
        .endif
        .if is_err
            ISR_ERR %vec
        .else
            ISR_NOERR %vec
        .endif
        .set vec, vec + 1
    .endr

    .section .rodata
    .align 8
    .global isr_stub_table
isr_stub_table:
    .set vec, 0
    .rept 256
        ISR_TABLE_ENTRY %vec
        .set vec, vec + 1
    .endr
    .text
"#
);

extern "C" {
    static isr_stub_table: [u64; IDT_ENTRIES];
}

pub fn init() {
    let table = unsafe { &isr_stub_table };
    let idt = unsafe { &mut *core::ptr::addr_of_mut!(IDT) };

    for (vector, handler) in table.iter().enumerate() {
        let ist = if vector == VECTOR_DOUBLE_FAULT as usize { 1 } else { 0 };
        idt[vector] = IdtEntry::new(
            *handler,
            gdt::KERNEL_CODE,
            ist,
            FLAG_PRESENT | FLAG_INTERRUPT_GATE,
        );
    }

    let pointer = IdtPointer {
        limit: (core::mem::size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
        base: idt.as_ptr() as u64,
    };

    unsafe {
        core::arch::asm!(
            "lidt [{}]",
            in(reg) &pointer,
            options(readonly, nostack, preserves_flags)
        );
    }
}

pub fn enable() {
    let mut enabled = INTERRUPTS_ENABLED.lock();
    if !*enabled {
        unsafe { cpu::enable_interrupts() };
        *enabled = true;
    }
}

pub fn disable() {
    let mut enabled = INTERRUPTS_ENABLED.lock();
    unsafe { cpu::disable_interrupts() };
    *enabled = false;
}

pub fn interrupts_enabled() -> bool {
    cpu::interrupts_enabled()
}

/// EOI'siz bir kesme dönerse PIC kilitlenir; bu yüzden IRQ'lar burada
/// dağıtılıp PIC'e EOI gönderilir.
#[no_mangle]
pub extern "C" fn isr_dispatch(frame: *mut InterruptFrame) {
    let frame = unsafe { &mut *frame };

    if frame.vector < VECTOR_IRQ_BASE {
        handle_exception(frame);
    } else if frame.vector < VECTOR_IRQ_BASE + 16 {
        let irq = (frame.vector - VECTOR_IRQ_BASE) as u8;
        match irq {
            0 => super::pit::on_tick(),
            1 => crate::drivers::ps2::on_keyboard_irq(),
            4 => {
                crate::arch::serial::poll_line(&mut []);
            }
            _ => {}
        }
        super::pic::end_of_interrupt(irq);
        crate::task::on_timer_tick(frame);
    }
}

fn handle_exception(frame: &mut InterruptFrame) {
    match frame.vector {
        VECTOR_BREAKPOINT => {
            println!(
                "kesme: #BP rip={:#x} (devam)",
                frame.rip
            );
            return;
        }
        VECTOR_PAGE_FAULT => {
            let address = cpu::read_cr2();
            let present = frame.error & 1 != 0;
            let write = frame.error & 2 != 0;
            let user = frame.error & 4 != 0;
            crate::syscall::dump_history();
            // Hata veren RIP'in bulunduğu modülün tabanını bul (ELF başlığı).
            // Kullanıcı yığınındaki dönüş adresleri (çağıranı bulmak için).
            for index in 0..4u64 {
                if let Some(physical) =
                    crate::mem::vmm::translate(crate::mem::vmm::current_pml4(), frame.rsp + index * 8)
                {
                    let value = unsafe {
                        core::ptr::read_volatile(crate::mem::hhdm::phys_to_ptr::<u64>(physical))
                    };
                    crate::println!("yigin[{}] = {:#x}", index, value);
                }
            }
            let bases = crate::mem::vmm::module_bases();
            for base in &bases {
                if *base <= frame.rip {
                    crate::println!(
                        "modul {:#x} ofset {:#x}",
                        base,
                        frame.rip - base
                    );
                }
            }
            let pte = crate::mem::vmm::leaf_entry(crate::mem::vmm::current_pml4(), address);
            crate::println!("pte={:#x?} cr3={:#x}", pte, crate::mem::vmm::current_pml4());
            panic!(
                "page fault: adres={:#x} rip={:#x} cs={:#x} rsp={:#x} hata={:#x} present={} write={} user={}",
                address, frame.rip, frame.cs, frame.rsp, frame.error, present, write, user
            );
        }
        _ => {}
    }

    panic!(
        "istisna {}: hata={:#x} rip={:#x} cs={:#x} rflags={:#x} rsp={:#x}\n\
         rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x}\n\
         rdi={:#x} rsi={:#x} rbp={:#x} r8={:#x} r9={:#x} r10={:#x} r11={:#x} \
         r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
        frame.vector, frame.error, frame.rip, frame.cs, frame.rflags, frame.rsp,
        frame.rax, frame.rbx, frame.rcx, frame.rdx,
        frame.rdi, frame.rsi, frame.rbp, frame.r8, frame.r9, frame.r10, frame.r11,
        frame.r12, frame.r13, frame.r14, frame.r15
    );
}

pub fn set_handler(vector: u64, handler: u64) {
    let idt = unsafe { &mut *core::ptr::addr_of_mut!(IDT) };
    idt[vector as usize] = IdtEntry::new(
        handler,
        gdt::KERNEL_CODE,
        0,
        FLAG_PRESENT | FLAG_INTERRUPT_GATE,
    );
}
