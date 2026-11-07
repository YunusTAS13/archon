//! Higher Half Direct Map: fiziksel bellek çekirdek sanal adres uzayında
//! `offset` kadar kaydırılmış olarak görünür.

use core::sync::atomic::{AtomicU64, Ordering};

static OFFSET: AtomicU64 = AtomicU64::new(0);

pub fn init(offset: u64) {
    OFFSET.store(offset, Ordering::Relaxed);
}

pub fn offset() -> u64 {
    OFFSET.load(Ordering::Relaxed)
}

#[inline]
pub fn phys_to_virt(physical: u64) -> u64 {
    physical + offset()
}

#[inline]
pub fn virt_to_phys(virtual_address: u64) -> u64 {
    virtual_address - offset()
}

#[inline]
pub fn phys_to_ptr<T>(physical: u64) -> *mut T {
    phys_to_virt(physical) as *mut T
}

/// # Safety
/// `physical..physical+size` geçerli bir fiziksel aralık olmalı.
pub unsafe fn phys_slice<T>(physical: u64, count: usize) -> &'static mut [T] {
    core::slice::from_raw_parts_mut(phys_to_ptr::<T>(physical), count)
}

pub unsafe fn zero_physical(physical: u64, size: usize) {
    let pointer = phys_to_ptr::<u8>(physical);
    core::ptr::write_bytes(pointer, 0, size);
}
