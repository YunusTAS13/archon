//! CPU kontrol register'ları ve talimatlar.

use core::arch::{asm, x86_64};

#[inline]
pub fn read_cr2() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack)) };
    value
}

#[inline]
pub fn read_cr3() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack)) };
    value
}

#[inline]
pub unsafe fn write_cr3(value: u64) {
    asm!("mov cr3, {}", in(reg) value, options(nostack));
}

#[inline]
pub fn read_rflags() -> u64 {
    let value: u64;
    unsafe { asm!("pushfq", "pop {}", out(reg) value, options(nomem, nostack)) };
    value
}

#[inline]
pub unsafe fn write_rflags(value: u64) {
    asm!("push {}", "popfq", in(reg) value, options(nomem, nostack));
}

#[inline]
pub fn interrupts_enabled() -> bool {
    read_rflags() & (1 << 9) != 0
}

#[inline]
pub unsafe fn disable_interrupts() {
    asm!("cli", options(nomem, nostack));
}

#[inline]
pub unsafe fn enable_interrupts() {
    asm!("sti", options(nomem, nostack));
}

#[inline]
pub fn halt() {
    unsafe { asm!("hlt", options(nomem, nostack)) };
}

#[inline]
pub fn idle() {
    unsafe { asm!("sti", "hlt", options(nomem, nostack)) };
}

#[inline]
pub unsafe fn invlpg(address: u64) {
    asm!("invlpg [{}]", in(reg) address, options(nostack));
}

#[inline]
pub fn rdtsc() -> u64 {
    unsafe { x86_64::_rdtsc() }
}

#[inline]
pub fn cpuid(leaf: u32, subleaf: u32) -> x86_64::CpuidResult {
    unsafe { x86_64::__cpuid_count(leaf, subleaf) }
}

pub fn vendor() -> [u8; 12] {
    let result = cpuid(0, 0);
    let mut bytes = [0u8; 12];
    bytes[0..4].copy_from_slice(&result.ebx.to_le_bytes());
    bytes[4..8].copy_from_slice(&result.edx.to_le_bytes());
    bytes[8..12].copy_from_slice(&result.ecx.to_le_bytes());
    bytes
}

pub fn has_apic() -> bool {
    cpuid(1, 0).edx & (1 << 9) != 0
}

pub const MSR_EFER: u32 = 0xC000_0080;
pub const MSR_STAR: u32 = 0xC000_0081;
pub const MSR_LSTAR: u32 = 0xC000_0082;
pub const MSR_SFMASK: u32 = 0xC000_0084;
pub const MSR_GS_BASE: u32 = 0xC000_0101;
pub const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;
pub const MSR_APIC_BASE: u32 = 0x1B;
pub const MSR_PAT: u32 = 0x277;

pub const EFER_SCE: u64 = 1 << 0;
pub const EFER_LME: u64 = 1 << 8;
pub const EFER_NXE: u64 = 1 << 11;

#[inline]
pub unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack));
    ((high as u64) << 32) | low as u64
}

#[inline]
pub unsafe fn write_msr(msr: u32, value: u64) {
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") value as u32,
        in("edx") (value >> 32) as u32,
        options(nomem, nostack)
    );
}

#[inline]
pub unsafe fn enable_syscall() {
    let efer = read_msr(MSR_EFER);
    write_msr(MSR_EFER, efer | EFER_SCE);
}

#[inline]
fn read_cr0() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr0", out(reg) value, options(nomem, nostack)) };
    value
}

#[inline]
fn read_cr4() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr4", out(reg) value, options(nomem, nostack)) };
    value
}

/// SSE'yi kullanıcı ve çekirdek için açar.
///
/// `CR0.OSFXSR`/`OSXMMEXCPT` olmadan `movaps` gibi talimatlar `#UD` üretir;
/// glibc'nin başlangıç kodu SSE kullandığı için bu, dinamik olmayan ikililerde
/// bile çekirdek paniğine yol açıyordu.
pub fn enable_sse() {
    unsafe {
        let mut cr0 = read_cr0();
        cr0 &= !(1 << 2); // EM: x87 emülasyonu kapalı
        cr0 &= !(1 << 3); // TS: görev değişti bayrağı temiz
        cr0 |= 1 << 1; // MP: beklemede x87
        cr0 |= 1 << 5; // NE: x87 hataları #MF üretir
        asm!("mov cr0, {}", in(reg) cr0, options(nostack));

        let mut cr4 = read_cr4();
        cr4 |= 1 << 9; // OSFXSR
        cr4 |= 1 << 10; // OSXMMEXCPT
        asm!("mov cr4, {}", in(reg) cr4, options(nostack));
    }
}

/// XSAVE/AVX durumunu açar.
///
/// glibc `CPUID` ile AVX gördüğünde `vpxor` gibi talimatlar kullanıyor;
/// `CR4.OSXSAVE` ve `XCR0` ayarlanmazsa bu talimatlar `#UD` üretir.
/// XCR0'a yalnızca x87/SSE/AVX yazılır (0x7); AVX-512 durumu kapalı kalır ki
/// glibc o yolu seçmesin.
pub fn enable_xsave() {
    let leaf1 = core::arch::x86_64::__cpuid(1);
    // CPUID.1:ECX.26 yalnızca XSAVE komutunun varlığını söyler; OSXSAVE (27)
    // biz CR4'ü kurduktan sonra 1 olur, bu yüzden ona bakılmaz.
    let xsave = leaf1.ecx & (1 << 26) != 0;
    crate::println!(
        "cpu: ecx={:#010x} xsave={} avx={}",
        leaf1.ecx,
        xsave,
        leaf1.ecx & (1 << 28) != 0
    );
    if !xsave {
        return;
    }

    unsafe {
        let cr4 = read_cr4() | (1 << 18); // OSXSAVE
        asm!("mov cr4, {}", in(reg) cr4, options(nostack));

        // Desteklenen XCR0 bitlerinin ilk üçü: x87, SSE, AVX.
        let leaf_d = core::arch::x86_64::__cpuid_count(0xD, 0);
        let xcr0 = leaf_d.eax & 0x7;
        crate::println!("cpu: XCR0 <- {:#x} (destek {:#x})", xcr0, leaf_d.eax);
        asm!("xsetbv", in("ecx") 0u32, in("eax") xcr0, in("edx") 0u32, options(nostack));
    }
}

/// CPU üretici marka dizesi (`CPUID` 0x8000_0002..4).
pub fn brand_string() -> alloc::string::String {
    use core::arch::x86_64::__cpuid;

    let extended = __cpuid(0x8000_0000).eax;
    if extended < 0x8000_0004 {
        return alloc::string::String::from("x86_64 CPU");
    }

    let mut bytes = [0u8; 48];
    for (index, leaf) in [0x8000_0002u32, 0x8000_0003, 0x8000_0004]
        .iter()
        .enumerate()
    {
        let values = __cpuid(*leaf);
        let base = index * 16;
        bytes[base..base + 4].copy_from_slice(&values.eax.to_le_bytes());
        bytes[base + 4..base + 8].copy_from_slice(&values.ebx.to_le_bytes());
        bytes[base + 8..base + 12].copy_from_slice(&values.ecx.to_le_bytes());
        bytes[base + 12..base + 16].copy_from_slice(&values.edx.to_le_bytes());
    }

    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    let text = core::str::from_utf8(&bytes[..end]).unwrap_or("x86_64 CPU");
    alloc::string::String::from(text.trim())
}
