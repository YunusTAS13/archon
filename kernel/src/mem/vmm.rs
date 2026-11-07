//! 4 seviyeli sayfa tablosu yardımcıları.
//!
//! Kernel, Limine'nin kurduğu tabloları kullanır. Bu modül üç şeyi bilir:
//! var olan bir eşlemeyi yürümek, yeni eşleme eklemek ve user süreçler için
//! çekirdek üst yarısını paylaşan yeni bir PML4 kurmak.

use super::{hhdm, pmm, pmm::FRAME_SIZE};
use crate::arch::cpu;

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const WRITE_THROUGH: u64 = 1 << 3;
pub const CACHE_DISABLE: u64 = 1 << 4;
pub const ACCESSED: u64 = 1 << 5;
pub const DIRTY: u64 = 1 << 6;
pub const HUGE_PAGE: u64 = 1 << 7;
pub const GLOBAL: u64 = 1 << 8;
pub const NO_EXECUTE: u64 = 1 << 63;

pub const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// Üst yarı çekirdeğe ait. PML4 girdileri 256..512 arası paylaşılır.
pub const KERNEL_HALF_START_INDEX: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    FrameAllocation,
    AlreadyMapped,
    NotAligned,
}

#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [u64; 512],
}

fn table(physical: u64) -> &'static mut PageTable {
    unsafe { &mut *hhdm::phys_to_ptr::<PageTable>(physical) }
}

fn allocate_table() -> Option<u64> {
    let frame = pmm::alloc_frame()?;
    unsafe { hhdm::zero_physical(frame, FRAME_SIZE as usize) };
    Some(frame)
}

pub fn current_pml4() -> u64 {
    cpu::read_cr3() & ADDRESS_MASK
}

fn indexes(virtual_address: u64) -> [usize; 4] {
    [
        ((virtual_address >> 39) & 0x1FF) as usize,
        ((virtual_address >> 30) & 0x1FF) as usize,
        ((virtual_address >> 21) & 0x1FF) as usize,
        ((virtual_address >> 12) & 0x1FF) as usize,
    ]
}

pub fn translate(pml4_physical: u64, virtual_address: u64) -> Option<u64> {
    let [i3, i2, i1, i0] = indexes(virtual_address);

    let entry = table(pml4_physical).entries[i3];
    if entry & PRESENT == 0 {
        return None;
    }

    let mut current = table(entry & ADDRESS_MASK).entries[i2];
    if current & PRESENT == 0 {
        return None;
    }
    if current & HUGE_PAGE != 0 {
        // 1 GiB sayfa: alt 30 bit ofset.
        return Some((current & 0x000F_FFFF_C000_0000) + (virtual_address & 0x3FFF_FFFF));
    }

    current = table(current & ADDRESS_MASK).entries[i1];
    if current & PRESENT == 0 {
        return None;
    }
    if current & HUGE_PAGE != 0 {
        // 2 MiB sayfa: alt 21 bit ofset.
        return Some((current & 0x000F_FFFF_FFE0_0000) + (virtual_address & 0x1F_FFFF));
    }

    current = table(current & ADDRESS_MASK).entries[i0];
    if current & PRESENT == 0 {
        return None;
    }
    Some((current & ADDRESS_MASK) + (virtual_address & 0xFFF))
}

/// Mevcut adres alanına 4 KiB eşleme ekler.
pub fn map(virtual_address: u64, physical_address: u64, flags: u64) -> Result<(), MapError> {
    map_in(current_pml4(), virtual_address, physical_address, flags)
}

