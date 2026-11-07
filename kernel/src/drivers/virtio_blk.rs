//! virtio-blk: modern (virtio 1.0) PCI blok aygıtı.
//!
//! Tek virtqueue kurulur; her istek üç tanımlayıcıdan oluşur:
//! başlık (16 bayt), veri (512 bayt sektörler) ve durum baytı.
//! Sürücü 512 baytlık sektörle çalışır; ext4 katmanı blok numaralarını
//! sektöre çevirir.
//!
//! Referans: VirtIO 1.2 bölüm 4.1 (PCI) ve 5.2 (blok aygıtı).

use alloc::sync::Arc;
use core::sync::atomic::{fence, Ordering};

use crate::fs::ext4::BlockDevice;
use crate::fs::{Result, VfsError};
use crate::mem::{hhdm, pmm};
use crate::pci::{self, Device};
use crate::sync::SpinLock;

const VIRTIO_VENDOR: u16 = 0x1AF4;
/// Geçişli (0x1001) ve modern (0x1042) aygıt kimlikleri.
const VIRTIO_BLK_IDS: [u16; 2] = [0x1042, 0x1001];

const VIRTQ_DESC_F_NEXT: u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;

const VRING_DESC_F_NEXT: u16 = VIRTQ_DESC_F_NEXT;

const QUEUE_SIZE: usize = 64;
const SECTOR_SIZE: usize = 512;

// Common config alanları (VIRTIO_PCI_CAP_COMMON_CFG).
const COMMON_DEVICE_FEATURE_SELECT: u64 = 0x00;
const COMMON_DEVICE_FEATURE: u64 = 0x04;
const COMMON_DRIVER_FEATURE_SELECT: u64 = 0x08;
const COMMON_DRIVER_FEATURE: u64 = 0x0C;
const COMMON_DEVICE_STATUS: u64 = 0x14;
const COMMON_QUEUE_SELECT: u64 = 0x16;
const COMMON_QUEUE_SIZE: u64 = 0x18;
const COMMON_QUEUE_ENABLE: u64 = 0x1C;
const COMMON_QUEUE_NOTIFY_OFF: u64 = 0x1E;
const COMMON_QUEUE_DESC: u64 = 0x20;
const COMMON_QUEUE_AVAIL: u64 = 0x28;
const COMMON_QUEUE_USED: u64 = 0x30;

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;

const FEATURE_VERSION_1: u32 = 1; // bit 32

const CAP_TYPE_COMMON: u8 = 1;
const CAP_TYPE_NOTIFY: u8 = 2;
const CAP_TYPE_DEVICE: u8 = 4;

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct Descriptor {
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
}

#[repr(C, align(2))]
struct Available {
    flags: u16,
    index: u16,
    ring: [u16; QUEUE_SIZE],
}

#[repr(C, align(4))]
struct UsedElement {
    id: u32,
    length: u32,
}

#[repr(C, align(4))]
struct Used {
    flags: u16,
    index: u16,
    ring: [UsedElement; QUEUE_SIZE],
}

#[repr(C)]
struct RequestHeader {
    kind: u32,
    reserved: u32,
    sector: u64,
}

pub struct VirtioBlk {
    common: u64,
    notify: u64,
    device_config: u64,
    notify_offset: u16,
    notify_multiplier: u32,
    queue_size: u16,
    descriptors: u64,
    available: u64,
    used: u64,
    last_used: core::sync::atomic::AtomicU16,
    header_frame: u64,
    data_frame: u64,
    capacity_sectors: u64,
}

static DEVICE: SpinLock<Option<Arc<VirtioBlk>>> = SpinLock::new(None);

fn read8(base: u64, offset: u64) -> u8 {
    unsafe { core::ptr::read_volatile((base + offset) as *const u8) }
}

fn read16(base: u64, offset: u64) -> u16 {
    unsafe { core::ptr::read_volatile((base + offset) as *const u16) }
}

