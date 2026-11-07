//! Boot bilgilerinin normalize edilmiş hali.
//!
//! Çekirdeğin geri kalanı Limine tiplerini görmemeli; burada tek bir yapıya
//! indirgenir. İleride başka bir bootloader veya UEFI doğrudan boot eklense
//! yalnızca bu modül değişir.

pub mod limine;
pub mod limine_ids;

use limine::MemmapEntry;

#[derive(Clone, Copy)]
pub struct BootInfo {
    pub hhdm_offset: u64,
    pub kernel_virtual: u64,
    pub kernel_physical: u64,
    pub rsdp: u64,
    pub memmap: &'static [MemmapEntry],
    pub memmap_len: usize,
    pub framebuffer_address: u64,
    pub framebuffer_width: u64,
    pub framebuffer_height: u64,
    pub framebuffer_pitch: u64,
}

// Erken boot'ta heap yok; memmap'i BSS'te sabit bir tampona kopyalıyoruz.
// 256 giriş tüm gerçekçi makineler için fazlasıyla yeterli.
const MAX_MEMMAP_ENTRIES: usize = 256;

static mut MEMMAP_STORAGE: [MemmapEntry; MAX_MEMMAP_ENTRIES] = [MemmapEntry {
    base: 0,
    length: 0,
    kind: 0,
}; MAX_MEMMAP_ENTRIES];

static mut BOOT_INFO: Option<BootInfo> = None;

pub fn init() {
    let mut storage = [MemmapEntry {
        base: 0,
        length: 0,
        kind: 0,
    }; MAX_MEMMAP_ENTRIES];
    let count = limine::copy_memmap(&mut storage);

    unsafe {
        MEMMAP_STORAGE[..count].copy_from_slice(&storage[..count]);
    }

    let hhdm_offset = limine::hhdm_offset().expect("Limine HHDM yanıtı yok");
    let (kernel_virtual, kernel_physical) =
        limine::kernel_address().expect("Limine kernel adres yanıtı yok");
    let rsdp = limine::rsdp_address().unwrap_or(0);
    let framebuffer = limine::framebuffer();

    let (framebuffer_address, framebuffer_width, framebuffer_height, framebuffer_pitch) =
        match framebuffer {
            Some(fb) => (fb.address as u64, fb.width, fb.height, fb.pitch),
            None => (0, 0, 0, 0),
        };

    let info = BootInfo {
        hhdm_offset,
        kernel_virtual,
        kernel_physical,
        rsdp,
        memmap: unsafe { &MEMMAP_STORAGE[..count] },
        memmap_len: count,
        framebuffer_address,
        framebuffer_width,
        framebuffer_height,
        framebuffer_pitch,
    };

    unsafe {
        BOOT_INFO = Some(info);
    }
}

pub fn info() -> BootInfo {
    unsafe { BOOT_INFO.expect("BootInfo henüz hazır değil") }
}

pub fn usable_ram_bytes() -> u64 {
    let info = info();
    info.memmap
        .iter()
        .filter(|entry| entry.kind == limine::MEMMAP_USABLE)
        .map(|entry| entry.length)
        .sum()
}