pub fn map_in(
    pml4_physical: u64,
    virtual_address: u64,
    physical_address: u64,
    flags: u64,
) -> Result<(), MapError> {
    if virtual_address % FRAME_SIZE != 0 || physical_address % FRAME_SIZE != 0 {
        return Err(MapError::NotAligned);
    }

    let [i3, i2, i1, i0] = indexes(virtual_address);
    let mut table_physical = pml4_physical;

    for index in [i3, i2, i1] {
        let entry = &mut table(table_physical).entries[index];
        if *entry & PRESENT == 0 {
            let fresh = allocate_table().ok_or(MapError::FrameAllocation)?;
            *entry = fresh | PRESENT | WRITABLE | USER;
        } else if *entry & HUGE_PAGE != 0 {
            // Üst seviyede büyük sayfa varsa altına inemeyiz.
            return Err(MapError::AlreadyMapped);
        }
        table_physical = *entry & ADDRESS_MASK;
    }

    let entry = &mut table(table_physical).entries[i0];
    if *entry & PRESENT != 0 {
        return Err(MapError::AlreadyMapped);
    }
    *entry = (physical_address & ADDRESS_MASK) | flags | PRESENT;
    if pml4_physical == current_pml4() {
        unsafe { cpu::invlpg(virtual_address) };
    }
    Ok(())
}

pub fn unmap(virtual_address: u64) -> Option<u64> {
    unmap_in(current_pml4(), virtual_address)
}

pub fn unmap_in(pml4_physical: u64, virtual_address: u64) -> Option<u64> {
    let [i3, i2, i1, i0] = indexes(virtual_address);

    let entry = table(pml4_physical).entries[i3];
    if entry & PRESENT == 0 {
        return None;
    }

    let mut current = table(entry & ADDRESS_MASK).entries[i2];
    if current & PRESENT == 0 {
        return None;
    }
    current = table(current & ADDRESS_MASK).entries[i1];
    if current & PRESENT == 0 {
        return None;
    }

    let entry = &mut table(current & ADDRESS_MASK).entries[i0];
    if *entry & PRESENT == 0 {
        return None;
    }
    let physical = *entry & ADDRESS_MASK;
    *entry = 0;
    if pml4_physical == current_pml4() {
        unsafe { cpu::invlpg(virtual_address) };
    }
    Some(physical)
}

/// Yeni adres alanı: çekirdek üst yarısı paylaşılır, alt yarı boş.
pub fn new_address_space() -> Option<u64> {
    let pml4 = allocate_table()?;
    let source = table(current_pml4());
    let destination = table(pml4);

    for index in KERNEL_HALF_START_INDEX..512 {
        destination.entries[index] = source.entries[index];
    }

    Some(pml4)
}

/// Kullanıcı yarısını derin kopyalayan yeni bir adres alanı üretir.
///
/// `fork` için kullanılır: her 4 KiB sayfa yeni bir çerçeveye kopyalanır.
/// 2 MiB/1 GiB büyük sayfa henüz kullanıcı uzayında yok; karşılaşılırsa
/// paylaşılır (yazma kopyalama M3 sonunda).
pub fn copy_address_space(source_pml4: u64) -> Option<u64> {
    let new_pml4 = allocate_table()?;
    let source = table(source_pml4);
    let destination = table(new_pml4);

    for index in KERNEL_HALF_START_INDEX..512 {
        destination.entries[index] = source.entries[index];
    }

    for index in 0..KERNEL_HALF_START_INDEX {
        let entry = source.entries[index];
        if entry & PRESENT != 0 {
            destination.entries[index] = copy_table(entry, 3)?;
        }
    }

    Some(new_pml4)
}

/// `level`: girdinin gösterdiği tablo seviyesi. 3 = PDPT, 1 = sayfa tablosu.
fn copy_table(entry: u64, level: u32) -> Option<u64> {
    let new_table = allocate_table()?;
    let source = table(entry & ADDRESS_MASK);
    let destination = table(new_table);

    for index in 0..512 {
        let child = source.entries[index];
        if child & PRESENT == 0 {
            continue;
        }

        if level == 1 {
            let frame = pmm::alloc_frame()?;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    hhdm::phys_to_ptr::<u8>(child & ADDRESS_MASK),
                    hhdm::phys_to_ptr::<u8>(frame),
                    FRAME_SIZE as usize,
                );
            }
            destination.entries[index] = frame | (child & !ADDRESS_MASK);
        } else if child & HUGE_PAGE != 0 {
            destination.entries[index] = child;
        } else {
            destination.entries[index] = copy_table(child, level - 1)?;
        }
    }

    Some(new_table | (entry & !ADDRESS_MASK))
}

