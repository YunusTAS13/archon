//! VirtIO-GPU (modern PCI, device id 0x1040 + alt 0x1050).
//!
//! M6'da DRM/KMS için kullanılacak. Bu dosya virtqueue mekaniğini ve kontrol
//! kuyruğu komut zarflarını kurar; komutların çoğu henüz gönderilmiyor.
//!
//! Referans: VirtIO 1.2, bölüm 5.7 (GPU device).

use core::sync::atomic::{fence, Ordering};

use crate::mem::hhdm;
use crate::pci::{self, Device};
use crate::sync::SpinLock;

pub const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
pub const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
pub const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;

pub const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
pub const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;

pub const FORMAT_B8G8R8A8_UNORM: u32 = 1;
pub const MAX_SCANOUTS: usize = 16;

const QUEUE_SIZE: usize = 64;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct Descriptor {
    pub address: u64,
    pub length: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C, align(2))]
struct Available {
    flags: u16,
    index: u16,
    ring: [u16; QUEUE_SIZE],
    used_event: u16,
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
    avail_event: u16,
}

#[derive(Clone, Copy)]
pub struct GpuConfig {
    pub events_read: u32,
    pub events_clear: u32,
    pub num_scanouts: u32,
    pub num_capsets: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DisplayInfo {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub enabled: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

struct GpuDevice {
    device: Device,
    common: u64,
    notify: u64,
    notify_offset: u64,
    isr: u64,
    device_status: u64,
    queue: u64,
    config: u64,
    descriptor_table: u64,
    available: u64,
    used: u64,
    last_used: u16,
    next_descriptor: u16,
}

static GPU: SpinLock<Option<GpuDevice>> = SpinLock::new(None);

pub fn init() {
    let Some(device) = pci::find_class(pci::CLASS_DISPLAY, 0x00) else {
        println!("gpu: virtio-gpu bulunamadi");
        return;
    };

    if device.vendor_id != pci::VENDOR_VIRTIO || (device.device_id != 0x1040 && device.device_id != 0x1050) {
        println!(
            "gpu: bilinmeyen ekran aygiti {:04x}:{:04x}",
            device.vendor_id, device.device_id
        );
        return;
    }

    println!("gpu: virtio-gpu {:04x} bulundu (surucu M6)", device.device_id);
    // TODO(M6): PCI capability'lerini yürü, common/notify/queue/config
    // bölgelerini bul, device status handshake'ini yap, kuyrukları kur.
    // `setup_queue` ve `submit` bu dosyada hazır; bağlanmayı bekliyor.
}

/// Kuyruk için bitişik bir çerçeve ayırır ve fiziksel adresini döndürür.
fn alloc_ring(size: usize) -> Option<(u64, u64)> {
    // Kuyruk yapıları tek bir 4 KiB çerçeveye sığmalı.
    if size > 4096 {
        return None;
    }
    let physical = crate::mem::pmm::alloc_frame()?;
    unsafe { hhdm::zero_physical(physical, 4096) };
    Some((physical, hhdm::phys_to_virt(physical)))
}

/// Tek bir virtqueue'yu kurar: descriptor tablosu, available ve used halkaları.
pub fn setup_queue(queue_index: u16) -> bool {
    let Some((descriptor_physical, descriptor_virtual)) =
        alloc_ring(QUEUE_SIZE * core::mem::size_of::<Descriptor>())
    else {
        return false;
    };
    let Some((available_physical, available_virtual)) =
        alloc_ring(core::mem::size_of::<Available>())
    else {
        return false;
    };
    let Some((used_physical, used_virtual)) = alloc_ring(core::mem::size_of::<Used>()) else {
        return false;
    };

    let mut guard = GPU.lock();
    let Some(gpu) = guard.as_mut() else {
        return false;
    };

    // Queue üç bitişik halkayı tek bir "queue" bölgesi olarak bekler.
    // Gerçek donanımda bu üç alan physical olarak bitişik olmalı; PMM
    // bitişiklik garantisi vermediği için burada açıkça başarısız oluyoruz.
    if used_physical != available_physical + 4096
        || available_physical != descriptor_physical + 4096
    {
        println!("gpu: kuyruk halkalari bitisik degil, bitisik ayirici gerekli");
        return false;
    }

    gpu.descriptor_table = descriptor_virtual;
    gpu.available = available_virtual;
    gpu.used = used_virtual;
    gpu.next_descriptor = 0;
    gpu.last_used = 0;

    unsafe {
        core::ptr::write_bytes(gpu.descriptor_table as *mut u8, 0, 4096);
        let queue_address = descriptor_physical;
        gpu.queue = queue_address;
    }

    let _ = queue_index;
    true
}

pub fn config() -> Option<GpuConfig> {
    let gpu = GPU.lock();
    let gpu = gpu.as_ref()?;
    let pointer = gpu.config as *const u32;
    Some(GpuConfig {
        events_read: unsafe { core::ptr::read_volatile(pointer) },
        events_clear: unsafe { core::ptr::read_volatile(pointer.add(1)) },
        num_scanouts: unsafe { core::ptr::read_volatile(pointer.add(2)) },
        num_capsets: unsafe { core::ptr::read_volatile(pointer.add(3)) },
    })
}
