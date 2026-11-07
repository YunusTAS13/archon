//! Limine boot protokolü bağları.
//!
//! `limine_ids.rs` üretilen dosyadır; sihirli sayılar `limine.h`'den gelir.
//! Struct yerleşimleri Limine base revision 3'e göredir.

use core::cell::UnsafeCell;

use super::limine_ids;

pub const BASE_REVISION: u64 = 3;

pub const MEMMAP_USABLE: u64 = 0;
pub const MEMMAP_RESERVED: u64 = 1;
pub const MEMMAP_ACPI_RECLAIMABLE: u64 = 2;
pub const MEMMAP_ACPI_NVS: u64 = 3;
pub const MEMMAP_BAD_MEMORY: u64 = 4;
pub const MEMMAP_BOOTLOADER_RECLAIMABLE: u64 = 5;
pub const MEMMAP_KERNEL_AND_MODULES: u64 = 6;
pub const MEMMAP_FRAMEBUFFER: u64 = 7;

pub const MEDIA_TYPE_USB: u64 = 1;

/// Limine yanıt yazacağı için `UnsafeCell` gerekir; aynı statik boot boyunca
/// tek çekirdek tarafından okunur.
#[repr(transparent)]
pub struct Request<T> {
    inner: UnsafeCell<T>,
}

impl<T> Request<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: UnsafeCell::new(value),
        }
    }

    /// # Safety
    /// Bootloader yanıtı yazdıktan sonra çağrılmalı.
    pub unsafe fn get(&self) -> &T {
        &*self.inner.get()
    }
}

unsafe impl<T> Sync for Request<T> {}

#[repr(C)]
pub struct FramebufferRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut FramebufferResponse,
}

#[repr(C)]
pub struct FramebufferResponse {
    pub revision: u64,
    pub framebuffer_count: u64,
    pub framebuffers: *mut *mut Framebuffer,
}

#[repr(C)]
pub struct Framebuffer {
    pub address: *mut u8,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
    _reserved: [u8; 7],
    pub edid_size: u64,
    pub edid: *mut u8,
    pub mode_count: u64,
    pub modes: *mut *mut VideoMode,
}

#[repr(C)]
pub struct VideoMode {
    pub pitch: u64,
    pub width: u64,
    pub height: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
}

#[repr(C)]
pub struct HhdmRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut HhdmResponse,
}

#[repr(C)]
pub struct HhdmResponse {
    pub revision: u64,
    pub offset: u64,
}

#[repr(C)]
pub struct MemmapRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut MemmapResponse,
}

#[repr(C)]
pub struct MemmapResponse {
    pub revision: u64,
    pub entry_count: u64,
    pub entries: *mut *mut MemmapEntry,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemmapEntry {
    pub base: u64,
    pub length: u64,
    pub kind: u64,
}

#[repr(C)]
pub struct RsdpRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut RsdpResponse,
}

#[repr(C)]
pub struct RsdpResponse {
    pub revision: u64,
    pub address: *mut u8,
}

#[repr(C)]
pub struct ExecutableAddressRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut ExecutableAddressResponse,
}

/// API revizyon 3'te alan sırası: fiziksel taban önce.
#[repr(C)]
pub struct ExecutableAddressResponse {
    pub revision: u64,
    pub physical_base: u64,
    pub virtual_base: u64,
}

#[repr(C)]
pub struct ModuleRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut ModuleResponse,
    pub internal_module_count: u64,
    pub internal_modules: *mut *mut File,
}

#[repr(C)]
pub struct ModuleResponse {
    pub revision: u64,
    pub module_count: u64,
    pub modules: *mut *mut File,
}

#[repr(C)]
pub struct File {
    pub revision: u64,
    pub address: *mut u8,
    pub size: u64,
    pub path: *const u8,
    pub cmdline: *const u8,
    pub media_type: u32,
    pub unused: u32,
    pub tftp_ip: u32,
    pub tftp_port: u32,
    pub partition_index: u32,
    pub mbr_disk_id: u32,
    pub gpt_disk_uuid: [u8; 16],
    pub gpt_part_uuid: [u8; 16],
    pub part_uuid: [u8; 16],
}

#[used]
#[link_section = ".limine_requests"]
pub static FRAMEBUFFER_REQUEST: Request<FramebufferRequest> = Request::new(FramebufferRequest {
    id: limine_ids::LIMINE_FRAMEBUFFER_REQUEST,
    revision: BASE_REVISION,
    response: core::ptr::null_mut(),
});