/// Hata ayıklama: bir sanal adresin yaprak PTE değerini döndürür.
pub fn leaf_entry(pml4_physical: u64, virtual_address: u64) -> Option<u64> {
    let [i3, i2, i1, i0] = indexes(virtual_address);
    let entry = table(pml4_physical).entries[i3];
    if entry & PRESENT == 0 {
        return None;
    }
    let entry = table(entry & ADDRESS_MASK).entries[i2];
    if entry & PRESENT == 0 || entry & HUGE_PAGE != 0 {
        return Some(entry);
    }
    let entry = table(entry & ADDRESS_MASK).entries[i1];
    if entry & PRESENT == 0 || entry & HUGE_PAGE != 0 {
        return Some(entry);
    }
    Some(table(entry & ADDRESS_MASK).entries[i0])
}

pub fn switch_to(pml4_physical: u64) {
    unsafe { cpu::write_cr3(pml4_physical) };
}

/// Kernel'in mevcut eşlemesinde bir aralık ayırır (ileride heap büyütmesi).
pub fn map_range(virtual_start: u64, size: usize, flags: u64) -> Result<(), MapError> {
    let pages = (size + FRAME_SIZE as usize - 1) / FRAME_SIZE as usize;
    for index in 0..pages {
        let frame = pmm::alloc_frame().ok_or(MapError::FrameAllocation)?;
        map(virtual_start + index as u64 * FRAME_SIZE, frame, flags)?;
    }
    Ok(())
}

/// MMIO için çekirdek sanal adres aralığı. HHDM'in (RAM) uzağında, üst yarıda.
const MMIO_BASE: u64 = 0xffff_c000_0000_0000;
static NEXT_MMIO: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(MMIO_BASE);

/// Fiziksel MMIO aralığını çekirdek adres uzayına kalıcı olarak eşler.
///
/// Süreç adres alanları oluşturulurken çekirdek üst yarısı kopyalandığı için
/// sürücüler süreçler başlamadan önce eşlenmelidir.
pub fn mmio_map(physical: u64, size: usize) -> Option<u64> {
    let pages = (size + FRAME_SIZE as usize - 1) / FRAME_SIZE as usize;
    let length = pages as u64 * FRAME_SIZE;
    let virtual_base = NEXT_MMIO.fetch_add(length, core::sync::atomic::Ordering::Relaxed);
    let physical_base = physical & !(FRAME_SIZE - 1);

    for index in 0..pages as u64 {
        let offset = index * FRAME_SIZE;
        map_in(
            current_pml4(),
            virtual_base + offset,
            physical_base + offset,
            WRITABLE | NO_EXECUTE,
        )
        .ok()?;
    }

    Some(virtual_base + (physical & (FRAME_SIZE - 1)))
}

pub fn init() {
    println!("mem: mevcut PML4 {:#x}", current_pml4());
}

/// Kullanıcı adres uzayındaki tüm ELF modül tabanları (sayfa hizasında
/// `ELF` imzası aranır; teşhis amaçlı).
pub fn module_bases() -> alloc::vec::Vec<u64> {
    let mut bases = alloc::vec::Vec::new();
    let pml4 = current_pml4();
    let mut address = 0x6000_0000_0000u64;
    while address < 0x6000_0200_0000u64 {
        if let Some(physical) = translate(pml4, address) {
            let pointer = hhdm::phys_to_ptr::<u8>(physical);
            let magic = unsafe { core::slice::from_raw_parts(pointer, 4) };
            if magic == b"\x7fELF" {
                bases.push(address);
            }
        }
        address += 0x1000;
    }
    bases
}
