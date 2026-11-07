//! PCI yapılandırma uzayı (legacy 0xCF8/0xCFC) ve BAR keşfi.
//!
//! ECAM (MCFG) sonra eklenecek; legacy yol tüm QEMU makinelerinde çalışır.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::port::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

pub const CLASS_STORAGE: u8 = 0x01;
pub const CLASS_NETWORK: u8 = 0x02;
pub const CLASS_DISPLAY: u8 = 0x03;
pub const CLASS_SERIAL_BUS: u8 = 0x0C;

pub const VENDOR_VIRTIO: u16 = 0x1AF4;

#[derive(Clone, Copy)]
pub struct Address {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

#[derive(Clone, Copy)]
pub struct Device {
    pub address: Address,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
}

fn config_address(address: Address, offset: u8) -> u32 {
    0x8000_0000
        | ((address.bus as u32) << 16)
        | ((address.device as u32) << 11)
        | ((address.function as u32) << 8)
        | (offset as u32 & 0xFC)
}

pub fn read32(address: Address, offset: u8) -> u32 {
    unsafe {
        outl(CONFIG_ADDRESS, config_address(address, offset));
        inl(CONFIG_DATA)
    }
}

pub fn read16(address: Address, offset: u8) -> u16 {
    let value = read32(address, offset);
    ((value >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

pub fn read8(address: Address, offset: u8) -> u8 {
    let value = read32(address, offset);
    ((value >> ((offset & 3) * 8)) & 0xFF) as u8
}

pub fn write32(address: Address, offset: u8, value: u32) {
    unsafe {
        outl(CONFIG_ADDRESS, config_address(address, offset));
        outl(CONFIG_DATA, value);
    }
}

pub fn write16(address: Address, offset: u8, value: u16) {
    let current = read32(address, offset);
    let shift = (offset & 2) * 8;
    let mask = !(0xFFFFu32 << shift);
    write32(address, offset, (current & mask) | ((value as u32) << shift));
}

/// 64 bit BAR çiftini okur. Dönüş: (taban, bellek mi, prefetch mi).
pub fn bar(address: Address, index: u8) -> Option<(u64, bool, bool)> {
    let offset = 0x10 + index * 4;
    let raw = read32(address, offset);
    if raw == 0 || raw == 0xFFFF_FFFF {
        return None;
    }

    let memory = raw & 1 == 0;
    let prefetch = raw & 8 != 0;
    let mut base = (raw & !0xF) as u64;

    if memory && (raw & 0x6) == 0x4 && index < 5 {
        // 64 bit BAR: üst yarı bir sonraki indekste.
        let high = read32(address, offset + 4) as u64;
        base |= high << 32;
    }

    Some((base, memory, prefetch))
}

static FOUND: AtomicU64 = AtomicU64::new(0);

fn probe(address: Address) -> Option<Device> {
    let vendor_id = read16(address, 0x00);
    if vendor_id == 0xFFFF || vendor_id == 0 {
        return None;
    }

    Some(Device {
        address,
        vendor_id,
        device_id: read16(address, 0x02),
        class: read8(address, 0x0B),
        subclass: read8(address, 0x0A),
        prog_if: read8(address, 0x09),
        header_type: read8(address, 0x0E),
    })
}

pub fn scan() -> impl Iterator<Item = Device> {
    (0..=255u16).flat_map(|bus| {
        (0..32u8).flat_map(move |device| {
            (0..8u8).filter_map(move |function| {
                if function != 0 {
                    let header = read8(
                        Address {
                            bus: bus as u8,
                            device,
                            function: 0,
                        },
                        0x0E,
                    );
                    // Çok işlevli olmayan cihazların 1..8 işlevleri yoktur.
                    if header & 0x80 == 0 {
                        return None;
                    }
                }
                probe(Address {
                    bus: bus as u8,
                    device,
                    function,
                })
            })
        })
    })
}

pub fn init() {
    let mut count = 0u64;
    for device in scan() {
        count += 1;
        match device.class {
            CLASS_STORAGE | CLASS_NETWORK | CLASS_DISPLAY => {
                println!(
                    "pci: {:02x}:{:02x}.{} {:04x}:{:04x} sinif {:02x}.{:02x}",
                    device.address.bus,
                    device.address.device,
                    device.address.function,
                    device.vendor_id,
                    device.device_id,
                    device.class,
                    device.subclass
                );
            }
            _ => {}
        }
    }
    FOUND.store(count, Ordering::Relaxed);
    println!("pci: {} aygit", count);
}

pub fn device_count() -> u64 {
    FOUND.load(Ordering::Relaxed)
}

pub fn find(vendor: u16, device: u16) -> Option<Device> {
    scan().find(|found| found.vendor_id == vendor && found.device_id == device)
}

pub fn find_class(class: u8, subclass: u8) -> Option<Device> {
    scan().find(|found| found.class == class && found.subclass == subclass)
}

/// Bellek uzayı BAR'larını ve bus master'ı açar.
pub fn enable_memory_space(device: &Device) {
    let command = read16(device.address, 0x04);
    write16(device.address, 0x04, command | 0x2 | 0x4);
}