#[used]
#[link_section = ".limine_requests"]
pub static HHDM_REQUEST: Request<HhdmRequest> = Request::new(HhdmRequest {
    id: limine_ids::LIMINE_HHDM_REQUEST,
    revision: BASE_REVISION,
    response: core::ptr::null_mut(),
});

#[used]
#[link_section = ".limine_requests"]
pub static MEMMAP_REQUEST: Request<MemmapRequest> = Request::new(MemmapRequest {
    id: limine_ids::LIMINE_MEMMAP_REQUEST,
    revision: BASE_REVISION,
    response: core::ptr::null_mut(),
});

#[used]
#[link_section = ".limine_requests"]
pub static RSDP_REQUEST: Request<RsdpRequest> = Request::new(RsdpRequest {
    id: limine_ids::LIMINE_RSDP_REQUEST,
    revision: BASE_REVISION,
    response: core::ptr::null_mut(),
});

#[used]
#[link_section = ".limine_requests"]
pub static EXECUTABLE_ADDRESS_REQUEST: Request<ExecutableAddressRequest> =
    Request::new(ExecutableAddressRequest {
        id: limine_ids::LIMINE_EXECUTABLE_ADDRESS_REQUEST,
        revision: BASE_REVISION,
        response: core::ptr::null_mut(),
    });


/// Modül isteği revizyon 1'de ek alanlar taşır; revizyon 0 kullanıyoruz ki
/// Limine `internal_modules` alanlarına dokunmasın.
#[used]
#[link_section = ".limine_requests"]
pub static MODULE_REQUEST: Request<ModuleRequest> = Request::new(ModuleRequest {
    id: limine_ids::LIMINE_MODULE_REQUEST,
    revision: 0,
    response: core::ptr::null_mut(),
    internal_module_count: 0,
    internal_modules: core::ptr::null_mut(),
});

#[used]
#[link_section = ".limine_requests_start"]
static REQUESTS_START: u64 = 0;

#[used]
#[link_section = ".limine_requests_end"]
static REQUESTS_END: u64 = 0;

pub fn hhdm_offset() -> Option<u64> {
    let response = unsafe { (*HHDM_REQUEST.get()).response };
    if response.is_null() {
        return None;
    }
    Some(unsafe { (*response).offset })
}

pub fn memmap_count() -> usize {
    let response = unsafe { (*MEMMAP_REQUEST.get()).response };
    if response.is_null() {
        return 0;
    }
    unsafe { (*response).entry_count as usize }
}

/// Memmap'i sabit bir diziye kopyalar. Erken boot'ta heap yok, bu yüzden
/// çağıran tampon verir.
pub fn copy_memmap(out: &mut [MemmapEntry]) -> usize {
    let response = unsafe { (*MEMMAP_REQUEST.get()).response };
    if response.is_null() {
        return 0;
    }
    let response = unsafe { &*response };
    let count = core::cmp::min(response.entry_count as usize, out.len());
    for index in 0..count {
        let entry = unsafe { *response.entries.add(index) };
        out[index] = unsafe { *entry };
    }
    count
}

pub fn framebuffer() -> Option<&'static Framebuffer> {
    let response = unsafe { (*FRAMEBUFFER_REQUEST.get()).response };
    if response.is_null() {
        return None;
    }
    let response = unsafe { &*response };
    if response.framebuffer_count == 0 {
        return None;
    }
    Some(unsafe { &**response.framebuffers })
}

pub fn rsdp_address() -> Option<u64> {
    let response = unsafe { (*RSDP_REQUEST.get()).response };
    if response.is_null() {
        return None;
    }
    Some(unsafe { (*response).address as u64 })
}

pub fn kernel_address() -> Option<(u64, u64)> {
    let response = unsafe { (*EXECUTABLE_ADDRESS_REQUEST.get()).response };
    if response.is_null() {
        return None;
    }
    let response = unsafe { &*response };
    Some((response.virtual_base, response.physical_base))
}

pub fn modules() -> &'static [*mut File] {
    let response = unsafe { (*MODULE_REQUEST.get()).response };
    if response.is_null() {
        return &[];
    }
    let response = unsafe { &*response };
    unsafe { core::slice::from_raw_parts(response.modules, response.module_count as usize) }
}

pub fn module_path(file: *mut File) -> &'static str {
    if file.is_null() {
        return "";
    }
    let path = unsafe { (*file).path };
    if path.is_null() {
        return "";
    }
    let mut length = 0usize;
    while unsafe { *path.add(length) } != 0 && length < 4096 {
        length += 1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(path, length) };
    core::str::from_utf8(bytes).unwrap_or("")
}