fn read32(base: u64, offset: u64) -> u32 {
    unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
}

fn read64(base: u64, offset: u64) -> u64 {
    unsafe { core::ptr::read_volatile((base + offset) as *const u64) }
}

fn write8(base: u64, offset: u64, value: u8) {
    unsafe { core::ptr::write_volatile((base + offset) as *mut u8, value) }
}

fn write16(base: u64, offset: u64, value: u16) {
    unsafe { core::ptr::write_volatile((base + offset) as *mut u16, value) }
}

fn write32(base: u64, offset: u64, value: u32) {
    unsafe { core::ptr::write_volatile((base + offset) as *mut u32, value) }
}

fn write64(base: u64, offset: u64, value: u64) {
    unsafe { core::ptr::write_volatile((base + offset) as *mut u64, value) }
}

struct Capabilities {
    common: Option<(u8, u32, u32)>,
    /// (bar, offset, uzunluk, yapılandırma uzayındaki capability işaretçisi)
    notify: Option<(u8, u32, u32, u8)>,
    device: Option<(u8, u32, u32)>,
}

/// PCI yapılandırma uzayındaki vendor capability'lerini yürür.
fn capabilities(device: &Device) -> Capabilities {
    let mut result = Capabilities {
        common: None,
        notify: None,
        device: None,
    };

    let mut pointer = pci::read8(device.address, 0x34) & 0xFC;
    let mut guard = 0;

    while pointer >= 0x40 && guard < 48 {
        guard += 1;
        let identifier = pci::read8(device.address, pointer);
        let next = pci::read8(device.address, pointer + 1) & 0xFC;

        if identifier == 0x09 {
            // struct virtio_pci_cap: id, next, len, cfg_type, bar, id, pad, offset, length
            let kind = pci::read8(device.address, pointer + 3);
            let bar_index = pci::read8(device.address, pointer + 4);
            let offset = pci::read32(device.address, pointer + 8);
            let length = pci::read32(device.address, pointer + 12);
            match kind {
                CAP_TYPE_COMMON => result.common = Some((bar_index, offset, length)),
                CAP_TYPE_NOTIFY => {
                    result.notify = Some((bar_index, offset, length, pointer as u8))
                }
                CAP_TYPE_DEVICE => result.device = Some((bar_index, offset, length)),
                _ => {}
            }
        }

        if next == 0 || next == pointer {
            break;
        }
        pointer = next;
    }

    result
}

fn map_region(device: &Device, bar_index: u8, offset: u32, length: u32) -> Option<u64> {
    let (bar_base, memory, _) = pci::bar(device.address, bar_index)?;
    if !memory {
        return None;
    }
    crate::mem::vmm::mmio_map(bar_base + offset as u64, length.max(1) as usize)
}

fn alloc_zeroed_frame() -> Option<u64> {
    let frame = pmm::alloc_frame()?;
    unsafe { hhdm::zero_physical(frame, 4096) };
    Some(frame)
}

