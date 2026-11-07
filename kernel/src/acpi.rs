//! ACPI tabloları: RSDP → XSDT/RSDT → MADT, FADT.
//!
//! Limine RSDP'yi zaten HHDM sanal adresi olarak verir. Tablo işaretçileri
//! içeride fiziksel olduğu için `hhdm::phys_to_virt` ile çevrilir; bu modül
//! içindeki tüm adresler sanaldır.
//!
//! AML yorumlayıcısı yok; Plasma için gereken `_OSI` yanıtları M7'de sabit
//! tablo olarak eklenecek (bkz. docs/KDE-PLAN.md).

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::mem::hhdm;

const MADT_SIGNATURE: u32 = u32::from_le_bytes(*b"APIC");

static LAPIC_BASE: AtomicU64 = AtomicU64::new(0);
static IOAPIC_BASE: AtomicU64 = AtomicU64::new(0);
static CPU_COUNT: AtomicU32 = AtomicU32::new(0);

pub fn init(rsdp_virtual: u64) {
    if rsdp_virtual == 0 {
        println!("acpi: RSDP yok, ACPI devre disi");
        return;
    }

    let Some(root) = find_root_table(rsdp_virtual) else {
        println!("acpi: XSDT/RSDT bulunamadi");
        return;
    };

    if let Some(madt) = find_table(root, MADT_SIGNATURE) {
        parse_madt(madt);
    } else {
        println!("acpi: MADT yok");
    }
}

#[repr(C, packed)]
struct Rsdp {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
}

#[repr(C, packed)]
struct SdtHeader {
    signature: u32,
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

fn checksum(virtual_address: u64, length: usize) -> bool {
    let pointer = virtual_address as *const u8;
    let mut sum = 0u8;
    for index in 0..length {
        sum = sum.wrapping_add(unsafe { *pointer.add(index) });
    }
    sum == 0
}

fn read_u64(virtual_address: u64) -> u64 {
    unsafe { core::ptr::read_unaligned(virtual_address as *const u64) }
}

fn read_u32(virtual_address: u64) -> u32 {
    unsafe { core::ptr::read_unaligned(virtual_address as *const u32) }
}

fn header(virtual_address: u64) -> SdtHeader {
    unsafe { core::ptr::read_unaligned(virtual_address as *const SdtHeader) }
}

/// RSDP'den XSDT (ACPI 2+) veya RSDT (ACPI 1) sanal adresini döndürür.
fn find_root_table(rsdp_virtual: u64) -> Option<u64> {
    let rsdp = unsafe { core::ptr::read_unaligned(rsdp_virtual as *const Rsdp) };
    if &rsdp.signature != b"RSD PTR " {
        return None;
    }
    if !checksum(rsdp_virtual, 20) {
        return None;
    }

    if rsdp.revision >= 2 {
        let xsdt_physical = read_u64(rsdp_virtual + 24);
        if xsdt_physical != 0 {
            return Some(hhdm::phys_to_virt(xsdt_physical));
        }
    }

    Some(hhdm::phys_to_virt(rsdp.rsdt_address as u64))
}

fn find_table(root_virtual: u64, signature: u32) -> Option<u64> {
    let root = header(root_virtual);
    let length = root.length as usize;
    let header_size = core::mem::size_of::<SdtHeader>();
    if length < header_size {
        return None;
    }

    let xsdt = root.signature == u32::from_le_bytes(*b"XSDT");
    let pointer_size = if xsdt { 8 } else { 4 };
    let entry_count = (length - header_size) / pointer_size;

    for index in 0..entry_count {
        let entry_address = root_virtual + header_size as u64 + (index * pointer_size) as u64;
        let table_physical = if xsdt {
            read_u64(entry_address)
        } else {
            read_u32(entry_address) as u64
        };
        if table_physical == 0 {
            continue;
        }
        let table_virtual = hhdm::phys_to_virt(table_physical);
        if header(table_virtual).signature == signature {
            return Some(table_virtual);
        }
    }

    None
}

fn parse_madt(madt_virtual: u64) {
    let madt = header(madt_virtual);
    let length = madt.length as usize;
    let header_size = core::mem::size_of::<SdtHeader>();
    let mut offset = header_size + 8; // LAPIC adresi + bayraklar

    let mut cpus = 0u32;
    while offset + 2 <= length {
        let entry_type = unsafe { *(madt_virtual as *const u8).add(offset) };
        let entry_length = unsafe { *(madt_virtual as *const u8).add(offset + 1) } as usize;
        if entry_length < 2 || offset + entry_length > length {
            break;
        }

        match entry_type {
            0 => cpus += 1,
            1 => {
                // IO APIC: [id, reserved, adres(4), gsi(4)]
                let address = read_u32(madt_virtual + offset as u64 + 4);
                IOAPIC_BASE.store(address as u64, Ordering::Relaxed);
            }
            _ => {}
        }

        offset += entry_length;
    }

    let lapic = read_u32(madt_virtual + header_size as u64);
    LAPIC_BASE.store(lapic as u64, Ordering::Relaxed);
    CPU_COUNT.store(cpus, Ordering::Relaxed);

    println!(
        "acpi: MADT LAPIC {:#x}, IOAPIC {:#x}, {} islemci",
        LAPIC_BASE.load(Ordering::Relaxed),
        IOAPIC_BASE.load(Ordering::Relaxed),
        CPU_COUNT.load(Ordering::Relaxed)
    );
}

pub fn lapic_base() -> u64 {
    LAPIC_BASE.load(Ordering::Relaxed)
}

pub fn ioapic_base() -> u64 {
    IOAPIC_BASE.load(Ordering::Relaxed)
}

pub fn cpu_count() -> u32 {
    CPU_COUNT.load(Ordering::Relaxed)
}
