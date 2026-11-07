//! Bitmap tabanlı fiziksel çerçeve ayırıcı.
//!
//! Bitmap BSS'te sabit boyutlu durur: 4 KiB çerçeve ile 32 GiB'a kadar RAM'i
//! kapsar (1 MiB bitmap). Daha büyük makineler için bitmap PMM'in kendisinden
//! ayrılacak; o zamana kadar sınır açıkça raporlanır.

use core::sync::atomic::Ordering;

use super::hhdm;
use crate::boot::{limine, BootInfo};
use crate::sync::SpinLock;

pub const FRAME_SIZE: u64 = 4096;
const MAX_SUPPORTED_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_FRAMES: usize = (MAX_SUPPORTED_BYTES / FRAME_SIZE) as usize;
const BITMAP_BYTES: usize = MAX_FRAMES / 8;

static mut BITMAP: [u8; BITMAP_BYTES] = [0xFF; BITMAP_BYTES];

struct Allocator {
    frames: usize,
    free: usize,
    hint: usize,
}

static ALLOCATOR: SpinLock<Option<Allocator>> = SpinLock::new(None);

fn bitmap() -> &'static mut [u8] {
    unsafe { &mut *core::ptr::addr_of_mut!(BITMAP) }
}

fn is_free(bitmap: &[u8], frame: usize) -> bool {
    bitmap[frame / 8] & (1 << (frame % 8)) != 0
}

fn set_free(bitmap: &mut [u8], frame: usize) {
    bitmap[frame / 8] |= 1 << (frame % 8);
}

fn set_used(bitmap: &mut [u8], frame: usize) {
    bitmap[frame / 8] &= !(1 << (frame % 8));
}

fn mark_range(bitmap: &mut [u8], first: usize, count: usize, free: bool) {
    for frame in first..first.saturating_add(count).min(MAX_FRAMES) {
        if free {
            set_free(bitmap, frame);
        } else {
            set_used(bitmap, frame);
        }
    }
}

pub fn init(info: &BootInfo) {
    let bitmap = bitmap();
    // Frame sayısı yalnızca *kullanılabilir* belleğin sonuna kadar hesaplanır;
    // memmap'teki yüksek rezerve adresler (ör. 0x1_0000_0000 üstü MMIO)
    // bitmap'i şişirmesin.
    let highest = info
        .memmap
        .iter()
        .filter(|entry| entry.kind == limine::MEMMAP_USABLE)
        .map(|entry| entry.base.saturating_add(entry.length))
        .max()
        .unwrap_or(0);
    let frames = ((highest + FRAME_SIZE - 1) / FRAME_SIZE) as usize;
    let frames = frames.min(MAX_FRAMES);

    // Her şey dolu başlar; yalnızca USABLE aralıklar boşaltılır. Böylece
    // çekirdek, modüller, framebuffer ve ACPI tabloları kendiliğinden korunur.
    mark_range(bitmap, 0, frames, false);

    for entry in info.memmap.iter().filter(|entry| entry.kind == limine::MEMMAP_USABLE) {
        let first = (entry.base / FRAME_SIZE) as usize;
        let count = (entry.length / FRAME_SIZE) as usize;
        mark_range(bitmap, first, count, true);
    }

    // Sıfırıncı sayfa ve gerçek mod vektörleri asla verilmez.
    mark_range(bitmap, 0, 1, false);

    let free = (0..frames).filter(|frame| is_free(bitmap, *frame)).count();

    *ALLOCATOR.lock() = Some(Allocator {
        frames,
        free,
        hint: 1,
    });

    println!(
        "mem: PMM {} cefre ({} MiB), {} bos",
        frames,
        frames as u64 * FRAME_SIZE / (1024 * 1024),
        free
    );
}

/// Sıfırlanmış bir çerçevenin fiziksel adresi.
pub fn alloc_frame() -> Option<u64> {
    let mut allocator = ALLOCATOR.lock();
    let allocator = allocator.as_mut()?;
    let bitmap = bitmap();

    let frames = allocator.frames;
    let start = allocator.hint;

    for offset in 0..frames {
        let frame = (start + offset) % frames;
        if is_free(bitmap, frame) {
            set_used(bitmap, frame);
            allocator.free -= 1;
            allocator.hint = (frame + 1) % frames;
            let physical = frame as u64 * FRAME_SIZE;
            unsafe { hhdm::zero_physical(physical, FRAME_SIZE as usize) };
            return Some(physical);
        }
    }

    None
}

pub fn free_frame(physical: u64) {
    let frame = (physical / FRAME_SIZE) as usize;
    let mut allocator = ALLOCATOR.lock();
    let Some(allocator) = allocator.as_mut() else {
        return;
    };
    if frame >= allocator.frames || is_free(bitmap(), frame) {
        return;
    }
    set_free(bitmap(), frame);
    allocator.free += 1;
    allocator.hint = allocator.hint.min(frame);
}

/// `count` çerçeve ister. Fiziksel olarak bitişik olma garantisi yok.
pub fn alloc_frames(count: usize) -> Option<&'static mut [u64]> {
    let mut addresses = alloc::vec::Vec::with_capacity(count);
    for _ in 0..count {
        addresses.push(alloc_frame()?);
    }
    Some(alloc::boxed::Box::leak(addresses.into_boxed_slice()))
}

pub fn total_frames() -> usize {
    ALLOCATOR.lock().as_ref().map(|allocator| allocator.frames).unwrap_or(0)
}

pub fn free_frames() -> usize {
    ALLOCATOR.lock().as_ref().map(|allocator| allocator.free).unwrap_or(0)
}

pub fn used_bytes() -> u64 {
    let total = total_frames();
    let free = free_frames();
    (total - free) as u64 * FRAME_SIZE
}