pub fn init() {
    let Some(device) = VIRTIO_BLK_IDS
        .iter()
        .find_map(|identifier| pci::find(VIRTIO_VENDOR, *identifier))
    else {
        println!("blk: virtio-blk bulunamadi");
        return;
    };

    pci::enable_memory_space(&device);

    let capabilities = capabilities(&device);
    let (Some(common_cap), Some(notify_cap)) = (capabilities.common, capabilities.notify) else {
        println!("blk: virtio capability eksik");
        return;
    };

    let Some(common) = map_region(&device, common_cap.0, common_cap.1, common_cap.2) else {
        println!("blk: common bolgesi eslenemedi");
        return;
    };
    let Some(notify) = map_region(&device, notify_cap.0, notify_cap.1, notify_cap.2) else {
        println!("blk: notify bolgesi eslenemedi");
        return;
    };
    // Çarpan, capability yapısının hemen ardından yapılandırma uzayında.
    let notify_multiplier = pci::read32(device.address, notify_cap.3 + 16);

    let device_config = match capabilities.device {
        Some((bar, offset, length)) => map_region(&device, bar, offset, length).unwrap_or(0),
        None => 0,
    };

    // Sıfırla ve sürücü el sıkışmasını yap.
    write8(common, COMMON_DEVICE_STATUS, 0);
    write8(common, COMMON_DEVICE_STATUS, STATUS_ACKNOWLEDGE);
    write8(common, COMMON_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

    // VIRTIO_F_VERSION_1 kabul et.
    write32(common, COMMON_DRIVER_FEATURE_SELECT, 1);
    write32(common, COMMON_DRIVER_FEATURE, FEATURE_VERSION_1);
    write32(common, COMMON_DRIVER_FEATURE_SELECT, 0);
    write32(common, COMMON_DRIVER_FEATURE, 0);

    write8(
        common,
        COMMON_DEVICE_STATUS,
        STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK,
    );
    if read8(common, COMMON_DEVICE_STATUS) & STATUS_FEATURES_OK == 0 {
        println!("blk: ozellik anlasmasi basarisiz");
        return;
    }

    // Kuyruk 0.
    write16(common, COMMON_QUEUE_SELECT, 0);
    let maximum = read16(common, COMMON_QUEUE_SIZE);
    if maximum == 0 {
        println!("blk: kuyruk yok");
        return;
    }
    let queue_size = maximum.min(QUEUE_SIZE as u16);
    write16(common, COMMON_QUEUE_SIZE, queue_size);

    let (Some(descriptors), Some(available), Some(used)) = (
        alloc_zeroed_frame(),
        alloc_zeroed_frame(),
        alloc_zeroed_frame(),
    ) else {
        println!("blk: kuyruk halkasi ayrilamadi");
        return;
    };

    write64(common, COMMON_QUEUE_DESC, descriptors);
    write64(common, COMMON_QUEUE_AVAIL, available);
    write64(common, COMMON_QUEUE_USED, used);
    write16(common, COMMON_QUEUE_ENABLE, 1);
    let notify_offset = read16(common, COMMON_QUEUE_NOTIFY_OFF);

    write8(
        common,
        COMMON_DEVICE_STATUS,
        STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
    );

    let capacity_sectors = if device_config != 0 {
        read64(device_config, 0)
    } else {
        0
    };

    let (_header_frame, data_frame) = match (alloc_zeroed_frame(), alloc_zeroed_frame()) {
        (Some(header), Some(data)) => (header, data),
        _ => {
            println!("blk: istek cercevesi ayrilamadi");
            return;
        }
    };

    let block_device = Arc::new(VirtioBlk {
        common,
        notify,
        device_config,
        notify_offset,
        notify_multiplier,
        queue_size,
        descriptors,
        available,
        used,
        last_used: core::sync::atomic::AtomicU16::new(0),
        header_frame: _header_frame,
        data_frame,
        capacity_sectors,
    });

    println!(
        "blk: virtio-blk hazir, {} sektor ({} MiB), kuyruk {}",
        capacity_sectors,
        capacity_sectors / 2048,
        queue_size
    );

    *DEVICE.lock() = Some(block_device);
}

/// Disk kapasitesi (MiB); aygıt yoksa `None`.
pub fn capacity_mib() -> Option<u64> {
    DEVICE
        .lock()
        .as_ref()
        .map(|device| device.capacity_sectors / 2048)
}

/// Kurulu blok aygıtı (ext4 bağlamak için).
pub fn device() -> Option<Arc<VirtioBlk>> {
    DEVICE.lock().clone()
}

impl VirtioBlk {
    fn notify_queue(&self) {
        let address = self.notify + self.notify_offset as u64 * self.notify_multiplier as u64;
        unsafe { core::ptr::write_volatile(address as *mut u16, 0) };
    }

    /// Tek bir istek gönderir: `sectors` sektör okur/yazar.
    fn request(&self, read: bool, sector: u64, sectors: u64, data: u64) -> Result<()> {
        if sectors == 0 || sectors * SECTOR_SIZE as u64 > 4096 {
            return Err(VfsError::InvalidArgument);
        }

        let header = hhdm::phys_to_ptr::<RequestHeader>(self.header_frame);
        let status_physical = self.header_frame + 4095;
        let status = hhdm::phys_to_ptr::<u8>(status_physical);

        unsafe {
            (*header).kind = if read { 0 } else { 1 };
            (*header).reserved = 0;
            (*header).sector = sector;
            *status = 0xFF;
        }

        let descriptors = hhdm::phys_to_ptr::<Descriptor>(self.descriptors);
        let available = hhdm::phys_to_ptr::<Available>(self.available);

        unsafe {
            (*descriptors.add(0)) = Descriptor {
                address: self.header_frame,
                length: core::mem::size_of::<RequestHeader>() as u32,
                flags: VRING_DESC_F_NEXT,
                next: 1,
            };
            (*descriptors.add(1)) = Descriptor {
                address: data,
                length: (sectors * SECTOR_SIZE as u64) as u32,
                flags: VRING_DESC_F_NEXT | if read { VIRTQ_DESC_F_WRITE } else { 0 },
                next: 2,
            };
            (*descriptors.add(2)) = Descriptor {
                address: status_physical,
                length: 1,
                flags: VIRTQ_DESC_F_WRITE,
                next: 0,
            };
        }

        unsafe {
            let index = (*available).index;
            (*available).ring[(index as usize) % self.queue_size as usize] = 0;
            fence(Ordering::SeqCst);
            (*available).index = index.wrapping_add(1);
        }
        fence(Ordering::SeqCst);
        self.notify_queue();

        let used = hhdm::phys_to_ptr::<Used>(self.used);
        let expected = self.last_used.load(Ordering::Relaxed).wrapping_add(1);
        let mut spin = 0u64;
        loop {
            fence(Ordering::SeqCst);
            if unsafe { core::ptr::read_volatile(&(*used).index) } == expected {
                break;
            }
            spin += 1;
            if spin > 2_000_000 {
                println!(
                    "blk: zaman asimi, used {} beklenen {}",
                    unsafe { core::ptr::read_volatile(&(*used).index) },
                    expected
                );
                return Err(VfsError::Io);
            }
            core::hint::spin_loop();
        }

        self.last_used.store(expected, Ordering::Relaxed);
        let code = unsafe { core::ptr::read_volatile(status) };
        if code != 0 {
            return Err(VfsError::Io);
        }
        Ok(())
    }
}

impl BlockDevice for VirtioBlk {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn read_block(&self, block: u64, buffer: &mut [u8]) -> Result<()> {
        let sectors = ((buffer.len() + SECTOR_SIZE - 1) / SECTOR_SIZE) as u64;
        if buffer.is_empty() || block + sectors > self.capacity_sectors {
            return Err(VfsError::InvalidArgument);
        }

        // Veri çerçevesine oku, sonra çağıranın tamponuna kopyala.
        self.request(true, block, sectors, self.data_frame)?;

        let source = hhdm::phys_to_ptr::<u8>(self.data_frame);
        unsafe {
            core::ptr::copy_nonoverlapping(source, buffer.as_mut_ptr(), buffer.len());
        }
        Ok(())
    }

    fn write_block(&self, block: u64, buffer: &[u8]) -> Result<()> {
        let sectors = ((buffer.len() + SECTOR_SIZE - 1) / SECTOR_SIZE) as u64;
        if buffer.is_empty() || block + sectors > self.capacity_sectors {
            return Err(VfsError::InvalidArgument);
        }

        let destination = hhdm::phys_to_ptr::<u8>(self.data_frame);
        unsafe {
            core::ptr::copy_nonoverlapping(buffer.as_ptr(), destination, buffer.len());
        }
        self.request(false, block, sectors, self.data_frame)
    }
}
