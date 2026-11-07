//! Fiziksel bellek yönetimi ve kernel heap.
//!
//! Kernel, Limine'nin kurduğu sayfa tablolarını devralır ve HHDM üzerinden tüm
//! fiziksel belleğe erişir. Bu yüzden M1'de heap için ayrı sanal eşleme
//! gerekmez; `heap` modülü BSS'teki bir arenayı kullanır ve PMM'e geçiş
//! M1 sonunda yapılır (bkz. ROADMAP).

pub mod heap;
pub mod hhdm;
pub mod pmm;
pub mod vmm;

use crate::boot::BootInfo;

pub fn init(info: &BootInfo) {
    hhdm::init(info.hhdm_offset);
    pmm::init(info);
    vmm::init();
    heap::init();
}
